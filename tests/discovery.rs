use feedr::{
    discovery::{Config, FileType, Input, extract, log_messages},
    herdr::sessions_from_snapshot,
    model::*,
    store::Store,
};
use serde_json::json;
use tempfile::tempdir;

fn setup(path: &std::path::Path) -> (Store, String) {
    let mut store = Store::open(&path.join("feed.sqlite3")).unwrap();
    let session = sessions_from_snapshot("/fake/herdr.sock", &json!({"panes":[{"pane_id":"w:p","workspace_id":"w","terminal_id":"term","agent":"codex","agent_status":"idle","cwd":path}],"workspaces":[{"workspace_id":"w","label":"Project"}]}), &[]).pop().unwrap();
    store
        .reconcile("/fake/herdr.sock", vec![session.clone()])
        .unwrap();
    (store, session.id)
}
fn input(session: &str, text: &str) -> Input {
    Input {
        session_id: session.into(),
        text: text.into(),
        source: "completed assistant message".into(),
    }
}
fn enable(store: &mut Store) {
    store
        .set_discovery_config(&Config {
            enabled: true,
            ..Config::default()
        })
        .unwrap();
}
#[test]
fn discovery_types_distinguish_markdown_plain_text_code_and_audio() {
    let context = Context {
        cwd: "/project".into(),
        ..Context::default()
    };
    for (uri, expected) in [
        ("./README.MD", FileType::Markdown),
        ("./guide.mdx", FileType::Markdown),
        ("./notes.txt", FileType::Text),
        ("./results.csv", FileType::Text),
        ("./main.rs", FileType::Code),
        ("./config.toml", FileType::Code),
        ("./Dockerfile", FileType::Code),
        ("./.env.local", FileType::Code),
        ("./plot.svg", FileType::Image),
        ("./clip.mov", FileType::Video),
        ("./music.M4A", FileType::Audio),
        ("./speech.opus", FileType::Audio),
        ("./report.pdf", FileType::Pdf),
        ("https://example.test/report.md", FileType::Web),
        ("./archive.zip", FileType::Other),
    ] {
        let resource = Resource::resolve(
            ResourceInput {
                uri: uri.into(),
                kind: None,
            },
            &context,
        )
        .unwrap();
        assert_eq!(FileType::of(&resource), expected, "{uri}");
    }
    let old: Config = serde_json::from_value(json!({"enabled":true})).unwrap();
    assert_eq!(old.file_types, FileType::ALL);
    let mut scoped = Config {
        sessions: Some(vec!["one".into()]),
        excluded_sessions: vec!["two".into()],
        ..Config::default()
    };
    scoped.set_all_sessions(true);
    assert!(scoped.includes("one") && scoped.includes("two") && scoped.includes("future-session"));
}
#[test]
fn markdown_discovery_is_not_crowded_out_by_excluded_code_mentions() {
    let dir = tempdir().unwrap();
    let (mut store, session) = setup(dir.path());
    store
        .set_discovery_config(&Config {
            enabled: true,
            file_types: vec![FileType::Markdown],
            ..Config::default()
        })
        .unwrap();
    let mut text = (0..45)
        .map(|number| format!("`./a{number:02}.rs`\n"))
        .collect::<String>();
    text.push_str("`./notes.txt` `./picture.png` `./song.mp3` `./clip.mp4` `./report.pdf` `./z-guide.md` `./z-second.markdown`");
    let result = store.ingest(&input(&session, &text)).unwrap();
    assert_eq!(result.created.len(), 2);
    let snapshot = store.snapshot(&FeedQuery::default()).unwrap();
    assert_eq!((snapshot.total, snapshot.unread), (2, 2));
    assert!(
        snapshot
            .posts
            .iter()
            .all(|post| FileType::of(&post.resources[0]) == FileType::Markdown)
    );
}
#[test]
fn audio_filter_finds_bare_audio_names_without_matching_source_or_prose() {
    let dir = tempdir().unwrap();
    let (mut store, session) = setup(dir.path());
    store
        .set_discovery_config(&Config {
            enabled: true,
            file_types: vec![FileType::Audio],
            ..Config::default()
        })
        .unwrap();
    let result = store
        .ingest(&input(
            &session,
            "Audio: song.mp3, voice.WAV and music.flac; main.rs is source, e.g. it is not audio.",
        ))
        .unwrap();
    assert_eq!(result.created.len(), 3);
    assert!(
        store
            .snapshot(&FeedQuery::default())
            .unwrap()
            .posts
            .iter()
            .all(|post| FileType::of(&post.resources[0]) == FileType::Audio)
    );
}
#[test]
fn type_changes_hide_only_unkept_mentions_and_preserve_counts_identity_and_preferences() {
    let dir = tempdir().unwrap();
    let (mut store, session) = setup(dir.path());
    enable(&mut store);
    let results = store
        .ingest(&input(&session, "`./main.rs` `./guide.md` `./dismiss.rs`"))
        .unwrap();
    let posts: Vec<_> = results
        .created
        .iter()
        .map(|id| store.get(id).unwrap())
        .collect();
    let code = posts.iter().find(|post| post.title == "main.rs").unwrap();
    let archived = posts
        .iter()
        .find(|post| post.title == "dismiss.rs")
        .unwrap();
    let mut dismiss = update(archived);
    dismiss.archived = Some(true);
    store.update(dismiss).unwrap();
    let mut config = store.discovery_config().unwrap();
    config.file_types = vec![FileType::Markdown];
    store.set_discovery_config(&config).unwrap();
    let filtered = store
        .snapshot(&FeedQuery {
            limit: 1,
            ..FeedQuery::default()
        })
        .unwrap();
    assert_eq!(
        (filtered.total, filtered.unread, filtered.posts.len()),
        (1, 1, 1)
    );
    assert_eq!(filtered.posts[0].title, "guide.md");
    assert_eq!(
        store
            .snapshot(&FeedQuery {
                archived: true,
                ..FeedQuery::default()
            })
            .unwrap()
            .total,
        1
    );
    let mut pin = update(code);
    pin.pinned = Some(true);
    let pinned = store.update(pin).unwrap();
    assert_eq!(store.snapshot(&FeedQuery::default()).unwrap().total, 2);
    let mut unpin = update(&pinned);
    unpin.pinned = Some(false);
    let unpinned = store.update(unpin).unwrap();
    assert_eq!(store.snapshot(&FeedQuery::default()).unwrap().total, 1);
    store.keep_discovery(&code.id, unpinned.revision).unwrap();
    let explicit = store
        .post(PostInput {
            title: "Agent-posted source".into(),
            body: String::new(),
            resources: vec![ResourceInput {
                uri: "./explicit.rs".into(),
                kind: None,
            }],
            context: None,
            session_id: Some(session.clone()),
            idempotency_key: None,
            readiness: "ready".into(),
        })
        .unwrap();
    assert_eq!(store.snapshot(&FeedQuery::default()).unwrap().total, 3);
    config.file_types.clear();
    store.set_discovery_config(&config).unwrap();
    assert_eq!(store.snapshot(&FeedQuery::default()).unwrap().total, 2);
    assert!(
        store
            .ingest(&input(&session, "`./new.md`"))
            .unwrap()
            .created
            .is_empty()
    );
    drop(store);
    let mut store = Store::open(&dir.path().join("feed.sqlite3")).unwrap();
    assert!(store.discovery_config().unwrap().file_types.is_empty());
    config.file_types = FileType::ALL.to_vec();
    store.set_discovery_config(&config).unwrap();
    let all = store.snapshot(&FeedQuery::default()).unwrap();
    assert_eq!(all.total, 3);
    assert!(all.posts.iter().any(|post| post.id == code.id));
    assert!(all.posts.iter().any(|post| post.id == explicit.id));
}
#[test]
fn v3_discoveries_gain_filter_metadata_and_legacy_settings_keep_all_types() {
    let dir = tempdir().unwrap();
    let (mut store, session) = setup(dir.path());
    enable(&mut store);
    let discovered = store
        .ingest(&input(&session, "`./main.rs` `./guide.md`"))
        .unwrap();
    drop(store);
    let db = dir.path().join("feed.sqlite3");
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection.execute_batch("ALTER TABLE posts DROP COLUMN discovery_type; UPDATE settings SET json=json_remove(json,'$.file_types') WHERE key='discovery.config'; PRAGMA user_version=3;").unwrap();
    drop(connection);
    let mut store = Store::open(&db).unwrap();
    let mut config = store.discovery_config().unwrap();
    assert_eq!(config.file_types, FileType::ALL);
    config.file_types = vec![FileType::Markdown];
    store.set_discovery_config(&config).unwrap();
    let snapshot = store.snapshot(&FeedQuery::default()).unwrap();
    assert_eq!(snapshot.total, 1);
    assert_eq!(snapshot.posts[0].title, "guide.md");
    assert!(discovered.created.iter().all(|id| store.get(id).is_ok()));
}
fn update(post: &Post) -> UpdateInput {
    UpdateInput {
        id: post.id.clone(),
        expected_revision: post.revision,
        title: None,
        body: None,
        resources: None,
        readiness: None,
        pinned: None,
        archived: None,
        read: None,
    }
}

#[test]
fn parsing_is_deterministic_and_suppresses_code_logs_and_dependencies() {
    let text = "\x1b[32mReady\x1b[0m: [PDF](<./Final report.pdf>) and `/tmp/image.png`.\nSee https://example.com/a_(b).\nAlso `report.md` and ./report.md\nCompiling /tmp/crate.rs\nCheck `/repo/node_modules/cache.json` and `/repo/target/debug/output.txt`.\n```sh\ncargo run /tmp/do-not-post.pdf\n```";
    let expected = vec![
        "./Final report.pdf",
        "./report.md",
        "/tmp/image.png",
        "https://example.com/a_(b)",
        "report.md",
    ];
    let first = extract(text, &Config::default());
    assert_eq!(first, expected);
    assert_eq!(extract(text, &Config::default()), first);
    let filtered = Config {
        exclude_paths: vec!["**/*.png".into()],
        ..Config::default()
    };
    assert!(!extract(text, &filtered).contains(&"/tmp/image.png".into()));
}
#[test]
fn harness_jsonl_extracts_assistant_text_and_ignores_users_tools_and_partial_data() {
    let codex = json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Ready: ./report.pdf"}]}});
    assert_eq!(
        log_messages(&codex.to_string(), "codex"),
        ["Ready: ./report.pdf"]
    );
    let claude = json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"View ./image.png"},{"type":"tool_use","input":{"path":"/tmp/no.md"}}]}});
    assert_eq!(
        log_messages(&claude.to_string(), "claude"),
        ["View ./image.png"]
    );
    assert!(
        log_messages(
            r#"{"type":"user","message":{"role":"user","content":"/tmp/no.pdf"}}"#,
            "jsonl"
        )
        .is_empty()
    );
    assert!(log_messages("{\"type\":\"assistant\"", "jsonl").is_empty());
}
#[test]
fn discovery_is_opt_in_and_respects_session_exclusions() {
    let dir = tempdir().unwrap();
    let (mut store, session) = setup(dir.path());
    assert!(
        store
            .ingest(&input(&session, "Ready: ./report.md"))
            .is_err()
    );
    assert_eq!(store.snapshot(&FeedQuery::default()).unwrap().total, 0);
    enable(&mut store);
    let mut config = store.discovery_config().unwrap();
    config.excluded_sessions.push(session.clone());
    store.set_discovery_config(&config).unwrap();
    assert!(
        store
            .ingest(&input(&session, "Ready: ./report.md"))
            .is_err()
    );
    assert!(
        store
            .ingest(&input("unknown-session", "Ready: ./report.md"))
            .is_err()
    );
}
#[test]
fn a_session_whitelist_never_turns_into_all_sessions_when_last_session_is_unchecked() {
    let mut config = Config {
        enabled: true,
        sessions: Some(vec!["only-this-session".into()]),
        ..Config::default()
    };
    assert!(config.includes("only-this-session"));
    assert!(!config.includes("another-session"));
    config.set_session("only-this-session", false);
    assert!(!config.includes("only-this-session"));
    assert!(!config.includes("another-session"));
}
#[test]
fn a_v2_database_upgrades_with_legacy_posts_and_discovery_disabled() {
    let dir = tempdir().unwrap();
    let (mut store, session) = setup(dir.path());
    let post = store
        .post(PostInput {
            title: "Legacy guide".into(),
            body: String::new(),
            resources: vec![ResourceInput {
                uri: "./guide.md".into(),
                kind: None,
            }],
            context: None,
            session_id: Some(session.clone()),
            idempotency_key: Some("legacy".into()),
            readiness: "ready".into(),
        })
        .unwrap();
    drop(store);
    let db = dir.path().join("feed.sqlite3");
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection.execute_batch("DROP TABLE post_keys; DROP TABLE discovery_mentions; DROP TABLE settings; UPDATE posts SET json=json_remove(json,'$.discovery'); PRAGMA user_version=2;").unwrap();
    drop(connection);
    let mut store = Store::open(&db).unwrap();
    assert_eq!(store.get(&post.id).unwrap().title, "Legacy guide");
    assert!(store.get(&post.id).unwrap().discovery.is_none());
    assert!(!store.discovery_config().unwrap().enabled);
    enable(&mut store);
    let mention = store.ingest(&input(&session, "See ./guide.md")).unwrap();
    assert!(mention.created.is_empty());
    assert_eq!(mention.matched, [post.id]);
}
#[test]
fn repeated_mentions_and_dismissal_survive_restarts_without_duplicates() {
    let dir = tempdir().unwrap();
    let (mut store, session) = setup(dir.path());
    enable(&mut store);
    let message = input(&session, "Ready: [report](./report.md)");
    let first = store.ingest(&message).unwrap();
    assert_eq!(first.created.len(), 1);
    let post = store.get(&first.created[0]).unwrap();
    assert!(post.discovery.is_some());
    assert_eq!(post.context.cwd, dir.path().to_string_lossy());
    let mut dismissed = update(&post);
    dismissed.archived = Some(true);
    store.update(dismissed).unwrap();
    drop(store);
    let mut store = Store::open(&dir.path().join("feed.sqlite3")).unwrap();
    let repeated = store.ingest(&message).unwrap();
    assert!(repeated.created.is_empty());
    assert_eq!(repeated.matched, first.created);
    assert_eq!(store.snapshot(&FeedQuery::default()).unwrap().total, 0);
    assert!(store.get(&repeated.matched[0]).unwrap().archived);
}
#[test]
fn explicit_publication_promotes_a_matching_discovery_and_keeps_retry_identity() {
    let dir = tempdir().unwrap();
    let (mut store, session) = setup(dir.path());
    enable(&mut store);
    let discovered = store
        .ingest(&input(&session, "Ready: ./report.md"))
        .unwrap();
    let publish = PostInput {
        title: "Final report".into(),
        body: String::new(),
        resources: vec![ResourceInput {
            uri: dir.path().join("report.md").to_string_lossy().into(),
            kind: None,
        }],
        context: None,
        session_id: Some(session.clone()),
        idempotency_key: Some("final-report".into()),
        readiness: "ready".into(),
    };
    let first = store.post(publish.clone()).unwrap();
    assert_eq!(first.id, discovered.created[0]);
    assert!(first.discovery.as_ref().unwrap().accepted);
    let retried = store.post(publish).unwrap();
    assert_eq!(retried.revision, first.revision);
    assert_eq!(store.snapshot(&FeedQuery::default()).unwrap().total, 1);
    let repeated = store
        .ingest(&input(&session, "./report.md is ready again"))
        .unwrap();
    assert!(repeated.created.is_empty());
    assert_eq!(repeated.matched, [first.id]);
}
#[test]
fn mentions_deduplicate_against_explicit_resources_but_keep_sessions_separate() {
    let dir = tempdir().unwrap();
    let (mut store, session) = setup(dir.path());
    enable(&mut store);
    let published = store
        .post(PostInput {
            title: "Guide".into(),
            body: String::new(),
            resources: vec![ResourceInput {
                uri: "./guide.md".into(),
                kind: None,
            }],
            context: None,
            session_id: Some(session.clone()),
            idempotency_key: None,
            readiness: "ready".into(),
        })
        .unwrap();
    let result = store.ingest(&input(&session, "Read ./guide.md")).unwrap();
    assert!(result.created.is_empty());
    assert_eq!(result.matched, [published.id]);
    let mut second = store.sessions().unwrap().pop().unwrap();
    second.id = "second-session".into();
    store.save_session(&second).unwrap();
    assert_eq!(
        store
            .ingest(&input(&second.id, "Read ./guide.md"))
            .unwrap()
            .created
            .len(),
        1
    );
}
#[test]
fn ambiguous_paths_remain_unresolved_and_cannot_open_a_local_file_by_accident() {
    let dir = tempdir().unwrap();
    let (mut store, session) = setup(dir.path());
    enable(&mut store);
    let mut context = store.sessions().unwrap().pop().unwrap();
    context.context.cwd.clear();
    store.save_session(&context).unwrap();
    let result = store.ingest(&input(&session, "See `Cargo.toml`.")).unwrap();
    let post = store.get(&result.created[0]).unwrap();
    assert_eq!(post.readiness, "unresolved");
    assert!(post.discovery.as_ref().unwrap().unresolved);
    assert!(feedr::platform::open_resource(&post, &post.resources[0]).is_err());
    assert!(feedr::platform::reveal(&post, &post.resources[0]).is_err());
    let kept = store.keep_discovery(&post.id, post.revision).unwrap();
    assert!(kept.discovery.unwrap().accepted);
    assert!(store.keep_discovery(&post.id, post.revision).is_err());
}

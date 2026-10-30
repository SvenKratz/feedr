use feedr::{herdr::sessions_from_snapshot, model::*, store::Store};
use serde_json::json;
use tempfile::tempdir;

fn post(cwd: &std::path::Path) -> PostInput {
    PostInput {
        title: "Report is ready".into(),
        body: "Inspect this result".into(),
        resources: vec![ResourceInput {
            uri: "reports/final draft.md".into(),
            kind: None,
        }],
        context: Some(Context {
            cwd: cwd.to_string_lossy().into_owned(),
            ..Default::default()
        }),
        session_id: None,
        idempotency_key: Some("result-1".into()),
        readiness: "ready".into(),
    }
}
fn update(p: &Post) -> UpdateInput {
    serde_json::from_value(json!({"id":p.id,"expected_revision":p.revision})).unwrap()
}

#[test]
fn acknowledged_posts_survive_reopen_and_retry() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("feed.sqlite");
    let mut store = Store::open(&db).unwrap();
    let p = store.post(post(dir.path())).unwrap();
    assert_eq!(
        p.resources[0].target,
        dir.path()
            .canonicalize()
            .unwrap()
            .join("reports/final draft.md")
            .to_string_lossy()
    );
    drop(store);
    let mut store = Store::open(&db).unwrap();
    let retried = store.post(post(dir.path())).unwrap();
    assert_eq!(p.id, retried.id);
    assert_eq!(store.snapshot(&FeedQuery::default()).unwrap().total, 1);
}
#[test]
fn revisions_prevent_lost_updates_and_preserve_idempotency() {
    let dir = tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("db")).unwrap();
    let p = store.post(post(dir.path())).unwrap();
    let mut change = update(&p);
    change.pinned = Some(true);
    change.title = Some("Revised report".into());
    let revised = store.update(change).unwrap();
    assert!(revised.pinned);
    assert_eq!(revised.revision, 2);
    assert!(
        store
            .update(update(&p))
            .unwrap_err()
            .to_string()
            .contains("Revision conflict")
    );
    assert_eq!(store.post(post(dir.path())).unwrap().id, p.id);
    drop(store);
    let store = Store::open(&dir.path().join("db")).unwrap();
    assert!(store.get(&p.id).unwrap().pinned);
}
#[test]
fn filtering_archiving_and_session_scoped_deduplication() {
    let dir = tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("db")).unwrap();
    let p = store.post(post(dir.path())).unwrap();
    let mut another = post(dir.path());
    another.context.as_mut().unwrap().label = "Other agent".into();
    let second = store.post(another).unwrap();
    assert_ne!(p.id, second.id);
    let mut change = update(&p);
    change.archived = Some(true);
    store.update(change).unwrap();
    assert_eq!(store.snapshot(&FeedQuery::default()).unwrap().total, 1);
    let archived = FeedQuery {
        archived: true,
        search: "final draft".into(),
        ..Default::default()
    };
    assert_eq!(store.snapshot(&archived).unwrap().posts[0].id, p.id);
}

#[test]
fn availability_filters_before_pagination_and_keeps_counts_and_history_accurate() {
    let dir = tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("db")).unwrap();
    std::fs::write(dir.path().join("available.md"), "# Ready").unwrap();
    let mut input = post(dir.path());
    input.resources = vec![
        ResourceInput {
            uri: "available.md".into(),
            kind: None,
        },
        ResourceInput {
            uri: "missing.png".into(),
            kind: None,
        },
    ];
    let available = store.post(input).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    for i in 0..8 {
        let mut input = post(dir.path());
        input.idempotency_key = Some(format!("unavailable-{i}"));
        input.resources[0].uri = format!("missing-{i}.md");
        store.post(input).unwrap();
    }
    let query = FeedQuery {
        available_only: true,
        limit: 1,
        ..Default::default()
    };
    let snapshot = store.snapshot(&query).unwrap();
    assert_eq!(
        (snapshot.total, snapshot.unread, snapshot.posts.len()),
        (1, 1, 1)
    );
    assert_eq!(snapshot.posts[0].id, available.id);
    assert_eq!(snapshot.posts[0].resources.len(), 1);
    assert_eq!(store.get(&available.id).unwrap().resources.len(), 2);
    assert_eq!(
        store
            .snapshot(&FeedQuery {
                kind: Some("Image".into()),
                ..query.clone()
            })
            .unwrap()
            .total,
        0
    );
    assert_eq!(store.snapshot(&FeedQuery::default()).unwrap().total, 9);
    let mut change = update(&available);
    change.read = Some(true);
    store.update(change).unwrap();
    assert_eq!(store.snapshot(&query).unwrap().unread, 0);
    let sequence = store.sequence().unwrap();
    std::fs::remove_file(dir.path().join("available.md")).unwrap();
    assert_eq!(store.snapshot(&query).unwrap().total, 0);
    std::fs::write(dir.path().join("available.md"), "# Returned").unwrap();
    assert_eq!(store.snapshot(&query).unwrap().posts[0].id, available.id);
    assert_eq!(store.sequence().unwrap(), sequence);
}

#[test]
fn availability_accepts_web_links_and_folders_without_confusing_remote_paths_with_local_files() {
    let dir = tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("db")).unwrap();
    let folder = dir.path().join("outputs");
    std::fs::create_dir(&folder).unwrap();
    let mut input = post(dir.path());
    input.resources = vec![
        ResourceInput {
            uri: "https://feedr.invalid/report".into(),
            kind: None,
        },
        ResourceInput {
            uri: folder.to_string_lossy().into_owned(),
            kind: None,
        },
    ];
    let local = store.post(input).unwrap();
    let mut input = post(dir.path());
    input.idempotency_key = Some("remote".into());
    input.context.as_mut().unwrap().host = "another-host".into();
    input.resources[0].uri = folder.to_string_lossy().into_owned();
    store.post(input).unwrap();
    let query = FeedQuery {
        available_only: true,
        ..Default::default()
    };
    let snapshot = store.snapshot(&query).unwrap();
    assert_eq!(snapshot.total, 1);
    assert_eq!(snapshot.posts[0].id, local.id);
    assert_eq!(snapshot.posts[0].resources.len(), 2);
    std::fs::remove_dir(&folder).unwrap();
    let snapshot = store.snapshot(&query).unwrap();
    assert_eq!(snapshot.posts[0].resources.len(), 1);
    assert_eq!(snapshot.posts[0].resources[0].kind, ResourceKind::Url);
    assert_eq!(store.snapshot(&FeedQuery::default()).unwrap().total, 2);
}

#[cfg(unix)]
#[test]
fn availability_waits_for_symlink_targets_to_exist() {
    let dir = tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("db")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("future.md"), dir.path().join("link.md")).unwrap();
    let mut input = post(dir.path());
    input.resources[0].uri = "link.md".into();
    let pending = store.post(input).unwrap();
    let query = FeedQuery {
        available_only: true,
        ..Default::default()
    };
    assert_eq!(store.snapshot(&query).unwrap().total, 0);
    std::fs::write(dir.path().join("future.md"), "# Created later").unwrap();
    assert_eq!(store.snapshot(&query).unwrap().posts[0].id, pending.id);
    std::fs::remove_file(dir.path().join("future.md")).unwrap();
    assert_eq!(store.snapshot(&query).unwrap().total, 0);
}
#[test]
fn invalid_inputs_and_unsafe_uri_schemes_are_rejected() {
    let dir = tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("db")).unwrap();
    for uri in ["javascript://alert", "https://", "\0"] {
        let mut p = post(dir.path());
        p.resources[0].uri = uri.into();
        assert!(store.post(p).is_err());
    }
    let mut p = post(dir.path());
    p.title = " ".into();
    assert!(store.post(p).is_err());
    let mut p = post(dir.path());
    p.session_id = Some("nonexistent".into());
    assert!(store.post(p).is_err());
}
#[test]
fn resources_keep_remote_identity_and_do_not_expand_remote_home() {
    let context = Context {
        host: "different-machine".into(),
        cwd: "/workspace".into(),
        ..Default::default()
    };
    let r = Resource::resolve(
        ResourceInput {
            uri: "report.md".into(),
            kind: None,
        },
        &context,
    )
    .unwrap();
    assert_eq!(r.target, "/workspace/report.md");
    assert!(
        Resource::resolve(
            ResourceInput {
                uri: "~/report.md".into(),
                kind: None
            },
            &context
        )
        .is_err()
    );
    let p = Post {
        discovery: None,
        id: new_id(),
        session_id: new_id(),
        title: "remote".into(),
        body: String::new(),
        resources: vec![r.clone()],
        context,
        readiness: "ready".into(),
        revision: 1,
        created_at: now(),
        updated_at: now(),
        pinned: false,
        archived: false,
        read: false,
    };
    assert!(
        feedr::platform::open_resource(&p, &r)
            .unwrap_err()
            .to_string()
            .contains("another machine")
    );
}
fn snapshot(native: Option<&str>, terminal: &str) -> serde_json::Value {
    let mut p = json!({"agent":"codex","pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","terminal_id":terminal,"cwd":"/project","agent_status":"working"});
    if let Some(id) = native {
        p["agent_session"] = json!({"value":id});
    }
    json!({"panes":[p],"workspaces":[{"workspace_id":"w1","label":"Project"}]})
}
#[test]
fn herdr_native_session_identity_survives_pane_reuse_and_restarts() {
    let first = sessions_from_snapshot("/herdr/a.sock", &snapshot(Some("native-a"), "term1"), &[]);
    let resumed =
        sessions_from_snapshot("/herdr/a.sock", &snapshot(Some("native-a"), "term2"), &[]);
    assert_eq!(first[0].id, resumed[0].id);
    let replacement = sessions_from_snapshot(
        "/herdr/a.sock",
        &snapshot(Some("native-b"), "term1"),
        &first,
    );
    assert_ne!(first[0].id, replacement[0].id);
    let other_server =
        sessions_from_snapshot("/herdr/b.sock", &snapshot(Some("native-a"), "term1"), &[]);
    assert_ne!(first[0].id, other_server[0].id);
}
#[test]
fn native_identity_can_arrive_after_detection_without_splitting_session() {
    let initial = sessions_from_snapshot("/herdr/a.sock", &snapshot(None, "term1"), &[]);
    let enriched = sessions_from_snapshot(
        "/herdr/a.sock",
        &snapshot(Some("native-a"), "term1"),
        &initial,
    );
    assert_eq!(initial[0].id, enriched[0].id);
    assert_eq!(
        enriched[0].context.native_session_id.as_deref(),
        Some("native-a")
    );
    let mut offline = enriched.clone();
    offline[0].online = false;
    let resumed = sessions_from_snapshot(
        "/herdr/a.sock",
        &snapshot(Some("native-a"), "replacement-terminal"),
        &offline,
    );
    assert_eq!(enriched[0].id, resumed[0].id);
}
#[test]
fn herdr_sessions_capture_workspace_order_and_agent_state_sequence() {
    let snapshot = json!({
        "workspaces": [
            {"workspace_id":"z", "label":"Zebra"},
            {"workspace_id":"a", "label":"Alpha"}
        ],
        "panes": [
            {"pane_id":"z:p2", "workspace_id":"z", "agent":"codex", "agent_status":"working"},
            {"pane_id":"z:p1", "workspace_id":"z", "agent":"claude", "agent_status":"idle"},
            {"pane_id":"a:p1", "workspace_id":"a", "agent":"codex", "agent_status":"working"}
        ],
        "agents": [
            {"pane_id":"a:p1", "agent_status":"done", "state_change_seq":100},
            {"pane_id":"z:p2", "agent_status":"blocked", "state_change_seq":101}
        ]
    });
    let sessions = sessions_from_snapshot("/herdr/socket", &snapshot, &[]);
    assert_eq!(sessions[0].workspace_label.as_deref(), Some("Zebra"));
    assert_eq!(sessions[0].workspace_order, 0);
    assert_eq!(sessions[0].pane_order, 0);
    assert_eq!(sessions[0].status, "blocked");
    assert_eq!(sessions[0].state_change_seq, 101);
    assert_eq!(sessions[1].pane_order, 1);
    assert_eq!(sessions[1].status, "idle");
    assert_eq!(sessions[1].state_change_seq, 0);
    assert_eq!(sessions[2].workspace_label.as_deref(), Some("Alpha"));
    assert_eq!(sessions[2].workspace_order, 1);
    assert_eq!(sessions[2].status, "done");
    assert_eq!(sessions[2].state_change_seq, 100);
}
#[test]
fn existing_session_json_loads_without_sidebar_ordering_metadata() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("db");
    let mut store = Store::open(&db).unwrap();
    let session = sessions_from_snapshot("/herdr/socket", &snapshot(None, "terminal"), &[])
        .pop()
        .unwrap();
    store.save_session(&session).unwrap();
    let mut legacy = serde_json::to_value(&session).unwrap();
    for field in [
        "workspace_label",
        "workspace_order",
        "pane_order",
        "state_change_seq",
        "display_name",
        "display_agent",
        "last_activity_at",
    ] {
        legacy.as_object_mut().unwrap().remove(field);
    }
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE sessions SET json=?1 WHERE id=?2",
        [&legacy.to_string(), &session.id],
    )
    .unwrap();
    drop(conn);
    drop(store);

    let mut store = Store::open(&db).unwrap();
    let restored = store.sessions().unwrap().pop().unwrap();
    assert_eq!(restored.id, session.id);
    assert_eq!(restored.workspace_label, None);
    assert_eq!(restored.state_change_seq, 0);
    assert_eq!(restored.workspace_order, 0);
    assert_eq!(restored.pane_order, 0);
    assert_eq!(restored.display_name, None);
    assert_eq!(restored.display_agent, None);
    assert_eq!(restored.last_activity_at, None);
    // The next authoritative refresh enriches the existing row in place.
    store
        .reconcile("/herdr/socket", vec![session.clone()])
        .unwrap();
    assert_eq!(store.sessions().unwrap()[0], session);
}
#[test]
fn herdr_display_names_match_workspace_and_custom_tabs_and_polls_do_not_refresh_recency() {
    let mut input = snapshot(Some("native-a"), "term1");
    input["workspaces"][0]["label"] = json!("glyph and glyphaudio");
    input["panes"][0]["title"] = json!("Build glyph visualization library | glyph_vis");
    input["tabs"] = json!([{"tab_id":"w1:t1","workspace_id":"w1","number":1,"label":"1"}]);
    input["agents"] = json!([{"pane_id":"w1:p1","agent_status":"working","state_change_seq":10,"display_agent":"codex"}]);
    let mut first = sessions_from_snapshot("/herdr/socket", &input, &[]);
    assert_eq!(
        first[0].display_name.as_deref(),
        Some("glyph and glyphaudio")
    );
    assert_eq!(first[0].display_agent.as_deref(), Some("codex"));
    assert_eq!(
        first[0].context.label,
        "Build glyph visualization library | glyph_vis"
    );
    assert_eq!(first[0].last_activity_at, None);
    let known_activity = now() - 600_000;
    first[0].last_activity_at = Some(known_activity);
    input["agents"][0]["focused"] = json!(true);
    input["agents"][0]["revision"] = json!(900);
    input["panes"][0]["title"] = json!("Renamed task");
    let polled = sessions_from_snapshot("/herdr/socket", &input, &first);
    assert_eq!(polled[0].last_activity_at, Some(known_activity));
    input["tabs"][0]["label"] = json!("audio");
    input["agents"][0]["state_change_seq"] = json!(11);
    input["agents"][0]["agent_status"] = json!("done");
    let changed = sessions_from_snapshot("/herdr/socket", &input, &polled);
    assert_eq!(
        changed[0].display_name.as_deref(),
        Some("glyph and glyphaudio · audio")
    );
    assert!(changed[0].last_activity_at.unwrap() > known_activity);
    input["agents"][0]["state_change_seq"] = json!(1); // Herdr restarted, not new activity.
    let reset = sessions_from_snapshot("/herdr/socket", &input, &changed);
    assert_eq!(reset[0].last_activity_at, changed[0].last_activity_at);
    let dir = tempdir().unwrap();
    let db = dir.path().join("db");
    let mut store = Store::open(&db).unwrap();
    store.reconcile("/herdr/socket", changed.clone()).unwrap();
    drop(store);
    let store = Store::open(&db).unwrap();
    let restored = store.sessions().unwrap();
    let reconnected = sessions_from_snapshot("/herdr/socket", &input, &restored);
    assert_eq!(reconnected[0].last_activity_at, changed[0].last_activity_at);
}

#[test]
fn publication_recency_survives_restart_and_read_pin_and_retry_do_not_fake_activity() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("db");
    let mut store = Store::open(&db).unwrap();
    let first = store.post(post(dir.path())).unwrap();
    let published = store.sessions().unwrap()[0].last_activity_at;
    assert_eq!(published, Some(first.created_at));
    std::thread::sleep(std::time::Duration::from_millis(3));
    let mut flags = update(&first);
    flags.read = Some(true);
    flags.pinned = Some(true);
    flags.archived = Some(true);
    let marked = store.update(flags).unwrap();
    assert_eq!(store.sessions().unwrap()[0].last_activity_at, published);
    store.post(post(dir.path())).unwrap();
    assert_eq!(store.sessions().unwrap()[0].last_activity_at, published);
    let mut content = update(&marked);
    content.body = Some("A revised deliverable".into());
    let revised = store.update(content).unwrap();
    let activity = store.sessions().unwrap()[0].last_activity_at;
    assert_eq!(activity, Some(revised.updated_at));
    assert!(activity > published);
    drop(store);
    let store = Store::open(&db).unwrap();
    assert_eq!(store.sessions().unwrap()[0].last_activity_at, activity);
    // Legacy JSON can still show a real publication age without a timestamp migration.
    let conn = rusqlite::Connection::open(&db).unwrap();
    let mut legacy = serde_json::to_value(&store.sessions().unwrap()[0]).unwrap();
    legacy.as_object_mut().unwrap().remove("last_activity_at");
    conn.execute("UPDATE sessions SET json=?1", [legacy.to_string()])
        .unwrap();
    assert_eq!(
        store.sessions().unwrap()[0].last_activity_at,
        Some(first.created_at)
    );
}

#[test]
fn terminal_only_sessions_keep_observed_activity_after_broker_restart() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("db");
    let input = snapshot(None, "terminal-only");
    let mut sessions = sessions_from_snapshot("/herdr/socket", &input, &[]);
    sessions[0].last_activity_at = Some(now() - 900_000);
    let expected = sessions[0].clone();
    let mut store = Store::open(&db).unwrap();
    store.reconcile("/herdr/socket", sessions).unwrap();
    drop(store);
    let store = Store::open(&db).unwrap();
    let restored = sessions_from_snapshot("/herdr/socket", &input, &store.sessions().unwrap());
    assert_eq!(restored[0].id, expected.id);
    assert_eq!(restored[0].last_activity_at, expected.last_activity_at);
}
#[test]
fn authoritative_snapshots_mark_closed_sessions_offline() {
    let dir = tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("db")).unwrap();
    let sessions =
        sessions_from_snapshot("/herdr/a.sock", &snapshot(Some("native-a"), "term1"), &[]);
    store.reconcile("/herdr/a.sock", sessions.clone()).unwrap();
    store.reconcile("/herdr/a.sock", vec![]).unwrap();
    assert!(!store.sessions().unwrap()[0].online);
    let mut input = post(dir.path());
    input.session_id = Some(sessions[0].id.clone());
    let p = store.post(input).unwrap();
    assert_eq!(p.context.agent, "codex");
}
#[test]
fn pipe_messages_are_bounded_and_require_newline() {
    assert!(feedr::broker::bounded_line("{}".as_bytes()).is_err());
    assert!(feedr::broker::bounded_line(vec![b'x'; 1_048_578].as_slice()).is_err());
    assert_eq!(
        feedr::broker::bounded_line("{}\n".as_bytes()).unwrap(),
        "{}\n"
    );
}

#[test]
fn resolving_parent_components_under_symlinks_uses_filesystem_semantics() {
    let dir = tempdir().unwrap();
    let elsewhere = tempdir().unwrap();
    std::fs::create_dir(elsewhere.path().join("nested")).unwrap();
    std::fs::write(elsewhere.path().join("report.md"), "correct").unwrap();
    std::os::unix::fs::symlink(elsewhere.path().join("nested"), dir.path().join("link")).unwrap();
    let context = Context {
        cwd: dir.path().to_string_lossy().into_owned(),
        ..Default::default()
    };
    let r = Resource::resolve(
        ResourceInput {
            uri: "link/../report.md".into(),
            kind: None,
        },
        &context,
    )
    .unwrap();
    assert_eq!(
        r.target,
        elsewhere
            .path()
            .join("report.md")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
    );
}

#[test]
fn type_filtering_and_pagination_return_accurate_counts() {
    let dir = tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("db")).unwrap();
    for i in 0..8 {
        let mut p = post(dir.path());
        p.idempotency_key = Some(format!("post-{i}"));
        p.title = format!("Report {i}");
        store.post(p).unwrap();
    }
    let q = FeedQuery {
        kind: Some("Markdown".into()),
        limit: 3,
        ..Default::default()
    };
    let snapshot = store.snapshot(&q).unwrap();
    assert_eq!(snapshot.total, 8);
    assert_eq!(snapshot.posts.len(), 3);
    assert_eq!(snapshot.unread, 8);
    assert_eq!(
        store
            .snapshot(&FeedQuery {
                kind: Some("Image".into()),
                ..Default::default()
            })
            .unwrap()
            .total,
        0
    );
}

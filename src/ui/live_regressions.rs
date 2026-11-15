//! Exercise delivery through the same event receiver and renderer as the desktop.
use super::{
    tests::{app, post},
    *,
};
use crate::{
    broker::{Paths, bounded_line},
    store::Store,
};
use std::{
    io::Write,
    os::unix::net::{UnixListener, UnixStream},
    sync::{Condvar, Mutex},
    time::Instant,
};

struct Source {
    store: Store,
    fail_snapshot: bool,
    fail_wait: bool,
    silent: bool,
    snapshot_reads: usize,
    wait_after: Vec<i64>,
    publish_during_snapshot: Option<PostInput>,
}

/// Real SQLite + Unix socket + production desktop worker/receiver/renderer.
/// Scripted failures keep these tests independent of the user's service/herdr.
struct Live {
    app: App,
    ctx: egui::Context,
    source: Arc<(Mutex<Source>, Condvar)>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    listener: Option<thread::JoinHandle<()>>,
    client: Client,
    root: tempfile::TempDir,
}
impl Live {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("fdr-live-")
            .tempdir_in("/tmp")
            .unwrap();
        let paths = Paths::new(Some(root.path().into())).unwrap();
        let listener = UnixListener::bind(&paths.socket).unwrap();
        let source = Arc::new((
            Mutex::new(Source {
                store: Store::open(&root.path().join("feed.sqlite")).unwrap(),
                fail_snapshot: false,
                fail_wait: false,
                silent: false,
                snapshot_reads: 0,
                wait_after: Vec::new(),
                publish_during_snapshot: None,
            }),
            Condvar::new(),
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = source.clone();
        let stopping = stop.clone();
        let server = thread::spawn(move || {
            let mut handlers = Vec::new();
            for stream in listener.incoming() {
                if stopping.load(Ordering::Relaxed) {
                    break;
                }
                let shared = shared.clone();
                handlers.push(thread::spawn(move || {
                    let mut stream = stream.unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                    let request: serde_json::Value = serde_json::from_str(&bounded_line(stream.try_clone().unwrap()).unwrap()).unwrap();
                    let result: Result<serde_json::Value> = {
                      let mut state = shared.0.lock().unwrap();
                      (|| {
                        match request["method"].as_str().unwrap() {
                            "snapshot" => {
                                state.snapshot_reads += 1;
                                if std::mem::take(&mut state.fail_snapshot) {
                                    anyhow::bail!("Synthetic snapshot failure");
                                }
                                let snapshot = state.store.snapshot(&serde_json::from_value(request["params"].clone())?)?;
                                if let Some(input) = state.publish_during_snapshot.take() {
                                    state.store.post(input)?;
                                }
                                serde_json::to_value(snapshot).map_err(Into::into)
                            }
                            "wait" => {
                                if std::mem::take(&mut state.fail_wait) { anyhow::bail!("Synthetic reconnect"); }
                                let after = request["params"]["after"].as_i64().unwrap();
                                state.wait_after.push(after);
                                let (guard, _) = shared.1.wait_timeout_while(state, Duration::from_millis(200), |s| s.silent || s.store.sequence().unwrap() <= after).unwrap();
                                state = guard;
                                Ok(json!({"sequence": if state.silent { after } else { state.store.sequence()? }}))
                            }
                            "ping" => Ok(json!({})),
                            "update" => {
                                let post = state.store.update(serde_json::from_value(request["params"].clone())?)?;
                                shared.1.notify_all();
                                Ok(serde_json::to_value(post)?)
                            }
                            other => anyhow::bail!("Unexpected test request: {other}"),
                        }
                      })()
                    };
                    let reply = match result {
                        Ok(value) => json!({"version":1,"ok":true,"result":value}),
                        Err(error) => json!({"version":1,"ok":false,"error":error.to_string()}),
                    };
                    writeln!(stream, "{reply}").unwrap();
                }));
            }
            for handler in handlers {
                handler.join().unwrap();
            }
        });
        Self {
            app: app(),
            ctx: egui::Context::default(),
            source,
            stop,
            worker: None,
            listener: Some(server),
            client: Client::new(paths),
            root,
        }
    }
    fn change<T>(&self, f: impl FnOnce(&mut Source) -> T) -> T {
        let result = f(&mut self.source.0.lock().unwrap());
        self.source.1.notify_all();
        result
    }
    fn publish(&self, name: &str, identity: &str) -> Post {
        self.change(|s| {
            s.store
                .post(PostInput {
                    title: name.into(),
                    body: String::new(),
                    resources: vec![ResourceInput {
                        uri: self.root.path().join(name).to_string_lossy().into(),
                        kind: Some(ResourceKind::File),
                    }],
                    context: Some(Context {
                        cwd: self.root.path().to_string_lossy().into(),
                        native_session_id: Some(identity.into()),
                        ..Context::default()
                    }),
                    session_id: None,
                    idempotency_key: Some(format!("{identity}:{name}")),
                    readiness: "ready".into(),
                })
                .unwrap()
        })
    }
    fn image(&self, name: &str) {
        image::RgbaImage::from_pixel(8, 8, image::Rgba([80, 160, 180, 255]))
            .save(self.root.path().join(name))
            .unwrap();
    }
    fn start(&mut self) {
        let (commands, rx) = mpsc::channel();
        let (tx, events) = mpsc::channel();
        self.app.commands = commands;
        self.app.events = events;
        let queries = self
            .app
            .columns
            .order
            .iter()
            .map(|t| (t.clone(), self.app.feeds[t].query(&self.app.query, t)))
            .collect();
        let (client, stop, ctx) = (self.client.clone(), self.app.stop.clone(), self.ctx.clone());
        self.worker = Some(thread::spawn(move || {
            live::run(client, queries, rx, tx, stop, ctx)
        }));
    }
    fn until(&mut self, check: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            self.app.receive();
            if check(&self.app) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "live feed did not converge: {:?}",
                self.app.error
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn rendered(&mut self, post: &Post) {
        // No click, keyboard input, or filter toggle is sent to this frame.
        let _ = self.ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400., 900.),
                )),
                ..Default::default()
            },
            |ctx| self.app.feed(ctx),
        );
        assert!(
            self.ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new((
                    "dense-resource-row",
                    &post.id,
                    0
                ))))
                .is_some(),
            "new artifact was not rendered"
        );
    }
}
impl Drop for Live {
    fn drop(&mut self) {
        self.app.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
        self.stop.store(true, Ordering::Relaxed);
        let _ = UnixStream::connect(&self.client.paths.socket);
        if let Some(listener) = self.listener.take() {
            listener.join().unwrap();
        }
    }
}

fn contains(app: &App, target: &FeedTarget, id: &str) -> bool {
    app.feeds[target].snapshot.posts.iter().any(|p| p.id == id)
}

#[test]
fn mentioned_and_published_images_reach_matching_columns_and_render_without_input() {
    let mut live = Live::new();
    live.image("old.png");
    let old = live.publish("old.png", "agent-a");
    live.image("other.png");
    let other = live.publish("other.png", "agent-b");
    let a = FeedTarget::Session(old.session_id.clone());
    let b = FeedTarget::Session(other.session_id.clone());
    live.app.open_feed(a.clone());
    live.app.open_feed(b.clone());
    live.app.query.available_only = true;
    live.app.query.kind = Some("Image".into());
    live.change(|s| {
        s.store
            .set_discovery_config(&crate::discovery::Config {
                enabled: true,
                sessions: Some(vec![old.session_id.clone()]),
                ..Default::default()
            })
            .unwrap()
    });
    live.start();
    live.until(|app| contains(app, &a, &old.id) && contains(app, &b, &other.id));
    live.app.select_resource(&old, 0);
    live.image("mentioned.png");
    let discovered = live.change(|s| {
        let result = s
            .store
            .ingest(&crate::discovery::Input {
                session_id: old.session_id.clone(),
                text: "Ready: ./mentioned.png".into(),
                source: "synthetic completed message".into(),
            })
            .unwrap();
        s.store.get(&result.created[0]).unwrap()
    });
    live.until(|app| {
        contains(app, &a, &discovered.id) && contains(app, &FeedTarget::All, &discovered.id)
    });
    assert!(!contains(&live.app, &b, &discovered.id));
    live.rendered(&discovered);
    live.image("published.png");
    let published = live.publish("published.png", "agent-a");
    live.until(|app| {
        contains(app, &a, &published.id) && contains(app, &FeedTarget::All, &published.id)
    });
    assert_eq!(live.app.selected.as_deref(), Some(old.id.as_str()));
    live.rendered(&published);
}

#[test]
fn delayed_files_appear_disappear_and_return_without_a_database_change() {
    let mut live = Live::new();
    let delayed = live.publish("delayed.png", "agent");
    live.app.query.available_only = true;
    live.start();
    // Initial authoritative query has completed, hiding the missing file.
    live.until(|app| !app.snapshot.sessions.is_empty());
    assert!(!contains(&live.app, &FeedTarget::All, &delayed.id));
    let sequence = live.change(|s| s.store.sequence().unwrap());
    live.image("delayed.png");
    live.until(|app| contains(app, &FeedTarget::All, &delayed.id));
    live.rendered(&delayed);
    fs::remove_file(live.root.path().join("delayed.png")).unwrap();
    live.until(|app| !contains(app, &FeedTarget::All, &delayed.id));
    live.image("delayed.png");
    live.until(|app| contains(app, &FeedTarget::All, &delayed.id));
    assert_eq!(live.change(|s| s.store.sequence().unwrap()), sequence);
}

#[test]
fn failed_snapshots_and_waits_recover_without_filter_clicks() {
    let mut live = Live::new();
    let first = live.publish("first.png", "agent");
    live.start();
    live.until(|app| contains(app, &FeedTarget::All, &first.id));
    live.change(|s| s.fail_snapshot = true);
    live.app.commands.send(Command::Refresh).unwrap();
    live.until(|app| {
        app.feed_error
            .as_deref()
            .is_some_and(|e| e.contains("Synthetic snapshot failure"))
    });
    let source = live.source.clone();
    live.until(|_| source.0.lock().unwrap().snapshot_reads >= 3);
    live.until(|app| app.feed_error.is_none());
    live.change(|s| s.fail_wait = true);
    let second = live.publish("second.png", "agent");
    live.until(|app| contains(app, &FeedTarget::All, &second.id));
}

#[test]
fn publication_between_column_queries_uses_the_earliest_snapshot_cursor() {
    let mut live = Live::new();
    let old = live.publish("old.png", "agent");
    let target = FeedTarget::Session(old.session_id.clone());
    live.app.open_feed(target.clone());
    let before = live.change(|s| {
        s.publish_during_snapshot = Some(PostInput {
            title: "between queries".into(),
            body: String::new(),
            resources: vec![ResourceInput {
                uri: live
                    .root
                    .path()
                    .join("between.png")
                    .to_string_lossy()
                    .into(),
                kind: Some(ResourceKind::File),
            }],
            context: Some(old.context.clone()),
            session_id: Some(old.session_id.clone()),
            idempotency_key: Some("between-queries".into()),
            readiness: "ready".into(),
        });
        s.store.sequence().unwrap()
    });
    live.start();
    live.until(|app| {
        app.feeds[&FeedTarget::All].snapshot.total == 2 && app.feeds[&target].snapshot.total == 2
    });
    assert_eq!(live.change(|s| s.wait_after[0]), before);
}

#[test]
fn burst_publications_and_late_attachments_preserve_selection_and_filters() {
    let mut live = Live::new();
    live.image("selected.png");
    let old = live.publish("selected.png", "agent");
    live.app.query.available_only = true;
    live.app.query.kind = Some("Image".into());
    live.start();
    live.until(|app| contains(app, &FeedTarget::All, &old.id));
    live.app.select_resource(&old, 0);
    let mut published = Vec::new();
    for n in 0..48 {
        let name = format!("burst-{n}.png");
        live.image(&name);
        published.push(live.publish(&name, "agent"));
    }
    live.publish("not-an-image.md", "agent");
    live.until(|app| app.feeds[&FeedTarget::All].snapshot.total == 49);
    let ids: HashSet<_> = live.app.feeds[&FeedTarget::All]
        .snapshot
        .posts
        .iter()
        .map(|p| &p.id)
        .collect();
    assert_eq!(ids.len(), 49);
    assert!(published.iter().all(|p| ids.contains(&p.id)));

    live.change(|s| {
        let current = s.store.get(&old.id).unwrap();
        s.store.update(serde_json::from_value(json!({
            "id":old.id, "expected_revision":current.revision,
            "resources":[{"uri":live.root.path().join("late.png")},{"uri":live.root.path().join("selected.png")}]
        })).unwrap()).unwrap();
    });
    live.until(|app| {
        app.snapshot
            .posts
            .iter()
            .any(|p| p.id == old.id && p.revision > old.revision)
    });
    let sequence = live.change(|s| s.store.sequence().unwrap());
    live.image("late.png");
    live.until(|app| {
        app.snapshot
            .posts
            .iter()
            .any(|p| p.id == old.id && p.resources.len() == 2)
    });
    assert_eq!(live.app.selected.as_deref(), Some(old.id.as_str()));
    assert_eq!(live.app.resource, 1);
    assert_eq!(live.change(|s| s.store.sequence().unwrap()), sequence);
}

#[test]
fn sidebar_reloads_an_atomically_replaced_image_with_preserved_modification_time() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("image.png");
    image::RgbaImage::new(8, 8).save(&path).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let mut post = post();
    post.resources[1].target = path.to_string_lossy().into();
    let resource = &post.resources[1];
    let mut app = app();
    let ctx = egui::Context::default();
    app.prepare_preview(&ctx, &post, resource);
    let first = app
        .preview_rx
        .take()
        .unwrap()
        .recv_timeout(Duration::from_secs(3))
        .unwrap();
    assert!(matches!(first, Preview::Image(ref image) if image.size == [8, 8]));
    let key = app.preview_key.clone();
    let replacement = root.path().join("replacement.png");
    image::RgbaImage::new(16, 8).save(&replacement).unwrap();
    fs::File::options()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    fs::rename(replacement, path).unwrap();
    app.prepare_preview(&ctx, &post, resource);
    assert_ne!(app.preview_key, key);
    let second = app
        .preview_rx
        .take()
        .unwrap()
        .recv_timeout(Duration::from_secs(3))
        .unwrap();
    assert!(matches!(second, Preview::Image(ref image) if image.size == [16, 8]));
}

#[test]
fn missed_notifications_self_heal_even_when_unavailable_files_are_shown() {
    let mut live = Live::new();
    let first = live.publish("first.png", "agent");
    live.start();
    live.until(|app| contains(app, &FeedTarget::All, &first.id));
    live.change(|s| s.silent = true);
    let second = live.publish("second.png", "agent");
    live.until(|app| contains(app, &FeedTarget::All, &second.id));
}

#[test]
fn sequence_rollback_and_empty_column_layout_recover() {
    let mut live = Live::new();
    for n in 0..5 {
        live.publish(&format!("old-{n}.png"), "old");
    }
    live.app.close_feed(&FeedTarget::All);
    live.start();
    live.until(|app| !app.snapshot.sessions.is_empty());
    assert!(live.app.snapshot.posts.is_empty());
    live.change(|s| s.store = Store::open(Path::new(":memory:")).unwrap());
    let replacement = live.publish("after-restart.png", "replacement");
    live.until(|app| {
        app.snapshot
            .sessions
            .iter()
            .any(|s| s.id == replacement.session_id)
    });
    live.app.open_feed(FeedTarget::All);
    live.until(|app| contains(app, &FeedTarget::All, &replacement.id));
    assert_eq!(live.app.feeds[&FeedTarget::All].snapshot.total, 1);
}

#[test]
fn selected_image_does_not_hide_new_images_in_any_open_feed() {
    let mut app = app();
    let session = FeedTarget::Session("session".into());
    app.open_feed(session.clone());
    let old = post();
    for feed in app.feeds.values_mut() {
        feed.snapshot.posts = vec![old.clone()];
    }
    app.merge_posts();
    app.select_resource(&old, 1);
    let mut new = old.clone();
    new.id = "new-image".into();
    new.resources = vec![old.resources[1].clone()];
    let (tx, rx) = mpsc::channel();
    app.events = rx;
    for target in [FeedTarget::All, session] {
        tx.send(Event::Snapshot(
            target.clone(),
            app.feeds[&target].query(&app.query, &target),
            Box::new(Snapshot {
                total: 2,
                posts: vec![new.clone(), old.clone()],
                ..Snapshot::default()
            }),
        ))
        .unwrap();
    }
    app.receive();
    for (target, feed) in &app.feeds {
        assert!(
            feed.snapshot.posts.iter().any(|p| p.id == new.id),
            "new image hidden in {target:?}"
        );
    }
    assert_eq!(app.selected.as_deref(), Some(old.id.as_str()));
    assert_eq!(app.resource, 1);
}

#[test]
fn reconnect_status_clears_without_hiding_an_unrelated_action_error() {
    let mut app = app();
    app.error = Some("Could not open file".into());
    let (tx, rx) = mpsc::channel();
    app.events = rx;
    tx.send(Event::FeedStatus(Some("Feed unavailable".into())))
        .unwrap();
    app.receive();
    assert!(app.feed_error.is_some());
    tx.send(Event::FeedStatus(None)).unwrap();
    app.receive();
    assert!(app.feed_error.is_none());
    assert_eq!(app.error.as_deref(), Some("Could not open file"));
}

#[test]
fn an_incoming_image_wakes_an_idle_gui_without_mouse_or_keyboard_input() {
    let mut live = Live::new();
    let old = live.publish("old.png", "agent");
    live.start();
    let source = live.source.clone();
    live.until(|app| {
        contains(app, &FeedTarget::All, &old.id) && !source.0.lock().unwrap().wait_after.is_empty()
    });
    // Let egui finish its initial settling frames before it goes idle.
    for _ in 0..4 {
        let _ = live.ctx.run(egui::RawInput::default(), |_| {});
    }
    let (tx, wakeups) = mpsc::channel();
    live.ctx.set_request_repaint_callback(move |_| {
        let _ = tx.send(());
    });
    let new = live.publish("new.png", "agent");
    wakeups
        .recv_timeout(Duration::from_secs(3))
        .expect("background delivery did not wake the UI");
    live.app.receive();
    assert!(contains(&live.app, &FeedTarget::All, &new.id));
}

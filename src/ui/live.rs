//! Reconcile authoritative snapshots as well as broker change notifications.
use super::{Command, Event, FeedTarget};
use crate::{broker::Client, model::FeedQuery};
use eframe::egui;
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

pub(super) const RECONCILE_INTERVAL: Duration = Duration::from_secs(2);

pub(super) fn run(
    client: Client,
    mut queries: Vec<(FeedTarget, FeedQuery)>,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
    stopping: Arc<AtomicBool>,
    context: egui::Context,
) {
    let mut refresh = true;
    let mut sequence = -1;
    let mut last_refresh = Instant::now();
    while !stopping.load(Ordering::Relaxed) {
        while let Ok(command) = commands.try_recv() {
            let result = match command {
                Command::Call(method, params) => {
                    client.call::<serde_json::Value>(method, params).map(|_| ())
                }
                Command::Query(q) => {
                    queries = q;
                    Ok(())
                }
                Command::Refresh => Ok(()),
                Command::Update(u) => client
                    .call::<crate::model::Post>(
                        "update",
                        serde_json::to_value(u).unwrap_or_default(),
                    )
                    .map(|_| ()),
            };
            if let Err(error) = result {
                let _ = events.send(Event::Error(format!("{error:#}")));
                context.request_repaint();
            }
            refresh = true;
        }
        // Files can appear without a database mutation. A periodic snapshot
        // also heals missed notifications for every filter configuration.
        refresh |= last_refresh.elapsed() >= RECONCILE_INTERVAL;
        if refresh {
            last_refresh = Instant::now();
            let requests = if queries.is_empty() {
                vec![(
                    FeedTarget::All,
                    FeedQuery {
                        limit: 1,
                        ..FeedQuery::default()
                    },
                )]
            } else {
                queries.clone()
            };
            let mut first_sequence = None;
            let mut failed = false;
            for (target, query) in requests {
                match client.snapshot(&query) {
                    Ok(snapshot) => {
                        // Use the earliest successful snapshot: a publication
                        // between column requests must trigger another pass.
                        first_sequence.get_or_insert(snapshot.sequence);
                        if events
                            .send(Event::Snapshot(target, query, Box::new(snapshot)))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        failed = true;
                        let _ = events.send(Event::FeedStatus(Some(format!(
                            "Feed unavailable: {error:#}"
                        ))));
                    }
                }
                // Publish each column immediately, even if the next is slow.
                context.request_repaint();
            }
            if let Some(first) = first_sequence {
                sequence = first;
            }
            refresh = failed;
            if !failed {
                let _ = events.send(Event::FeedStatus(None));
                context.request_repaint();
            }
            if failed {
                // A failed snapshot must retry without requiring a new post or
                // filter click, but never spin against an unavailable broker.
                thread::sleep(Duration::from_millis(200));
            }
        }
        if stopping.load(Ordering::Relaxed) {
            break;
        }
        match client.call::<serde_json::Value>("wait", json!({"after":sequence,"timeout_ms":200})) {
            Ok(value) => refresh |= value["sequence"].as_i64().unwrap_or(-1) != sequence,
            Err(_) => {
                thread::sleep(Duration::from_millis(500));
                let _ = client.ensure_running();
                refresh = true;
            }
        }
    }
}

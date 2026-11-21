//! Service changes and GUI requests independently wake the feed worker.
use super::{Command, Event, FeedTarget};
use crate::{
    broker::Client,
    model::{FeedQuery, Snapshot},
    store::desktop::{Batch, BatchQuery, ColumnQuery, MAX_COLUMNS},
};
use anyhow::{Result, ensure};
use eframe::egui;
use serde::Deserialize;
use serde_json::{Value, json};
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
const RETRY_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct Cursor {
    sequence: i64,
    #[serde(default)]
    desktop_generation: u64,
}

fn watch(
    client: Client,
    mut cursor: Cursor,
    wake: mpsc::Sender<Command>,
    stopping: Arc<AtomicBool>,
) {
    while !stopping.load(Ordering::Relaxed) {
        match client.call::<Cursor>(
            "wait",
            json!({"after":cursor.sequence,"desktop_after":cursor.desktop_generation,"timeout_ms":1000}),
        ) {
            Ok(next) if next != cursor => {
                cursor = next;
                if wake.send(Command::Changed(next)).is_err() {
                    return;
                }
            }
            Ok(_) => (),
            Err(_) => {
                thread::sleep(Duration::from_millis(500));
                if stopping.load(Ordering::Relaxed) {
                    return;
                }
                let _ = client.ensure_running();
                if wake.send(Command::Refresh).is_err() {
                    return;
                }
            }
        }
    }
}

pub(super) fn run(
    client: Client,
    mut queries: Vec<(FeedTarget, FeedQuery)>,
    wake: mpsc::Sender<Command>,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
    stopping: Arc<AtomicBool>,
    context: egui::Context,
) {
    let mut watch_wake = Some(wake);
    let mut pending = None;
    let mut cursor = Cursor {
        sequence: -1,
        desktop_generation: 0,
    };
    let mut refresh = true;
    let mut refresh_at = Instant::now();
    let mut supports_batch = true;
    while !stopping.load(Ordering::Relaxed) {
        for command in pending.take().into_iter().chain(commands.try_iter()) {
            let result = match command {
                Command::Changed(next) => {
                    refresh |= next != cursor;
                    continue;
                }
                Command::Call(method, params) => client.call::<Value>(method, params).map(|_| ()),
                Command::Query(new_queries) => {
                    queries = new_queries;
                    Ok(())
                }
                Command::Refresh => {
                    supports_batch = true;
                    Ok(())
                }
                Command::Update(update) => client
                    .call::<crate::model::Post>(
                        "update",
                        serde_json::to_value(update).unwrap_or_default(),
                    )
                    .map(|_| ()),
            };
            if let Err(error) = result {
                let _ = events.send(Event::Error(format!("{error:#}")));
                context.request_repaint();
            }
            refresh = true;
        }
        if refresh || Instant::now() >= refresh_at {
            refresh = false;
            match fetch(&client, &queries, &events, &context, &mut supports_batch) {
                Ok(next) => {
                    cursor = next;
                    let _ = events.send(Event::FeedStatus(None));
                    refresh_at = Instant::now() + RECONCILE_INTERVAL;
                }
                Err(error) => {
                    let _ = events.send(Event::FeedStatus(Some(format!(
                        "Feed unavailable: {error:#}"
                    ))));
                    refresh_at = Instant::now() + RETRY_INTERVAL;
                }
            }
            context.request_repaint();
            if let Some(wake) = watch_wake.take() {
                let client = client.clone();
                let stopping = stopping.clone();
                thread::spawn(move || watch(client, cursor, wake, stopping));
            }
        }
        let remaining = refresh_at.saturating_duration_since(Instant::now());
        match commands.recv_timeout(remaining.min(RETRY_INTERVAL)) {
            Ok(command) => pending = Some(command),
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn fetch(
    client: &Client,
    queries: &[(FeedTarget, FeedQuery)],
    events: &mpsc::Sender<Event>,
    context: &egui::Context,
    supports_batch: &mut bool,
) -> Result<Cursor> {
    if *supports_batch && queries.len() <= MAX_COLUMNS {
        let query = BatchQuery {
            filters: queries
                .first()
                .map(|(_, query)| query.clone())
                .unwrap_or_default(),
            columns: queries
                .iter()
                .map(|(_, query)| ColumnQuery {
                    session_id: query.session_id.clone(),
                    limit: query.limit,
                })
                .collect(),
        };
        match client.call::<Batch>("desktop.snapshot", serde_json::to_value(query)?) {
            Ok(batch) => {
                ensure!(
                    batch.pages.len() == queries.len(),
                    "Feed service returned the wrong number of columns"
                );
                let cursor = Cursor {
                    sequence: batch.snapshot.sequence,
                    desktop_generation: batch.generation,
                };
                events.send(Event::Batch(Box::new(batch), queries.to_vec()))?;
                return Ok(cursor);
            }
            Err(error) if error.to_string() == "Unknown method desktop.snapshot" => {
                *supports_batch = false;
            }
            Err(error) if error.to_string() == "Feed batch exceeds response limit" => (),
            Err(error) => return Err(error),
        }
    }
    // Older services and oversized batches still support independent snapshots.
    let fallback = [(
        FeedTarget::All,
        FeedQuery {
            limit: 1,
            ..FeedQuery::default()
        },
    )];
    let requests = if queries.is_empty() {
        &fallback
    } else {
        queries
    };
    let mut first = None;
    let mut failed = None;
    for (target, query) in requests {
        match client.snapshot(query) {
            Ok(snapshot) => {
                first.get_or_insert(snapshot.sequence);
                events.send(Event::Snapshot(
                    target.clone(),
                    query.clone(),
                    Box::new(snapshot),
                ))?;
                context.request_repaint();
            }
            Err(error) => failed = Some(error),
        }
    }
    if let Some(error) = failed {
        return Err(error);
    }
    Ok(Cursor {
        sequence: first.unwrap_or(-1),
        desktop_generation: 0,
    })
}

// Materialize each page from the single copy of each post sent by the broker.
pub(super) fn expand_batch(
    batch: Batch,
    queries: Vec<(FeedTarget, FeedQuery)>,
) -> impl Iterator<Item = (FeedTarget, FeedQuery, Snapshot)> {
    let Batch {
        snapshot, pages, ..
    } = batch;
    let sequence = snapshot.sequence;
    let posts = snapshot.posts;
    queries
        .into_iter()
        .zip(pages)
        .map(move |((target, query), page)| {
            let snapshot = Snapshot {
                posts: page
                    .posts
                    .into_iter()
                    .filter_map(|index| posts.get(index).cloned())
                    .collect(),
                total: page.total,
                unread: page.unread,
                sequence,
                ..Snapshot::default()
            };
            (target, query, snapshot)
        })
}

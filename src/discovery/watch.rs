use super::{Config, Input, LogSource, log_messages};
use crate::{
    broker::Shared,
    herdr,
    model::{Post, Session},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    thread,
    time::{Duration, Instant, SystemTime},
};

#[derive(Default, Clone, Serialize, Deserialize)]
pub(crate) struct Cursor {
    offset: u64,
    identity: String,
    discard: bool,
}
pub(crate) fn cursor_key(source: &LogSource) -> String {
    format!(
        "discovery.cursor.{:x}",
        Sha256::digest(format!(
            "{}:{}:{}",
            source.session_id, source.path, source.format
        ))
    )
}
pub(crate) fn initial_cursor(source: &LogSource) -> Cursor {
    if source.from_start {
        return Cursor::default();
    }
    fs::metadata(&source.path)
        .map(|metadata| {
            let mut last = *b"\n";
            if metadata.len() > 0
                && let Ok(mut file) = fs::File::open(&source.path)
            {
                let _ = file.seek(SeekFrom::End(-1));
                let _ = file.read_exact(&mut last);
            }
            Cursor {
                identity: format!("{}:{}", metadata.dev(), metadata.ino()),
                offset: metadata.len(),
                discard: last[0] != b'\n',
            }
        })
        .unwrap_or_default()
}
pub(crate) fn read_log(source: &LogSource, previous: &Cursor) -> Result<(Vec<String>, Cursor)> {
    let mut file = fs::File::open(&source.path)?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "Watched log must be a regular file");
    let identity = format!("{}:{}", metadata.dev(), metadata.ino());
    let mut cursor = if identity != previous.identity || metadata.len() < previous.offset {
        Cursor {
            identity,
            ..Cursor::default()
        }
    } else {
        previous.clone()
    };
    file.seek(SeekFrom::Start(cursor.offset))?;
    let mut bytes = Vec::new();
    file.take(256 * 1024).read_to_end(&mut bytes)?;
    let Some(end) = bytes.iter().rposition(|byte| *byte == b'\n').map(|n| n + 1) else {
        if bytes.len() == 256 * 1024 || cursor.discard {
            cursor.offset += bytes.len() as u64;
            cursor.discard = true;
        }
        return Ok((vec![], cursor));
    };
    let start = if cursor.discard {
        bytes.iter().position(|byte| *byte == b'\n').unwrap() + 1
    } else {
        0
    };
    cursor.discard = false;
    cursor.offset += end as u64;
    let text = String::from_utf8_lossy(&bytes[start..end]);
    let messages = text
        .lines()
        .flat_map(|line| log_messages(line, &source.format))
        .collect();
    Ok((messages, cursor))
}

pub fn start(shared: Shared) {
    thread::spawn(move || {
        let mut seen = HashMap::<String, String>::new();
        let mut stable = HashMap::<String, (u64, Option<SystemTime>, Instant)>::new();
        let mut retries = HashMap::<String, u8>::new();
        loop {
            let errors = tick(&shared, &mut seen, &mut stable, &mut retries)
                .err()
                .map(|error| format!("{error:#}"));
            if let Ok(mut state) = shared.0.lock()
                && state.discovery_error != errors
            {
                state.discovery_error = errors;
                let _ = state.store.touch();
                shared.1.notify_all();
            }
            thread::sleep(Duration::from_secs(2));
        }
    });
}
fn tick(
    shared: &Shared,
    seen: &mut HashMap<String, String>,
    stable: &mut HashMap<String, (u64, Option<SystemTime>, Instant)>,
    retries: &mut HashMap<String, u8>,
) -> Result<()> {
    let (config, sessions, pending) = {
        let state = shared
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Discovery lock failed"))?;
        (
            state.store.discovery_config()?,
            state.store.sessions()?,
            state.store.pending_mentions()?,
        )
    };
    if !config.enabled || config.file_types.is_empty() {
        seen.clear();
        stable.clear();
        retries.clear();
        return Ok(());
    }
    let mut errors = Vec::new();
    for session in sessions.iter().filter(|session| {
        session.online
            && session.context.agent != "Shell"
            && session.status != "working"
            && config.includes(&session.id)
    }) {
        match herdr::pane_text(session) {
            Ok(text) => {
                let hash = format!(
                    "{:x}",
                    Sha256::digest(format!(
                        "{}\0{text}",
                        serde_json::to_string(&(&config.exclude_paths, &config.file_types))?
                    ))
                );
                if seen.get(&session.id) == Some(&hash) {
                    retries.remove(&session.id);
                    continue;
                }
                if let Some(socket) = &session.context.herdr_socket
                    && let Err(error) = herdr::refresh(shared, std::path::Path::new(socket))
                {
                    record_error(session, &error, retries, &mut errors);
                    continue;
                }
                retries.remove(&session.id);
                let mut state = shared
                    .0
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Discovery lock failed"))?;
                let current = state
                    .store
                    .sessions()?
                    .into_iter()
                    .find(|current| current.id == session.id);
                let same_occupant = current.is_some_and(|current| {
                    current.online
                        && current.terminal_id == session.terminal_id
                        && current.context.pane_id == session.context.pane_id
                        && current.context.native_session_id == session.context.native_session_id
                        && current.state_change_seq == session.state_change_seq
                        && current.status != "working"
                });
                if same_occupant && state.store.discovery_config()?.includes(&session.id) {
                    state.store.ingest(&Input {
                        session_id: session.id.clone(),
                        text,
                        source: "herdr pane".into(),
                    })?;
                    seen.insert(session.id.clone(), hash);
                    shared.1.notify_all();
                }
            }
            Err(error) => record_error(session, &error, retries, &mut errors),
        }
    }
    seen.retain(|id, _| sessions.iter().any(|session| session.id == *id));
    retries.retain(|id, _| sessions.iter().any(|session| session.id == *id));
    for source in &config.logs {
        if !config.includes(&source.session_id) {
            continue;
        }
        if let Err(error) = scan_log(shared, source) {
            errors.push(format!("{}: {error}", source.path));
        }
    }
    stable.retain(|id, _| pending.iter().any(|post| post.id == *id));
    for post in pending {
        settle(shared, &config, post, stable)?;
    }
    ensure!(
        errors.is_empty(),
        "{}",
        errors.join("\n").chars().take(1800).collect::<String>()
    );
    Ok(())
}
fn transient(error: &anyhow::Error) -> bool {
    error.chain().any(|source| {
        source
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| {
                matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                )
            })
    })
}
fn record_error(
    session: &Session,
    error: &anyhow::Error,
    retries: &mut HashMap<String, u8>,
    errors: &mut Vec<String>,
) {
    if transient(error) {
        let count = retries.entry(session.id.clone()).or_default();
        *count = count.saturating_add(1);
        if *count >= 3 {
            errors.push(format!(
                "{}: Herdr is taking longer to respond. Retrying automatically.",
                session.context.label
            ));
        }
    } else {
        errors.push(format!("{}: {error}", session.context.label));
    }
}
fn scan_log(shared: &Shared, source: &LogSource) -> Result<()> {
    let key = cursor_key(source);
    let cursor = shared
        .0
        .lock()
        .map_err(|_| anyhow::anyhow!("Discovery lock failed"))?
        .store
        .setting::<Cursor>(&key)?;
    let (messages, cursor) = read_log(source, &cursor)?;
    let mut state = shared
        .0
        .lock()
        .map_err(|_| anyhow::anyhow!("Discovery lock failed"))?;
    let current = state.store.discovery_config()?;
    if !current.includes(&source.session_id) || !current.logs.contains(source) {
        return Ok(());
    }
    // Commit cursor only after every completed message was durably ingested.
    // A restart between these writes replays harmlessly through mention keys.
    for text in messages {
        if text.len() > 128 * 1024 {
            continue;
        }
        state.store.ingest(&Input {
            session_id: source.session_id.clone(),
            text,
            source: format!("{} log: {}", source.format, source.path),
        })?;
    }
    state.store.set_setting(&key, &cursor)?;
    shared.1.notify_all();
    Ok(())
}
fn settle(
    shared: &Shared,
    config: &Config,
    post: Post,
    stable: &mut HashMap<String, (u64, Option<SystemTime>, Instant)>,
) -> Result<()> {
    if !config.includes(&post.session_id)
        || !post.context.is_local()
        || post
            .discovery
            .as_ref()
            .is_some_and(|source| source.unresolved)
    {
        return Ok(());
    }
    let Some(resource) = post.resources.first() else {
        return Ok(());
    };
    let Ok(metadata) = fs::metadata(&resource.target) else {
        stable.remove(&post.id);
        return Ok(());
    };
    let stamp = (metadata.len(), metadata.modified().ok());
    let entry = stable
        .entry(post.id.clone())
        .or_insert((stamp.0, stamp.1, Instant::now()));
    if (entry.0, entry.1) != stamp {
        *entry = (stamp.0, stamp.1, Instant::now());
    }
    if entry.2.elapsed() >= Duration::from_millis(config.settle_ms) {
        shared
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Discovery lock failed"))?
            .store
            .settle_mention(&post.id)?;
        shared.1.notify_all();
        stable.remove(&post.id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn temporary_io_errors_retry_without_becoming_discovery_errors() {
        for kind in [
            std::io::ErrorKind::WouldBlock,
            std::io::ErrorKind::TimedOut,
            std::io::ErrorKind::Interrupted,
        ] {
            let error = anyhow::Error::new(std::io::Error::from(kind)).context("Pane read");
            assert!(transient(&error));
        }
        assert!(!transient(&anyhow::Error::new(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied
        ))));
        assert!(!transient(&anyhow::anyhow!("Unsupported response")));
        let session = crate::herdr::sessions_from_snapshot(
            "/fake/socket",
            &serde_json::json!({"panes":[{"pane_id":"p","agent":"codex"}]}),
            &[],
        )
        .pop()
        .unwrap();
        let error = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::WouldBlock));
        let mut retries = HashMap::new();
        let mut errors = Vec::new();
        record_error(&session, &error, &mut retries, &mut errors);
        record_error(&session, &error, &mut retries, &mut errors);
        assert!(errors.is_empty());
        record_error(&session, &error, &mut retries, &mut errors);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Retrying automatically"));
        assert!(!errors[0].contains("os error"));
    }

    fn source(path: &std::path::Path) -> LogSource {
        LogSource {
            path: path.to_string_lossy().into(),
            session_id: "session".into(),
            format: "plain".into(),
            from_start: false,
        }
    }
    #[test]
    fn pane_reads_are_visible_only_and_replacements_cannot_inherit_mentions() {
        use crate::{broker::State, herdr::sessions_from_snapshot, store::Store};
        use std::{
            os::unix::net::UnixListener,
            sync::{Arc, Condvar, Mutex},
        };
        let dir = tempfile::Builder::new()
            .prefix("fdr-")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let initial = serde_json::json!({"workspaces":[{"workspace_id":"w","label":"Project"}],"panes":[{"pane_id":"w:p","workspace_id":"w","terminal_id":"terminal","agent":"codex","agent_status":"idle","agent_session":{"value":"old-agent"},"cwd":dir.path()}],"agents":[{"pane_id":"w:p","agent_status":"idle","state_change_seq":1}]});
        let session = sessions_from_snapshot(socket.to_str().unwrap(), &initial, &[])
            .pop()
            .unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        store
            .reconcile(socket.to_str().unwrap(), vec![session.clone()])
            .unwrap();
        store
            .set_discovery_config(&Config {
                enabled: true,
                sessions: Some(vec![session.id.clone()]),
                ..Config::default()
            })
            .unwrap();
        let shared = Arc::new((
            Mutex::new(State {
                store,
                connections: HashMap::new(),
                discovery_error: None,
            }),
            Condvar::new(),
        ));
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request: serde_json::Value = serde_json::from_str(
                &crate::broker::bounded_line(stream.try_clone().unwrap()).unwrap(),
            )
            .unwrap();
            assert_eq!(request["method"], "pane.read");
            assert_eq!(request["params"]["format"], "text");
            assert_eq!(request["params"]["source"], "visible");
            assert!(request["params"].get("lines").is_none());
            assert_eq!(request["params"]["strip_ansi"], true);
            writeln!(stream,"{}",serde_json::json!({"result":{"read":{"text":"New occupant says ./replacement-report.pdf"}}})).unwrap();
            let (mut stream, _) = listener.accept().unwrap();
            let request: serde_json::Value = serde_json::from_str(
                &crate::broker::bounded_line(stream.try_clone().unwrap()).unwrap(),
            )
            .unwrap();
            assert_eq!(request["method"], "session.snapshot");
            let mut replacement = initial;
            replacement["panes"][0]["agent_session"]["value"] = "replacement-agent".into();
            writeln!(
                stream,
                "{}",
                serde_json::json!({"result":{"snapshot":replacement}})
            )
            .unwrap();
        });
        tick(
            &shared,
            &mut HashMap::new(),
            &mut HashMap::new(),
            &mut HashMap::new(),
        )
        .unwrap();
        server.join().unwrap();
        let state = shared.0.lock().unwrap();
        assert_eq!(
            state
                .store
                .snapshot(&crate::model::FeedQuery::default())
                .unwrap()
                .total,
            0
        );
        assert!(
            !state
                .store
                .sessions()
                .unwrap()
                .iter()
                .find(|current| current.id == session.id)
                .unwrap()
                .online
        );
    }
    #[test]
    fn incomplete_lines_wait_for_completion_and_cursor_survives_serialization() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.log");
        fs::write(&path, "Ready: ./first.md\nPartial: ./sec").unwrap();
        let log = source(&path);
        let (first, cursor) = read_log(&log, &Cursor::default()).unwrap();
        assert_eq!(first, ["Ready: ./first.md"]);
        let cursor: Cursor =
            serde_json::from_str(&serde_json::to_string(&cursor).unwrap()).unwrap();
        let (empty, cursor) = read_log(&log, &cursor).unwrap();
        assert!(empty.is_empty());
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"ond.pdf\n")
            .unwrap();
        let (complete, _) = read_log(&log, &cursor).unwrap();
        assert_eq!(complete, ["Partial: ./second.pdf"]);
    }
    #[test]
    fn new_log_watch_starts_at_end_and_does_not_drop_the_next_complete_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.log");
        fs::write(&path, "Old: ./old.md\n").unwrap();
        let log = source(&path);
        let cursor = initial_cursor(&log);
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"New: ./new.md\n")
            .unwrap();
        let (messages, _) = read_log(&log, &cursor).unwrap();
        assert_eq!(messages, ["New: ./new.md"]);
    }
    #[test]
    fn rotation_truncation_and_oversized_lines_do_not_stall_log_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.log");
        fs::write(&path, "Initial ./a.md\n").unwrap();
        let log = source(&path);
        let (_, cursor) = read_log(&log, &Cursor::default()).unwrap();
        fs::rename(&path, dir.path().join("old.log")).unwrap();
        fs::write(&path, "Rotated ./b.md\n").unwrap();
        let (rotated, cursor) = read_log(&log, &cursor).unwrap();
        assert_eq!(rotated, ["Rotated ./b.md"]);
        fs::write(&path, "./c.md\n").unwrap();
        let (truncated, _) = read_log(&log, &cursor).unwrap();
        assert_eq!(truncated, ["./c.md"]);
        fs::write(&path, vec![b'x'; 256 * 1024 + 100]).unwrap();
        let (_, cursor) = read_log(&log, &Cursor::default()).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"\nAfter ./d.md\n")
            .unwrap();
        let (after, _) = read_log(&log, &cursor).unwrap();
        assert_eq!(after, ["After ./d.md"]);
    }
}

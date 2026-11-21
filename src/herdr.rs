//! Herdr lifecycle events invalidate our session cache. With Herdr 0.9.1 and
//! later, live focus events can also identify the most recent client's pane.
use crate::{
    broker::{Shared, State, bounded_line},
    model::*,
};
use anyhow::{Context as _, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[derive(PartialEq, Eq)]
pub(crate) struct Focus {
    socket: String,
    pane_id: String,
}

impl Focus {
    fn new(socket: &str, pane_id: &str) -> Self {
        Self {
            socket: socket.into(),
            pane_id: pane_id.into(),
        }
    }

    pub(crate) fn session_id<'a>(&self, sessions: &'a [Session]) -> Option<&'a str> {
        sessions
            .iter()
            .find(|s| {
                s.online
                    && s.context.herdr_socket.as_deref() == Some(self.socket.as_str())
                    && s.context.pane_id.as_deref() == Some(self.pane_id.as_str())
            })
            .map(|s| s.id.as_str())
    }
}

fn set_focus(state: &mut State, focus: Option<Focus>) -> Result<()> {
    if state.herdr_focus != focus {
        state.store.touch()?;
        state.herdr_focus = focus;
    }
    Ok(())
}

struct FocusWatch {
    live_events: bool,
    snapshot_pane: Option<String>,
}

fn fingerprint(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))[..24].into()
}
pub fn discover(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os("HERDR_SOCKET_PATH") {
        paths.push(path.into());
    }
    if let Some(home) = dirs::home_dir() {
        let config = home.join(".config/herdr");
        paths.push(config.join("herdr.sock"));
        if let Ok(entries) = std::fs::read_dir(config.join("sessions")) {
            for e in entries.flatten() {
                paths.push(e.path().join("herdr.sock"));
            }
        }
    }
    if let Ok(bytes) = std::fs::read(root.join("herdr-sockets.json"))
        && let Ok(config) = serde_json::from_slice::<Vec<PathBuf>>(&bytes)
    {
        paths.extend(config);
    }
    paths.retain(|p| p.is_absolute());
    paths.sort();
    paths.dedup();
    paths
}
pub fn start(shared: Shared, root: PathBuf) {
    thread::spawn(move || {
        let mut workers = HashSet::new();
        loop {
            for socket in discover(&root) {
                if workers.insert(socket.clone()) {
                    let state = shared.clone();
                    thread::spawn(move || watch(state, socket));
                }
            }
            thread::sleep(Duration::from_secs(2));
        }
    });
}
fn status(shared: &Shared, socket: &str, connected: bool, message: String) {
    if let Ok(mut state) = shared.0.lock() {
        let changed = state
            .connections
            .get(socket)
            .is_none_or(|s| s.connected != connected || s.message != message);
        state.connections.insert(
            socket.into(),
            ConnectionStatus {
                socket: socket.into(),
                connected,
                message,
            },
        );
        if changed {
            let _ = state.store.touch();
        }
        if !connected {
            if state
                .herdr_focus
                .as_ref()
                .is_some_and(|f| f.socket == socket)
            {
                let _ = set_focus(&mut state, None);
            }
            let _ = state.store.reconcile(socket, vec![]);
        }
    }
    shared.1.notify_all();
}
fn watch(shared: Shared, socket: PathBuf) {
    let key = socket.to_string_lossy().into_owned();
    loop {
        match subscribed(&shared, &socket) {
            Ok(()) => {}
            Err(e) => status(&shared, &key, false, format!("{e:#}")),
        }
        thread::sleep(Duration::from_secs(2));
    }
}
fn request(socket: &Path, method: &str) -> Result<Value> {
    request_params(socket, method, json!({}))
}
fn request_params(socket: &Path, method: &str, params: Value) -> Result<Value> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    writeln!(
        stream,
        "{}",
        json!({"id":"feedr","method":method,"params":params})
    )?;
    let reply: Value = serde_json::from_str(&bounded_line(stream)?)?;
    ensure!(reply.get("error").is_none(), "Herdr: {}", reply["error"]);
    Ok(reply)
}
pub(crate) fn pane_text(session: &Session) -> Result<String> {
    let socket = session
        .context
        .herdr_socket
        .as_deref()
        .context("Session has no herdr socket")?;
    let pane = session
        .context
        .pane_id
        .as_deref()
        .context("Session has no pane")?;
    let reply = request_params(
        Path::new(socket),
        "pane.read",
        // Recent-history reads can make herdr scroll alternate-screen agents
        // to harvest their private history. Background discovery must never
        // request that traversal or fall back to it when a read fails.
        json!({"pane_id":pane,"source":"visible","format":"text","strip_ansi":true}),
    )?;
    reply["result"]["read"]["text"]
        .as_str()
        .or_else(|| reply["result"]["text"].as_str())
        .map(|text| {
            let mut end = text.len().min(64 * 1024);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text[..end].to_string()
        })
        .context("Herdr returned no readable pane text")
}

pub(crate) fn refresh(shared: &Shared, socket: &Path) -> Result<()> {
    refresh_sessions(shared, socket, None)
}

fn refresh_sessions(
    shared: &Shared,
    socket: &Path,
    focus_watch: Option<&mut Option<FocusWatch>>,
) -> Result<()> {
    let reply = request(socket, "session.snapshot")?;
    let snapshot = &reply["result"]["snapshot"];
    ensure!(
        snapshot["panes"].is_array(),
        "Herdr does not provide session.snapshot"
    );
    let key = socket.to_string_lossy().into_owned();
    let mut state = shared
        .0
        .lock()
        .map_err(|_| anyhow::anyhow!("State lock failed"))?;
    let sessions = sessions_from_snapshot(&key, snapshot, &state.store.sessions()?);
    if let Some(focus_watch) = focus_watch {
        let snapshot_pane = snapshot["focused_pane_id"].as_str().filter(|pane| {
            sessions
                .iter()
                .any(|s| s.context.pane_id.as_deref() == Some(*pane))
        });
        let watch = focus_watch.get_or_insert_with(|| FocusWatch {
            // Before 0.9, subscriptions replay retained focus events. 0.9.1
            // restored focus notifications from independent client views.
            live_events: snapshot["version"]
                .as_str()
                .and_then(|version| semver::Version::parse(version).ok())
                .is_some_and(|version| version >= semver::Version::new(0, 9, 1)),
            snapshot_pane: snapshot_pane.map(str::to_owned),
        });
        let own_focus = state
            .herdr_focus
            .as_ref()
            .filter(|focus| focus.socket == key);
        let default_changed = !watch.live_events && watch.snapshot_pane.as_deref() != snapshot_pane;
        if state.herdr_focus.is_none()
            || own_focus.is_some_and(|focus| focus.session_id(&sessions).is_none())
            || (default_changed && (snapshot_pane.is_some() || own_focus.is_some()))
        {
            set_focus(&mut state, snapshot_pane.map(|pane| Focus::new(&key, pane)))?;
        }
        watch.snapshot_pane = snapshot_pane.map(str::to_owned);
    }
    state.store.reconcile(&key, sessions)?;
    drop(state);
    status(
        shared,
        &key,
        true,
        format!(
            "herdr {} · protocol {}",
            snapshot["version"].as_str().unwrap_or("unknown"),
            snapshot["protocol"]
        ),
    );
    shared.1.notify_all();
    Ok(())
}
fn subscribed(shared: &Shared, socket: &Path) -> Result<()> {
    let key = socket.to_string_lossy();
    let mut stream = UnixStream::connect(socket)
        .with_context(|| format!("Herdr unavailable at {}", socket.display()))?;
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let events = [
        "workspace.created",
        "workspace.renamed",
        "workspace.closed",
        "pane.created",
        "pane.updated",
        "pane.focused",
        "pane.closed",
        "pane.exited",
        "pane.agent_detected",
    ];
    writeln!(
        stream,
        "{}",
        json!({"id":"feedr-events","method":"events.subscribe","params":{"subscriptions":events.map(|s|json!({"type":s}))}})
    )?;
    let mut reader = BufReader::new(stream);
    let mut pending = Vec::new();
    // Subscribe first, wait for acknowledgment, then read a fresh snapshot.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut acknowledged = false;
    while Instant::now() < deadline {
        if let Some(v) = read_event(&mut reader, &mut pending)? {
            ensure!(
                v.get("error").is_none(),
                "Herdr rejected subscriptions: {}",
                v["error"]
            );
            acknowledged = true;
            break;
        }
    }
    ensure!(acknowledged, "Herdr subscription timed out");
    let mut focus_watch = None;
    refresh_sessions(shared, socket, Some(&mut focus_watch))?;
    let mut last = Instant::now();
    let mut dirty = false;
    loop {
        if let Some(event) = read_event(&mut reader, &mut pending)? {
            ensure!(
                event.get("error").is_none(),
                "Herdr subscription lost: {}",
                event["error"]
            );
            if focus_watch.as_ref().is_some_and(|watch| watch.live_events)
                && event["event"] == "pane_focused"
                && let Some(pane) = event["data"]["pane_id"].as_str()
            {
                let mut state = shared
                    .0
                    .lock()
                    .map_err(|_| anyhow::anyhow!("State lock failed"))?;
                set_focus(&mut state, Some(Focus::new(&key, pane)))?;
                drop(state);
                shared.1.notify_all();
            }
            dirty = true;
        }
        if (dirty && last.elapsed() > Duration::from_millis(120))
            || last.elapsed() > Duration::from_secs(2)
        {
            refresh_sessions(shared, socket, Some(&mut focus_watch))?;
            last = Instant::now();
            dirty = false;
        }
    }
}
fn read_event(reader: &mut BufReader<UnixStream>, pending: &mut Vec<u8>) -> Result<Option<Value>> {
    match reader.read_until(b'\n', pending) {
        Ok(0) => anyhow::bail!("Herdr disconnected"),
        Ok(_) => {
            ensure!(pending.len() < 1_048_576, "Oversized Herdr event");
            if pending.last() == Some(&b'\n') {
                let v = serde_json::from_slice(pending)?;
                pending.clear();
                Ok(Some(v))
            } else {
                Ok(None)
            }
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}
pub fn sessions_from_snapshot(socket: &str, snapshot: &Value, old: &[Session]) -> Vec<Session> {
    let mut workspaces = HashMap::new();
    if let Some(ws) = snapshot["workspaces"].as_array() {
        for (index, w) in ws.iter().enumerate() {
            workspaces.insert(
                w["workspace_id"].as_str().unwrap_or(""),
                (w["label"].as_str().unwrap_or("Workspace"), index),
            );
        }
    }
    let text = |v: &Value, k: &str| v[k].as_str().map(str::to_owned);
    let agents: HashMap<_, _> = snapshot["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|agent| Some((agent["pane_id"].as_str()?, agent)))
        .collect();
    let tabs: Vec<_> = snapshot["tabs"].as_array().into_iter().flatten().collect();
    snapshot["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(pane_order, p)| {
            let agent = text(p, "agent").unwrap_or_else(|| "Shell".into());
            let pane = text(p, "pane_id")?;
            let terminal = text(p, "terminal_id").unwrap_or_else(|| pane.clone());
            let native = text(&p["agent_session"], "value");
            let resumed = native.as_ref().and_then(|native| {
                old.iter().find(|s| {
                    s.context.herdr_socket.as_deref() == Some(socket)
                        && s.context.agent == agent
                        && s.context.native_session_id.as_ref() == Some(native)
                })
            });
            let previous = resumed.or_else(|| {
                old.iter().find(|s| {
                    s.context.herdr_socket.as_deref() == Some(socket)
                        && s.terminal_id == terminal
                        && s.context.agent == agent
                        && s.online
                        && native.as_ref().is_none_or(|n| {
                            s.context.native_session_id.as_ref().is_none_or(|o| o == n)
                        })
                })
            });
            let native =
                native.or_else(|| previous.and_then(|s| s.context.native_session_id.clone()));
            let identity = native.clone().unwrap_or_else(|| terminal.clone());
            let id = previous.map(|s| s.id.clone()).unwrap_or_else(|| {
                format!(
                    "herdr-{}",
                    fingerprint(&format!("{socket}:{agent}:{identity}"))
                )
            });
            let workspace = text(p, "workspace_id");
            let workspace_info = workspace.as_deref().and_then(|w| workspaces.get(w));
            let label = workspace_info
                .map_or("Workspace", |(label, _)| label)
                .to_string();
            let label = text(p, "title")
                .or_else(|| text(p, "terminal_title_stripped"))
                .filter(|s| !s.is_empty())
                .unwrap_or(label);
            let agent_info = agents.get(pane.as_str());
            let state_change_seq = agent_info
                .and_then(|agent| agent["state_change_seq"].as_u64())
                .unwrap_or_default();
            let status = agent_info
                .and_then(|agent| text(agent, "agent_status"))
                .or_else(|| text(p, "agent_status"))
                .unwrap_or_else(|| "unknown".into());
            let tab = tabs.iter().find(|t| t["tab_id"] == p["tab_id"]);
            let display_name = workspace_info.map(|(name, _)| {
                let tab = tab.filter(|t| {
                    t["custom_label"].as_bool().unwrap_or_else(|| {
                        t["number"].as_u64().is_some_and(|number| {
                            t["label"]
                                .as_str()
                                .is_some_and(|label| label != number.to_string())
                        })
                    }) || tabs
                        .iter()
                        .filter(|other| other["workspace_id"] == p["workspace_id"])
                        .count()
                        > 1
                });
                match tab
                    .and_then(|t| t["label"].as_str())
                    .filter(|s| !s.trim().is_empty())
                {
                    Some(tab) => format!("{name} · {tab}"),
                    None => (*name).to_string(),
                }
            });
            let display_agent = agent_info.and_then(|info| {
                ["display_agent", "name", "agent"]
                    .into_iter()
                    .find_map(|field| text(info, field).filter(|s| !s.trim().is_empty()))
            });
            // Terminal-only identities are deterministic too. On broker restart
            // they are initially offline, so identity matching above may not
            // select a predecessor even though the same durable ID exists.
            let activity_previous = previous.or_else(|| old.iter().find(|s| s.id == id));
            let last_activity_at = activity_previous.and_then(|old| {
                let changed = (old.status != "offline" && old.status != status)
                    || (old.state_change_seq > 0 && state_change_seq > old.state_change_seq);
                if changed {
                    old.last_activity_at.max(Some(now()))
                } else {
                    old.last_activity_at
                }
            });
            Some(Session {
                id,
                workspace_label: workspace_info.map(|(label, _)| (*label).to_string()),
                workspace_order: workspace_info.map_or(0, |(_, order)| *order),
                pane_order,
                state_change_seq,
                display_name,
                display_agent,
                last_activity_at,
                context: Context {
                    host: local_host(),
                    herdr_socket: Some(socket.into()),
                    workspace_id: workspace,
                    tab_id: text(p, "tab_id"),
                    pane_id: Some(pane),
                    native_session_id: native,
                    agent,
                    label,
                    cwd: text(p, "foreground_cwd")
                        .or_else(|| text(p, "cwd"))
                        .unwrap_or_default(),
                },
                status,
                online: true,
                terminal_id: terminal,
            })
        })
        .collect()
}

#[cfg(test)]
mod passive_read_tests {
    use super::*;
    use std::{io::ErrorKind, os::unix::net::UnixListener, sync::mpsc};

    #[test]
    fn failed_visible_reads_never_fall_back_to_interactive_history() {
        let dir = tempfile::Builder::new()
            .prefix("fdr-")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let session = sessions_from_snapshot(
            socket.to_str().unwrap(),
            &json!({"panes":[{"pane_id":"test","agent":"codex"}]}),
            &[],
        )
        .remove(0);
        let (done, finished) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request: Value =
                serde_json::from_str(&bounded_line(stream.try_clone().unwrap()).unwrap()).unwrap();
            assert_eq!(request["method"], "pane.read");
            assert_eq!(request["params"]["source"], "visible");
            writeln!(
                stream,
                "{}",
                json!({"error":{"code":"unsupported","message":"Visible snapshots unavailable"}})
            )
            .unwrap();
            drop(stream);
            // Keep the listener alive until the client returns. Any attempted
            // fallback connection remains queued and fails the assertion below.
            finished.recv_timeout(Duration::from_secs(4)).unwrap();
            listener.set_nonblocking(true).unwrap();
            assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
        });
        let result = pane_text(&session);
        done.send(()).unwrap();
        server.join().unwrap();
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Visible snapshots unavailable")
        );
    }
}

#[cfg(test)]
mod focus_tests {
    use super::*;
    use crate::store::Store;
    use std::{
        os::unix::net::UnixListener,
        sync::{Arc, Condvar, Mutex, mpsc},
    };

    #[test]
    fn subscribed_split_pane_focus_wins_over_the_server_default() {
        // Newer herdr can report a client whose selection differs from the
        // server's default. The periodic snapshot must preserve that choice.
        check_split_focus("0.9.1", &[(1, 0, 1), (0, 0, 0)]);
    }

    #[test]
    fn older_herdr_follows_current_focus_without_replaying_old_selections() {
        check_split_focus("0.8.2", &[(1, 0, 0), (1, 1, 1), (0, 0, 0)]);
    }

    fn check_split_focus(version: &'static str, steps: &'static [(usize, usize, usize)]) {
        let dir = tempfile::Builder::new()
            .prefix("fdr-")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let snapshot = json!({
            "version": version,
            "workspaces": [{"workspace_id": "w", "label": "Project"}],
            "panes": [
                {"pane_id": "w:p1", "workspace_id": "w", "tab_id": "t", "agent": "codex"},
                {"pane_id": "w:p2", "workspace_id": "w", "tab_id": "t", "agent": "codex"}
            ]
        });
        let key = socket.to_str().unwrap();
        let panes = sessions_from_snapshot(key, &snapshot, &[]);
        assert_ne!(panes[0].id, panes[1].id);
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        let other = dir.path().join("other.sock");
        let other_key = other.to_str().unwrap();
        store
            .reconcile(other_key, sessions_from_snapshot(other_key, &snapshot, &[]))
            .unwrap();
        let shared = Arc::new((
            Mutex::new(State {
                store,
                connections: HashMap::new(),
                herdr_focus: None,
                discovery_error: None,
                desktop: Default::default(),
            }),
            Condvar::new(),
        ));
        let (focus_pane, focus_requests) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut events, _) = listener.accept().unwrap();
            let subscribe: Value =
                serde_json::from_str(&bounded_line(events.try_clone().unwrap()).unwrap()).unwrap();
            assert_eq!(subscribe["method"], "events.subscribe");
            assert!(
                subscribe["params"]["subscriptions"]
                    .as_array()
                    .unwrap()
                    .contains(&json!({"type":"pane.focused"}))
            );
            writeln!(
                events,
                "{}",
                json!({"result":{"type":"subscription_started"}})
            )
            .unwrap();
            let send_snapshot = |protocol, pane| {
                let (mut stream, _) = listener.accept().unwrap();
                let request: Value =
                    serde_json::from_str(&bounded_line(stream.try_clone().unwrap()).unwrap())
                        .unwrap();
                assert_eq!(request["method"], "session.snapshot");
                let mut snapshot = snapshot.clone();
                snapshot["protocol"] = json!(protocol);
                snapshot["focused_pane_id"] = snapshot["panes"][pane]["pane_id"].clone();
                writeln!(stream, "{}", json!({"result":{"snapshot":snapshot}})).unwrap();
            };
            send_snapshot(0, 0usize);
            for (index, &(event_pane, snapshot_pane, _)) in steps.iter().enumerate() {
                focus_requests.recv_timeout(Duration::from_secs(3)).unwrap();
                writeln!(
                    events,
                    "{}",
                    json!({"event":"pane_focused","data":{
                        "pane_id":snapshot["panes"][event_pane]["pane_id"],"workspace_id":"w"
                    }})
                )
                .unwrap();
                send_snapshot(index + 1, snapshot_pane);
            }
            focus_requests.recv_timeout(Duration::from_secs(3)).unwrap();
        });
        let watcher = {
            let shared = shared.clone();
            let socket = socket.clone();
            thread::spawn(move || {
                let error = subscribed(&shared, &socket).unwrap_err();
                status(&shared, socket.to_str().unwrap(), false, error.to_string());
            })
        };
        let wait_for_focus = |id: &str, after, protocol| {
            let message = format!("herdr {version} · protocol {protocol}");
            let (state, timeout) = shared
                .1
                .wait_timeout_while(shared.0.lock().unwrap(), Duration::from_secs(3), |state| {
                    let sessions = state.store.sessions().unwrap();
                    state.store.sequence().unwrap() <= after
                        || state
                            .connections
                            .get(key)
                            .is_none_or(|status| status.message != message)
                        || state
                            .herdr_focus
                            .as_ref()
                            .and_then(|focus| focus.session_id(&sessions))
                            != Some(id)
                })
                .unwrap();
            assert!(!timeout.timed_out(), "expected focus on {id}");
            state.store.sequence().unwrap()
        };
        let mut sequence = wait_for_focus(&panes[0].id, -1, 0);
        for (index, &(_, _, expected)) in steps.iter().enumerate() {
            focus_pane.send(()).unwrap();
            sequence = wait_for_focus(&panes[expected].id, sequence, index + 1);
        }
        focus_pane.send(()).unwrap();
        server.join().unwrap();
        watcher.join().unwrap();
        assert!(shared.0.lock().unwrap().herdr_focus.is_none());
    }
}

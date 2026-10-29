use crate::{herdr, model::*, store::Store};
use anyhow::{Context as _, Result, bail, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const MAX_MESSAGE: u64 = 1_048_576;
#[derive(Clone, Debug)]
pub struct Paths {
    pub root: PathBuf,
    pub socket: PathBuf,
}
impl Paths {
    pub fn new(root: Option<PathBuf>) -> Result<Self> {
        let root = root.unwrap_or_else(|| {
            dirs::data_local_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("feedr")
        });
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()?.join(root)
        };
        let socket = root.join("feedr.sock");
        ensure!(
            socket.as_os_str().len() < 100,
            "Data directory path is too long for a Unix socket; use --data-dir with a shorter path"
        );
        Ok(Self { root, socket })
    }
    pub fn prepare(&self) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }
}

pub struct State {
    pub store: Store,
    pub connections: HashMap<String, ConnectionStatus>,
    pub discovery_error: Option<String>,
}
pub type Shared = Arc<(Mutex<State>, Condvar)>;

#[derive(Serialize, Deserialize)]
struct Request {
    version: u32,
    method: String,
    #[serde(default)]
    params: Value,
}
#[derive(Serialize, Deserialize)]
struct Reply {
    version: u32,
    ok: bool,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: Option<String>,
}

pub fn bounded_line(reader: impl Read) -> Result<String> {
    bounded_line_limit(reader, MAX_MESSAGE)
}
fn bounded_line_limit(reader: impl Read, limit: u64) -> Result<String> {
    let mut line = String::new();
    let mut reader = BufReader::new(reader.take(limit + 1));
    reader.read_line(&mut line)?;
    ensure!(
        line.len() as u64 <= limit,
        "Protocol message exceeds size limit"
    );
    ensure!(line.ends_with('\n'), "Incomplete protocol message");
    Ok(line)
}

pub fn serve(paths: Paths) -> Result<()> {
    paths.prepare()?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(paths.root.join("broker.lock"))?;
    lock.try_lock_exclusive()
        .context("Feedr broker is already running")?;
    if paths.socket.exists() {
        fs::remove_file(&paths.socket)?;
    }
    let listener = UnixListener::bind(&paths.socket)?;
    fs::set_permissions(&paths.socket, fs::Permissions::from_mode(0o600))?;
    let state = Arc::new((
        Mutex::new(State {
            store: Store::open(&paths.root.join("feedr.sqlite3"))?,
            connections: HashMap::new(),
            discovery_error: None,
        }),
        Condvar::new(),
    ));
    herdr::start(state.clone(), paths.root.clone());
    crate::discovery::start(state.clone());
    let clients = Arc::new(AtomicUsize::new(0));
    let stopping = Arc::new(AtomicBool::new(false));
    eprintln!("Feedr broker listening on {}", paths.socket.display());
    for stream in listener.incoming() {
        if stopping.load(Ordering::Acquire) {
            break;
        }
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Accept: {e}");
                continue;
            }
        };
        if clients.fetch_add(1, Ordering::SeqCst) >= 64 {
            clients.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        let state = state.clone();
        let clients = clients.clone();
        let stopping = stopping.clone();
        let wake_socket = paths.socket.clone();
        thread::spawn(move || {
            match handle(stream, state) {
                Ok(true) => {
                    stopping.store(true, Ordering::Release);
                    // Wake blocking accept without adding an idle polling timer.
                    let _ = UnixStream::connect(wake_socket);
                }
                Ok(false) => {}
                Err(e) => eprintln!("Client: {e:#}"),
            }
            clients.fetch_sub(1, Ordering::SeqCst);
        });
    }
    // Let acknowledged requests finish before releasing the single-broker lock.
    let deadline = Instant::now() + Duration::from_secs(10);
    while clients.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}
fn handle(mut stream: UnixStream, shared: Shared) -> Result<bool> {
    stream.set_read_timeout(Some(Duration::from_secs(8)))?;
    stream.set_write_timeout(Some(Duration::from_secs(8)))?;
    let mut shutdown = false;
    let result = (|| -> Result<Value> {
        let request: Request = serde_json::from_str(&bounded_line(stream.try_clone()?)?)?;
        ensure!(request.version == 1, "Unsupported Feedr protocol version");
        if request.method == "post"
            && request.params["session_id"].is_null()
            && let Some(socket) = request.params["context"]["herdr_socket"].as_str()
            && request.params["context"]["pane_id"].is_string()
        {
            herdr::refresh(&shared, Path::new(socket))
                .context("Cannot bind the producing herdr pane")?;
        }
        let mut state = shared
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Broker state lock failed"))?;
        if request.method == "wait" {
            let after = request.params["after"].as_i64().unwrap_or(-1);
            let timeout = request.params["timeout_ms"]
                .as_u64()
                .unwrap_or(1000)
                .min(4000);
            let (guard, _) = shared
                .1
                .wait_timeout_while(state, Duration::from_millis(timeout), |s| {
                    s.store.sequence().unwrap_or(-1) <= after
                })
                .map_err(|_| anyhow::anyhow!("Broker wait failed"))?;
            state = guard;
            return Ok(json!({"sequence":state.store.sequence()?}));
        }
        let result = match request.method.as_str() {
            "shutdown" => {
                shutdown = true;
                json!({"stopping":true})
            }
            "discovery.config" => serde_json::to_value(state.store.discovery_config()?)?,
            "discovery.configure" => {
                let config = serde_json::from_value(request.params)?;
                state.store.set_discovery_config(&config)?;
                serde_json::to_value(config)?
            }
            "discovery.ingest" => serde_json::to_value(
                state
                    .store
                    .ingest(&serde_json::from_value(request.params)?)?,
            )?,
            "discovery.keep" => serde_json::to_value(
                state.store.keep_discovery(
                    request.params["id"]
                        .as_str()
                        .context("Missing mention ID")?,
                    request.params["expected_revision"]
                        .as_i64()
                        .context("Missing expected revision")?,
                )?,
            )?,
            "ping" => {
                json!({"version":env!("CARGO_PKG_VERSION"),"protocol":1,"sequence":state.store.sequence()?})
            }
            "post" => {
                serde_json::to_value(state.store.post(serde_json::from_value(request.params)?)?)?
            }
            "update" => serde_json::to_value(
                state
                    .store
                    .update(serde_json::from_value(request.params)?)?,
            )?,
            "get" => serde_json::to_value(
                state
                    .store
                    .get(request.params["id"].as_str().context("Missing post id")?)?,
            )?,
            "snapshot" => {
                let mut s = state
                    .store
                    .snapshot(&serde_json::from_value(request.params)?)?;
                s.connections = state.connections.values().cloned().collect();
                s.discovery_error = state.discovery_error.clone();
                s.connections.sort_by(|a, b| a.socket.cmp(&b.socket));
                serde_json::to_value(s)?
            }
            "sessions" => serde_json::to_value(state.store.sessions()?)?,
            _ => bail!("Unknown method {}", request.method),
        };
        drop(state);
        shared.1.notify_all();
        Ok(result)
    })();
    let reply = match result {
        Ok(result) => Reply {
            version: 1,
            ok: true,
            result,
            error: None,
        },
        Err(e) => Reply {
            version: 1,
            ok: false,
            result: Value::Null,
            error: Some(format!("{e:#}")),
        },
    };
    serde_json::to_writer(&mut stream, &reply)?;
    stream.write_all(b"\n")?;
    Ok(shutdown)
}

#[derive(Clone)]
pub struct Client {
    pub paths: Paths,
}
impl Client {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }
    pub fn call<T: serde::de::DeserializeOwned>(&self, method: &str, params: Value) -> Result<T> {
        self.call_with_timeout(method, params, Duration::from_secs(8))
    }
    pub fn call_with_timeout<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<T> {
        let mut stream = UnixStream::connect(&self.paths.socket)
            .with_context(|| format!("Connect to Feedr at {}", self.paths.socket.display()))?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout.min(Duration::from_secs(5))))?;
        let request = Request {
            version: 1,
            method: method.into(),
            params,
        };
        let bytes = serde_json::to_vec(&request)?;
        ensure!((bytes.len() as u64) < MAX_MESSAGE, "Request exceeds 1 MiB");
        stream.write_all(&bytes)?;
        stream.write_all(b"\n")?;
        let reply: Reply = serde_json::from_str(&bounded_line_limit(stream, 64 * 1024 * 1024)?)?;
        ensure!(reply.version == 1, "Incompatible broker protocol");
        if !reply.ok {
            bail!(
                "{}",
                reply
                    .error
                    .unwrap_or_else(|| "Broker request failed".into())
            );
        }
        Ok(serde_json::from_value(reply.result)?)
    }
    pub fn ensure_running(&self) -> Result<()> {
        let _update_guard = installation_lock(&self.paths, false)?;
        if self.call::<Value>("ping", json!({})).is_ok() {
            return Ok(());
        }
        self.paths.prepare()?;
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.paths.root.join("broker.log"))?;
        Command::new(self.service_executable()?)
            .process_group(0)
            .arg("--data-dir")
            .arg(&self.paths.root)
            .arg("serve")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.call::<Value>("ping", json!({})).is_ok() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
        bail!(
            "Broker did not start; inspect {}",
            self.paths.root.join("broker.log").display()
        )
    }
    fn service_executable(&self) -> Result<PathBuf> {
        let preferred = fs::read(self.paths.root.join("updates/executable.json"))
            .ok()
            .filter(|bytes| bytes.len() < 64 * 1024)
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        if let Some(record) = preferred
            && let (Some(path), Some(version)) =
                (record["path"].as_str(), record["version"].as_str())
            && let Ok(version) = semver::Version::parse(version)
            && version >= semver::Version::parse(env!("CARGO_PKG_VERSION"))?
            && Path::new(path).is_absolute()
            && Path::new(path).is_file()
        {
            return Ok(path.into());
        }
        Ok(std::env::current_exe()?)
    }
    pub fn snapshot(&self, q: &FeedQuery) -> Result<Snapshot> {
        self.call("snapshot", serde_json::to_value(q)?)
    }
}

pub(crate) fn installation_lock(paths: &Paths, exclusive: bool) -> Result<fs::File> {
    paths.prepare()?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(paths.root.join("update.lock"))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let result = if exclusive {
            fs2::FileExt::try_lock_exclusive(&file)
        } else {
            fs2::FileExt::try_lock_shared(&file)
        };
        match result {
            Ok(()) => return Ok(file),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                ensure!(
                    Instant::now() < deadline,
                    "Feedr is installing an update; try again shortly"
                );
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub fn add_socket(paths: &Paths, socket: &Path) -> Result<()> {
    paths.prepare()?;
    ensure!(socket.is_absolute(), "Herdr socket path must be absolute");
    let file = paths.root.join("herdr-sockets.json");
    let mut sockets: Vec<String> = if file.exists() {
        serde_json::from_slice(&fs::read(&file)?)?
    } else {
        vec![]
    };
    let s = socket.to_string_lossy().into_owned();
    if !sockets.contains(&s) {
        sockets.push(s);
    }
    let tmp = file.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(&sockets)?)?;
    fs::rename(tmp, file)?;
    Ok(())
}

#[cfg(test)]
mod update_tests {
    use super::*;
    #[test]
    fn installation_excludes_service_startups_and_keeps_the_installed_executable_preferred() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::new(Some(root.path().into())).unwrap();
        let reader = installation_lock(&paths, false).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let other = paths.clone();
        let worker = thread::spawn(move || {
            let guard = installation_lock(&other, true).unwrap();
            tx.send(()).unwrap();
            drop(guard);
        });
        assert!(rx.recv_timeout(Duration::from_millis(80)).is_err());
        drop(reader);
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        fs::create_dir(root.path().join("updates")).unwrap();
        let preferred = root.path().join("installed-feedr");
        fs::write(&preferred, b"executable fixture").unwrap();
        let config = root.path().join("updates/executable.json");
        fs::write(
            &config,
            json!({"path":preferred,"version":"999.0.0"}).to_string(),
        )
        .unwrap();
        let client = Client::new(paths);
        assert_eq!(client.service_executable().unwrap(), preferred);
        fs::write(
            &config,
            json!({"path":preferred,"version":"0.0.0"}).to_string(),
        )
        .unwrap();
        assert_eq!(
            client.service_executable().unwrap(),
            std::env::current_exe().unwrap()
        );
    }
}

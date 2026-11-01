//! Anonymous statistics sent to PostHog; see TELEMETRY.md. Each installation is counted once by default;
//! daily activity and usage counts are opt-in. Nothing is sent by builds without a project token, after
//! the user opts out, or while DO_NOT_TRACK or FEEDR_TELEMETRY=0 is set.
use crate::{agent_setup::write_private, discovery::FileType, model::Post};
use anyhow::{Result, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const DETAILS_URL: &str = "https://github.com/SvenKratz/feedr/blob/main/TELEMETRY.md";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DAY: u64 = 86_400;
const INTERVAL: Duration = Duration::from_secs(10 * 60);
const MAX_OUTBOX: usize = 64;

#[derive(Clone, Debug)]
struct Endpoint {
    host: String,
    token: String,
    /// Debug builds print events so development runs never mix into release statistics.
    dry_run: bool,
}
impl Endpoint {
    fn built_in() -> Option<Self> {
        Some(Self {
            host: option_env!("FEEDR_POSTHOG_HOST")?.into(),
            token: option_env!("FEEDR_POSTHOG_TOKEN")?.into(),
            dry_run: cfg!(debug_assertions),
        })
    }
    fn send(&self, batch: &[Value]) -> Result<()> {
        if self.dry_run {
            eprintln!(
                "Usage statistics (debug build, not sent): {}",
                Value::from(batch)
            );
            return Ok(());
        }
        let response = reqwest::blocking::Client::builder()
            .user_agent(concat!("feedr/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()?
            .post(format!("{}/batch/", self.host))
            .json(&json!({"api_key":self.token,"batch":batch}))
            .send()?;
        ensure!(
            response.status().is_success(),
            "Usage statistics upload returned HTTP {}",
            response.status().as_u16()
        );
        Ok(())
    }
}

/// Whether this build can send statistics at all; source builds without a token cannot.
pub fn available() -> bool {
    Endpoint::built_in().is_some()
}
/// The environment variable that currently overrides recorded consent, if any.
pub fn blocked_by() -> Option<&'static str> {
    blocked(|key| std::env::var(key).ok())
}
fn blocked(var: impl Fn(&str) -> Option<String>) -> Option<&'static str> {
    let value = |key| {
        var(key)
            .map(|v| v.trim().to_ascii_lowercase())
            .filter(|v| !v.is_empty())
    };
    if value("DO_NOT_TRACK").is_some_and(|v| v != "0" && v != "false") {
        Some("DO_NOT_TRACK")
    } else if value("FEEDR_TELEMETRY")
        .is_some_and(|v| matches!(v.as_str(), "0" | "false" | "off" | "no"))
    {
        Some("FEEDR_TELEMETRY")
    } else {
        None
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Consent {
    enabled: bool,
}
#[derive(Default, Serialize, Deserialize)]
struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    install_id: Option<String>,
    #[serde(default)]
    day: i64,
    #[serde(default)]
    counts: BTreeMap<String, u64>,
    #[serde(default)]
    active_day: i64,
    #[serde(default)]
    version: String,
    #[serde(default)]
    outbox: Vec<Value>,
}

fn dir(data: &Path) -> PathBuf {
    data.join("telemetry")
}
fn load<T: DeserializeOwned>(path: &Path) -> Option<T> {
    fs::read(path)
        .ok()
        .filter(|bytes| bytes.len() <= 1024 * 1024)
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}
fn consent(data: &Path) -> Option<Consent> {
    load(&dir(data).join("consent.json"))
}
fn state(data: &Path) -> State {
    load(&dir(data).join("state.json")).unwrap_or_default()
}
fn save(path: &Path, value: &impl Serialize) -> Result<()> {
    write_private(path, &serde_json::to_vec(value)?)
}
/// Serializes the desktop app, the local service and the CLI across processes.
fn lock(data: &Path) -> Result<fs::File> {
    fs::create_dir_all(dir(data))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir(data).join("lock"))?;
    file.lock_exclusive()?;
    Ok(file)
}

/// The recorded choice, or None when the user has not been asked yet.
pub fn decision(data: &Path) -> Option<bool> {
    consent(data).map(|c| c.enabled)
}
/// Whether first launch should ask: the build can send, nothing overrides it and no choice is recorded.
pub fn should_ask(data: &Path) -> bool {
    available() && blocked_by().is_none() && decision(data).is_none()
}
/// Records consent. Opting out discards unsent data but keeps the install ID, so opting back in
/// does not count the installation twice; opting out before Feedr first runs skips the install count.
pub fn set_enabled(data: &Path, enabled: bool) -> Result<()> {
    let _lock = lock(data)?;
    let mut state = state(data);
    if !enabled {
        state = State {
            install_id: state.install_id,
            ..State::default()
        };
    } else if state.install_id.is_none() {
        install(&mut state, unix_now());
    }
    save(&dir(data).join("state.json"), &state)?;
    save(&dir(data).join("consent.json"), &Consent { enabled })
}
/// Creates the random install ID and queues the one-time install event.
fn install(state: &mut State, now: u64) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    state
        .outbox
        .push(event("feedr_installed", &id, now, Map::new()));
    state.install_id = Some(id.clone());
    id
}

pub fn status(data: &Path) -> Value {
    let endpoint = Endpoint::built_in();
    let decided = decision(data);
    let state = state(data);
    let blocked = blocked_by();
    let id = state.install_id.clone();
    let sending = decided != Some(false)
        && blocked.is_none()
        && endpoint.as_ref().is_some_and(|e| !e.dry_run);
    let today = state.day == (unix_now() / DAY) as i64;
    let example = event(
        "feedr_active",
        id.as_deref().unwrap_or("random-install-id"),
        unix_now(),
        Map::new(),
    );
    json!({
        "available": endpoint.is_some(),
        "decided": decided.is_some(),
        "enabled": decided == Some(true),
        "blocked_by": blocked,
        "sending": sending,
        "debug_build": cfg!(debug_assertions),
        "endpoint": endpoint.map(|e| e.host),
        "install_id": id,
        "counts_today": if today { state.counts } else { BTreeMap::new() },
        "pending_events": state.outbox.len(),
        "example_event": example,
        "details": DETAILS_URL,
    })
}

struct Recorder {
    data: PathBuf,
    endpoint: Endpoint,
    /// Only the desktop app marks a day active; the service runs whenever an agent connects.
    app: bool,
    blocked: fn() -> Option<&'static str>,
    counts: Mutex<BTreeMap<String, u64>>,
}
static RECORDER: OnceLock<Recorder> = OnceLock::new();

/// Starts periodic delivery for the desktop app or the local service; a no-op without a built-in endpoint.
pub fn start(data: &Path, app: bool) {
    let Some(endpoint) = Endpoint::built_in() else {
        return;
    };
    if RECORDER
        .set(Recorder::new(data.into(), endpoint, app))
        .is_err()
    {
        return;
    }
    thread::spawn(|| {
        loop {
            if let Some(recorder) = RECORDER.get() {
                let _ = recorder.tick(unix_now(), true);
            }
            thread::sleep(INTERVAL);
        }
    });
}
/// Delivers right away, e.g. after the user opts in.
pub fn wake() {
    if RECORDER.get().is_some() {
        thread::spawn(|| {
            if let Some(recorder) = RECORDER.get() {
                let _ = recorder.tick(unix_now(), true);
            }
        });
    }
}
/// Saves pending counts without network access, e.g. when the window closes.
pub fn persist() {
    if let Some(recorder) = RECORDER.get() {
        let _ = recorder.tick(unix_now(), false);
    }
}
/// Counts a fixed, content-free action name for today's usage summary.
pub fn count(name: &str) {
    if let Some(recorder) = RECORDER.get() {
        recorder.count(name);
    }
}
/// Counts a newly published post by producing client and attachment type, never by content.
pub fn record_post(post: &Post) {
    let Some(recorder) = RECORDER.get() else {
        return;
    };
    recorder.count("posts");
    let agent = post.context.agent.to_ascii_lowercase();
    recorder.count(if agent.contains("claude") {
        "posts_from_claude"
    } else if agent.contains("codex") {
        "posts_from_codex"
    } else {
        "posts_from_other"
    });
    for resource in &post.resources {
        recorder.count(&format!("resources_{}", FileType::of(resource).key()));
    }
}

impl Recorder {
    fn new(data: PathBuf, endpoint: Endpoint, app: bool) -> Self {
        Self {
            data,
            endpoint,
            app,
            blocked: blocked_by,
            counts: Mutex::default(),
        }
    }
    fn count(&self, name: &str) {
        if let Ok(mut counts) = self.counts.lock() {
            *counts.entry(name.into()).or_default() += 1;
        }
    }
    /// Counts the installation once, folds today's counts in when the user opted in, then delivers.
    fn tick(&self, now: u64, deliver: bool) -> Result<()> {
        let counts = self
            .counts
            .lock()
            .map(|mut counts| std::mem::take(&mut *counts))
            .unwrap_or_default();
        let batch = {
            let _lock = lock(&self.data)?;
            let enabled = decision(&self.data);
            if enabled == Some(false) || (self.blocked)().is_some() {
                return Ok(());
            }
            let mut state = state(&self.data);
            let id = match state.install_id.clone() {
                Some(id) => id,
                None => install(&mut state, now),
            };
            if enabled == Some(true) {
                self.fold(&mut state, &id, counts, now);
            }
            let batch = if deliver {
                std::mem::take(&mut state.outbox)
            } else {
                Vec::new()
            };
            save(&dir(&self.data).join("state.json"), &state)?;
            batch
        };
        if batch.is_empty() {
            return Ok(());
        }
        let result = self.endpoint.send(&batch);
        if result.is_err() {
            let _lock = lock(&self.data)?;
            if decision(&self.data) != Some(false) {
                let mut state = state(&self.data);
                let mut retry = batch;
                retry.append(&mut state.outbox);
                retry.drain(..retry.len().saturating_sub(MAX_OUTBOX));
                state.outbox = retry;
                save(&dir(&self.data).join("state.json"), &state)?;
            }
        }
        result
    }
    /// Folds counts into the shared daily state and queues finished days and today's activity.
    fn fold(&self, state: &mut State, id: &str, counts: BTreeMap<String, u64>, now: u64) {
        let today = (now / DAY) as i64;
        if state.day != today {
            if !state.counts.is_empty() {
                let counts = state
                    .counts
                    .iter()
                    .map(|(name, count)| (name.clone(), Value::from(*count)))
                    .collect();
                // Noon UTC keeps the summary on the same calendar day in nearly every time zone.
                state.outbox.push(event(
                    "feedr_usage",
                    id,
                    state.day as u64 * DAY + DAY / 2,
                    counts,
                ));
            }
            state.day = today;
            state.counts.clear();
        }
        if !state.version.is_empty() && state.version != VERSION {
            *state.counts.entry("updated".into()).or_default() += 1;
        }
        state.version = VERSION.into();
        for (name, count) in counts {
            *state.counts.entry(name).or_default() += count;
        }
        if self.app && state.active_day != today {
            state.active_day = today;
            state
                .outbox
                .push(event("feedr_active", id, now, Map::new()));
        }
    }
}

fn event(name: &str, id: &str, at: u64, mut properties: Map<String, Value>) -> Value {
    properties.extend([
        ("$process_person_profile".into(), false.into()),
        ("$geoip_disable".into(), true.into()),
        ("$lib".into(), "feedr".into()),
        ("$lib_version".into(), VERSION.into()),
        ("version".into(), VERSION.into()),
        ("os".into(), std::env::consts::OS.into()),
        ("arch".into(), std::env::consts::ARCH.into()),
        ("install".into(), channel().into()),
    ]);
    json!({"event":name,"distinct_id":id,"timestamp":timestamp(at),"properties":properties})
}
fn channel() -> &'static str {
    let Ok(exe) = std::env::current_exe() else {
        return "unknown";
    };
    let path = exe.to_string_lossy();
    if path.contains(".app/Contents/MacOS/") {
        "macos-app"
    } else if crate::update::package_managed(&exe) {
        "deb"
    } else if path.contains("/.cargo/bin/") {
        "cargo"
    } else if path.contains("/target/") {
        "source"
    } else {
        "portable"
    }
}
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
/// RFC 3339 UTC time using Howard Hinnant's civil-from-days conversion.
fn timestamp(secs: u64) -> String {
    let z = (secs / DAY) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let time = secs % DAY;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        sync::mpsc,
    };

    fn collector(status: u16) -> (String, mpsc::Receiver<Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let _ = tx.send(serde_json::from_slice(&body).unwrap());
                write!(
                    stream,
                    "HTTP/1.1 {status} Status\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            }
        });
        (host, rx)
    }
    fn recorder(data: &Path, host: String) -> Recorder {
        Recorder {
            blocked: || None,
            ..Recorder::new(
                data.into(),
                Endpoint {
                    host,
                    token: "phc_test".into(),
                    dry_run: false,
                },
                true,
            )
        }
    }
    fn names(body: &Value) -> Vec<&str> {
        body["batch"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["event"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn opting_out_discards_unsent_data_but_never_counts_the_install_twice() {
        let data = tempfile::tempdir().unwrap();
        assert_eq!(decision(data.path()), None);
        set_enabled(data.path(), true).unwrap();
        let first = state(data.path()).install_id.unwrap();
        assert!(uuid::Uuid::parse_str(&first).is_ok());
        assert_eq!(state(data.path()).outbox.len(), 1);
        set_enabled(data.path(), true).unwrap();
        assert_eq!(state(data.path()).outbox.len(), 1);
        set_enabled(data.path(), false).unwrap();
        assert_eq!(decision(data.path()), Some(false));
        assert_eq!(state(data.path()).install_id.unwrap(), first);
        assert!(state(data.path()).outbox.is_empty());
        set_enabled(data.path(), true).unwrap();
        assert_eq!(state(data.path()).install_id.unwrap(), first);
        assert!(state(data.path()).outbox.is_empty());
    }

    #[test]
    fn installs_are_counted_once_by_default_and_nothing_else_before_opting_in() {
        let data = tempfile::tempdir().unwrap();
        let (down, rx_down) = collector(503);
        let offline = recorder(data.path(), down);
        offline.count("posts");
        assert!(offline.tick(DAY * 20_000, true).is_err());
        rx_down.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(decision(data.path()), None);

        let (host, rx) = collector(200);
        let recorder = recorder(data.path(), host);
        recorder.count("posts");
        recorder.tick(DAY * 20_000 + 60, true).unwrap();
        let body = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(names(&body), ["feedr_installed"]);
        let id = state(data.path()).install_id.unwrap();
        assert_eq!(body["batch"][0]["distinct_id"], id.as_str());
        assert!(state(data.path()).counts.is_empty());
        recorder.tick(DAY * 20_001, true).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());

        set_enabled(data.path(), true).unwrap();
        recorder.tick(DAY * 20_001 + 60, true).unwrap();
        let body = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(names(&body), ["feedr_active"]);
        assert_eq!(body["batch"][0]["distinct_id"], id.as_str());
    }

    #[test]
    fn opting_out_first_or_do_not_track_skips_the_install_count() {
        let (host, rx) = collector(200);
        let declined = tempfile::tempdir().unwrap();
        set_enabled(declined.path(), false).unwrap();
        recorder(declined.path(), host.clone())
            .tick(DAY, true)
            .unwrap();
        assert!(state(declined.path()).install_id.is_none());

        let tracked = tempfile::tempdir().unwrap();
        let blocked = Recorder {
            blocked: || Some("DO_NOT_TRACK"),
            ..recorder(tracked.path(), host)
        };
        blocked.tick(DAY, true).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        assert!(!dir(tracked.path()).join("state.json").exists());
    }

    #[test]
    fn daily_activity_and_usage_are_batched_anonymously() {
        let data = tempfile::tempdir().unwrap();
        let (host, rx) = collector(200);
        let recorder = recorder(data.path(), host);
        set_enabled(data.path(), true).unwrap();
        let id = state(data.path()).install_id.unwrap();
        let day = DAY * 20_000;
        recorder.count("posts");
        recorder.count("posts");
        recorder.count("resources_image");
        recorder.tick(day + 3_600, true).unwrap();
        let body = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(body["api_key"], "phc_test");
        assert_eq!(names(&body), ["feedr_installed", "feedr_active"]);
        for event in body["batch"].as_array().unwrap() {
            assert_eq!(event["distinct_id"], id.as_str());
            assert_eq!(event["properties"]["$process_person_profile"], false);
            assert_eq!(event["properties"]["version"], VERSION);
        }
        assert!(!body.to_string().contains(&*data.path().to_string_lossy()));

        recorder.tick(day + 7_200, true).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());

        recorder.count("posts");
        recorder.tick(day + DAY + 60, true).unwrap();
        let body = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(names(&body), ["feedr_usage", "feedr_active"]);
        let usage = &body["batch"][0];
        assert_eq!(usage["timestamp"], timestamp(day + DAY / 2));
        assert_eq!(usage["properties"]["posts"], 2);
        assert_eq!(usage["properties"]["resources_image"], 1);
        assert_eq!(state(data.path()).counts["posts"], 1);
    }

    #[test]
    fn failed_uploads_are_retried_and_opting_out_discards_them() {
        let data = tempfile::tempdir().unwrap();
        set_enabled(data.path(), true).unwrap();
        let (down, rx_down) = collector(503);
        assert!(recorder(data.path(), down).tick(DAY, true).is_err());
        rx_down.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(state(data.path()).outbox.len(), 2);
        let (up, rx_up) = collector(200);
        recorder(data.path(), up.clone())
            .tick(DAY + 60, true)
            .unwrap();
        let body = rx_up.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(names(&body), ["feedr_installed", "feedr_active"]);
        assert!(state(data.path()).outbox.is_empty());

        let (down, _rx) = collector(503);
        let recorder_down = recorder(data.path(), down);
        recorder_down.count("posts");
        assert!(recorder_down.tick(DAY * 2, true).is_err());
        assert!(!state(data.path()).outbox.is_empty());
        set_enabled(data.path(), false).unwrap();
        recorder(data.path(), up).tick(DAY * 3, true).unwrap();
        assert!(rx_up.recv_timeout(Duration::from_millis(300)).is_err());
    }

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn environment_switches_override_consent() {
        assert_eq!(blocked(env(&[])), None);
        assert_eq!(blocked(env(&[("DO_NOT_TRACK", "1")])), Some("DO_NOT_TRACK"));
        assert_eq!(
            blocked(env(&[("DO_NOT_TRACK", "true")])),
            Some("DO_NOT_TRACK")
        );
        assert_eq!(blocked(env(&[("DO_NOT_TRACK", "0")])), None);
        assert_eq!(
            blocked(env(&[("FEEDR_TELEMETRY", "off")])),
            Some("FEEDR_TELEMETRY")
        );
        assert_eq!(blocked(env(&[("FEEDR_TELEMETRY", "1")])), None);
    }

    #[test]
    fn timestamps_are_utc_rfc3339() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp(951_786_123), "2000-02-29T01:02:03Z");
        assert_eq!(timestamp(1_793_318_400), "2026-10-30T00:00:00Z");
    }
}

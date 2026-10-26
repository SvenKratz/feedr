//! Opt-in, deterministic resource discovery. No model or executable inference.
mod file_type;
mod parse;
mod persistence;
mod watch;
pub use file_type::FileType;
pub use parse::{extract, log_messages};
pub use watch::start;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    pub file_types: Vec<FileType>,
    pub sessions: Option<Vec<String>>,
    pub excluded_sessions: Vec<String>,
    pub exclude_paths: Vec<String>,
    pub logs: Vec<LogSource>,
    pub settle_ms: u64,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            file_types: FileType::ALL.to_vec(),
            sessions: None,
            excluded_sessions: vec![],
            exclude_paths: [
                "**/node_modules/**",
                "**/target/**",
                "**/.git/**",
                "**/vendor/**",
                "**/Cargo.lock",
                "**/package-lock.json",
                "**/bun.lock*",
            ]
            .map(str::to_string)
            .into(),
            logs: vec![],
            settle_ms: 1500,
        }
    }
}
impl Config {
    pub fn set_all_sessions(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.sessions = None;
        self.excluded_sessions.clear();
    }
    pub fn allows(&self, resource: &crate::model::Resource) -> bool {
        self.file_types.contains(&FileType::of(resource))
    }
    pub(crate) fn type_filter(&self) -> String {
        format!(
            "|{}|",
            self.file_types
                .iter()
                .map(|kind| kind.key())
                .collect::<Vec<_>>()
                .join("|")
        )
    }
    pub fn includes(&self, session: &str) -> bool {
        self.enabled && self.session_enabled(session)
    }
    pub fn session_enabled(&self, session: &str) -> bool {
        self.sessions
            .as_ref()
            .is_none_or(|ids| ids.iter().any(|id| id == session))
            && !self.excluded_sessions.iter().any(|id| id == session)
    }
    pub fn set_session(&mut self, session: &str, included: bool) {
        self.excluded_sessions.retain(|id| id != session);
        if let Some(ids) = &mut self.sessions {
            ids.retain(|id| id != session);
            if included {
                ids.push(session.to_string());
            }
        } else if !included {
            self.excluded_sessions.push(session.to_string());
        }
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.file_types.len() <= FileType::ALL.len(),
            "Too many discovery file types"
        );
        ensure!(
            self.sessions.as_ref().is_none_or(|ids| ids.len() <= 1000),
            "Too many discovery sessions"
        );
        ensure!(
            self.logs.len() <= 32
                && self.excluded_sessions.len() <= 1000
                && self.exclude_paths.len() <= 100,
            "Discovery configuration exceeds limits"
        );
        ensure!(
            (250..=60_000).contains(&self.settle_ms),
            "File settling time must be 250–60000 ms"
        );
        for pattern in &self.exclude_paths {
            globset::Glob::new(pattern)?;
        }
        for log in &self.logs {
            ensure!(
                std::path::Path::new(&log.path).is_absolute(),
                "Watched logs require absolute paths"
            );
            ensure!(
                matches!(log.format.as_str(), "plain" | "codex" | "claude" | "jsonl"),
                "Log format must be plain, codex, claude or jsonl"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogSource {
    pub path: String,
    pub session_id: String,
    pub format: String,
    #[serde(default)]
    pub from_start: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Origin {
    pub source: String,
    pub excerpt: String,
    pub locator: String,
    pub accepted: bool,
    pub unresolved: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Input {
    pub session_id: String,
    pub text: String,
    #[serde(default = "hook_source")]
    pub source: String,
}
fn hook_source() -> String {
    "hook".into()
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct Ingested {
    pub created: Vec<String>,
    pub matched: Vec<String>,
}

use anyhow::{Result, bail, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
pub fn local_host() -> String {
    hostname::get()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(default)]
pub struct Context {
    pub host: String,
    pub herdr_socket: Option<String>,
    pub workspace_id: Option<String>,
    pub tab_id: Option<String>,
    pub pane_id: Option<String>,
    pub native_session_id: Option<String>,
    pub agent: String,
    pub label: String,
    pub cwd: String,
}
impl Default for Context {
    fn default() -> Self {
        Self {
            host: local_host(),
            herdr_socket: None,
            workspace_id: None,
            tab_id: None,
            pane_id: None,
            native_session_id: None,
            agent: "Agent".into(),
            label: "Unassigned".into(),
            cwd: String::new(),
        }
    }
}
impl Context {
    pub fn from_env() -> Self {
        let env = |k| std::env::var(k).ok().filter(|s| !s.is_empty());
        Self {
            herdr_socket: env("HERDR_SOCKET_PATH"),
            workspace_id: env("HERDR_WORKSPACE_ID"),
            tab_id: env("HERDR_TAB_ID"),
            pane_id: env("HERDR_PANE_ID"),
            cwd: std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            ..Self::default()
        }
    }
    pub fn is_local(&self) -> bool {
        self.host == local_host()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    File,
    Folder,
    Url,
    App,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ResourceInput {
    /// Absolute path, path relative to context.cwd, file URI, or HTTP(S) URL.
    pub uri: String,
    #[serde(default)]
    pub kind: Option<ResourceKind>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Resource {
    pub original: String,
    pub target: String,
    pub kind: ResourceKind,
    pub name: String,
}
impl Resource {
    pub fn resolve(input: ResourceInput, context: &Context) -> Result<Self> {
        ensure!(
            !input.uri.trim().is_empty() && input.uri.len() <= 16_384,
            "Resource URI is empty or too long"
        );
        ensure!(
            !input.uri.contains('\0'),
            "Resource URI contains a NUL byte"
        );
        if input.uri.starts_with("https://") || input.uri.starts_with("http://") {
            let u = url::Url::parse(&input.uri)?;
            ensure!(u.host_str().is_some(), "URL must have a host");
            ensure!(
                input.kind.is_none() || input.kind == Some(ResourceKind::Url),
                "HTTP resources must have kind url"
            );
            return Ok(Self {
                name: u.host_str().unwrap_or("Web page").into(),
                original: input.uri,
                target: u.to_string(),
                kind: ResourceKind::Url,
            });
        }
        ensure!(
            input.kind != Some(ResourceKind::Url),
            "Only HTTP(S) URLs are supported"
        );
        let raw = if input.uri.starts_with("file:") {
            url::Url::parse(&input.uri)?
                .to_file_path()
                .map_err(|_| anyhow::anyhow!("Invalid or remote file URI"))?
        } else {
            if input.uri.contains("://") {
                bail!("Unsupported URI scheme");
            }
            if input.uri == "~" || input.uri.starts_with("~/") {
                ensure!(
                    context.is_local(),
                    "Use absolute paths for remote resources"
                );
                dirs::home_dir()
                    .ok_or_else(|| anyhow::anyhow!("Home directory unavailable"))?
                    .join(input.uri.strip_prefix("~/").unwrap_or(""))
            } else {
                PathBuf::from(&input.uri)
            }
        };
        let path = if raw.is_absolute() {
            raw
        } else {
            ensure!(
                Path::new(&context.cwd).is_absolute(),
                "Relative paths require an absolute context.cwd"
            );
            Path::new(&context.cwd).join(raw)
        };
        // Canonicalize existing local paths using filesystem semantics. Lexical
        // removal of '..' would select the wrong file below a symlink.
        let clean = if context.is_local() {
            // Canonicalize the existing prefix even when the output file is
            // still being created. Components removes harmless './' segments
            // while preserving '..' below missing directories or symlinks.
            let path: PathBuf = path.components().collect();
            path.ancestors()
                .find_map(|parent| {
                    parent.canonicalize().ok().map(|resolved| {
                        let suffix = path.strip_prefix(parent).unwrap();
                        if suffix.as_os_str().is_empty() {
                            resolved
                        } else {
                            resolved.join(suffix)
                        }
                    })
                })
                .unwrap_or(path)
        } else {
            path.components().collect()
        };
        let kind = input.kind.unwrap_or_else(|| {
            if clean.extension().is_some_and(|e| e == "app") {
                ResourceKind::App
            } else if context.is_local() && clean.is_dir() {
                ResourceKind::Folder
            } else {
                ResourceKind::File
            }
        });
        Ok(Self {
            name: clean
                .file_name()
                .unwrap_or(clean.as_os_str())
                .to_string_lossy()
                .into_owned(),
            original: input.uri,
            target: clean.to_string_lossy().into_owned(),
            kind,
        })
    }
    pub fn file_type(&self) -> &'static str {
        if self.kind == ResourceKind::Url {
            return "Web";
        }
        if self.kind == ResourceKind::App {
            return "App";
        }
        if self.kind == ResourceKind::Folder {
            return "Folder";
        }
        match Path::new(&self.target)
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_lowercase()
            .as_str()
        {
            "md" | "markdown" => "Markdown",
            "png" | "jpg" | "jpeg" | "gif" | "webp" => "Image",
            "pdf" => "PDF",
            "mp4" | "mov" | "mkv" | "webm" => "Video",
            _ => "File",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PostInput {
    pub title: String,
    #[serde(default)]
    pub body: String,
    pub resources: Vec<ResourceInput>,
    #[serde(default)]
    pub context: Option<Context>,
    /// Use an ID from feedr.sessions to bind a global MCP client to a session.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default = "ready")]
    pub readiness: String,
}
fn ready() -> String {
    "ready".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct UpdateInput {
    pub id: String,
    pub expected_revision: i64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub resources: Option<Vec<ResourceInput>>,
    #[serde(default)]
    pub readiness: Option<String>,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub archived: Option<bool>,
    #[serde(default)]
    pub read: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Post {
    #[serde(default)]
    pub discovery: Option<crate::discovery::Origin>,
    pub id: String,
    pub session_id: String,
    pub title: String,
    pub body: String,
    pub resources: Vec<Resource>,
    pub context: Context,
    pub readiness: String,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub pinned: bool,
    pub archived: bool,
    pub read: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub context: Context,
    pub status: String,
    pub online: bool,
    pub terminal_id: String,
    #[serde(default)]
    pub workspace_label: Option<String>,
    #[serde(default)]
    pub workspace_order: usize,
    #[serde(default)]
    pub pane_order: usize,
    #[serde(default)]
    pub state_change_seq: u64,
}
#[derive(Clone, Default, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(default)]
pub struct FeedQuery {
    pub search: String,
    pub session_id: Option<String>,
    pub kind: Option<String>,
    pub pinned_only: bool,
    pub archived: bool,
    pub limit: usize,
    /// Hide unavailable attachments and posts with no available resources.
    /// HTTP(S) links remain visible; remote file paths cannot be verified locally.
    pub available_only: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub discovery: crate::discovery::Config,
    #[serde(default)]
    pub discovery_error: Option<String>,
    pub sequence: i64,
    pub posts: Vec<Post>,
    pub sessions: Vec<Session>,
    pub total: usize,
    pub unread: usize,
    pub connections: Vec<ConnectionStatus>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionStatus {
    pub socket: String,
    pub connected: bool,
    pub message: String,
}

pub fn validate_post(title: &str, body: &str, resources: usize, readiness: &str) -> Result<()> {
    ensure!(
        !title.trim().is_empty() && title.chars().count() <= 240,
        "Title must contain 1–240 characters"
    );
    ensure!(body.len() <= 65_536, "Body exceeds 64 KiB");
    ensure!((1..=32).contains(&resources), "A post needs 1–32 resources");
    ensure!(
        ["creating", "ready", "failed", "unresolved"].contains(&readiness),
        "Readiness must be creating, ready, failed or unresolved"
    );
    Ok(())
}

//! Passive, bounded publishing reminders. Never reads terminal screens or posts automatically.
use crate::{
    agent_setup::{Agent, POLICY},
    broker::{Client, Paths},
    discovery::{self, FileType},
    model::*,
};
use anyhow::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

pub fn session_context(agent: Agent, input: &Value) -> Context {
    let mut context = Context::from_env();
    context.agent = agent.name().into();
    if let Some(cwd) = input["cwd"]
        .as_str()
        .filter(|cwd| Path::new(cwd).is_absolute())
    {
        context.cwd = cwd.into();
    }
    if context.herdr_socket.is_none() || context.pane_id.is_none() {
        context.herdr_socket = None;
        context.pane_id = None;
        context.workspace_id = None;
        context.tab_id = None;
        context.native_session_id = input["session_id"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 512)
            .map(str::to_owned);
        context.label = Path::new(&context.cwd)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
    }
    context
}

pub fn candidates(message: &str, context: &Context) -> Vec<Resource> {
    let lower = message.to_lowercase();
    let ready = [
        "ready",
        "created",
        "generated",
        "saved",
        "exported",
        "download",
        "preview",
        "open ",
        "rendered",
        "built",
        "report",
    ]
    .iter()
    .any(|word| lower.contains(word));
    let config = discovery::Config {
        exclude_paths: vec![
            "**/.git/**".into(),
            "**/node_modules/**".into(),
            "**/vendor/**".into(),
        ],
        ..Default::default()
    };
    discovery::extract(message, &config)
        .into_iter()
        .filter_map(|uri| Resource::resolve(ResourceInput { uri, kind: None }, context).ok())
        .filter(|resource| {
            if resource.kind == ResourceKind::Url {
                return ready
                    && url::Url::parse(&resource.target).ok().is_some_and(|url| {
                        lower.contains("preview")
                            || lower.contains("try the app")
                            || matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
                            || url.path().contains("/releases/")
                            || Path::new(url.path()).extension().is_some_and(|e| {
                                ["pdf", "png", "jpg", "jpeg", "mp4", "zip", "dmg", "deb"]
                                    .iter()
                                    .any(|s| e == *s)
                            })
                    });
            }
            if !Path::new(&resource.target).exists() {
                return false;
            }
            match FileType::of(resource) {
                FileType::Code => false,
                FileType::Markdown
                | FileType::Text
                | FileType::Folder
                | FileType::App
                | FileType::Other => ready && !resource.target.ends_with(".log"),
                FileType::Image | FileType::Video | FileType::Audio | FileType::Pdf => true,
                FileType::Web => false,
            }
        })
        .take(8)
        .collect()
}

fn is_shared(client: &Client, resource: &Resource, context: &Context) -> Result<bool> {
    let query = FeedQuery {
        search: resource.target.clone(),
        limit: 20,
        ..Default::default()
    };
    let snapshot: Snapshot = client.call_with_timeout(
        "snapshot",
        serde_json::to_value(query)?,
        Duration::from_millis(400),
    )?;
    let modified = fs::metadata(&resource.target)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64);
    Ok(snapshot.posts.iter().any(|post| {
        post.discovery.is_none()
            && post.context.host == context.host
            && (if let (Some(socket), Some(pane)) = (&context.herdr_socket, &context.pane_id) {
                post.context.herdr_socket.as_ref() == Some(socket)
                    && post.context.pane_id.as_ref() == Some(pane)
            } else if let Some(native) = &context.native_session_id {
                post.context.native_session_id.as_ref() == Some(native)
            } else {
                post.context.agent.eq_ignore_ascii_case(&context.agent)
            })
            && post.resources.iter().any(|r| r.target == resource.target)
            && modified.is_none_or(|time| time <= post.updated_at + 1000)
    }))
}

fn first_reminder(data: &Path, agent: Agent, input: &Value, missing: &[Resource]) -> Result<bool> {
    let session = input["session_id"].as_str().unwrap_or("unknown");
    let turn = input["turn_id"].as_str().unwrap_or("");
    let key = format!(
        "{}:{session}:{turn}:{:?}",
        agent.key(),
        missing.iter().map(|r| &r.target).collect::<Vec<_>>()
    );
    let hash = format!("{:x}", Sha256::digest(key.as_bytes()));
    let folder = data.join("agent-setup/reminders");
    fs::create_dir_all(&folder)?;
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o700))?;
    // Entries contain only timestamps, never transcripts or resource paths. Bound storage growth.
    let entries: Vec<_> = fs::read_dir(&folder)?
        .filter_map(Result::ok)
        .take(1025)
        .collect();
    for entry in entries.iter().filter(|entry| {
        entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(86400))
    }) {
        let _ = fs::remove_file(entry.path());
    }
    if entries.len() > 1024 {
        return Ok(false);
    }
    let path = folder.join(hash);
    if path
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(300))
    {
        return Ok(false);
    }
    if path.exists() {
        fs::remove_file(&path)?;
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(b"reminded\n")?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub fn handle(agent: Agent, event: &str, data: &Path, input: &Value) -> Result<Value> {
    let context = session_context(agent, input);
    if event == "session-start" {
        return Ok(
            json!({"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":format!("{POLICY}\nYour publishing context (supply this as context to feedr_post): {}", serde_json::to_string(&context)?)}}),
        );
    }
    if event != "stop" || input["stop_hook_active"] == true {
        return Ok(json!({}));
    }
    let message = input["last_assistant_message"].as_str().unwrap_or("");
    if message.len() > 128 * 1024 {
        return Ok(json!({}));
    }
    let resources = candidates(message, &context);
    if resources.is_empty() {
        return Ok(json!({}));
    }
    let client = Client::new(Paths::new(Some(data.to_owned()))?);
    let mut missing = Vec::new();
    for resource in resources {
        match is_shared(&client, &resource, &context) {
            Ok(true) => (),
            Ok(false) => missing.push(resource),
            // Offline or delayed Feedr never holds up a turn, and this hook never starts the service.
            Err(_) => return Ok(json!({})),
        }
    }
    if missing.is_empty() || !first_reminder(data, agent, input, &missing)? {
        return Ok(json!({}));
    }
    Ok(
        json!({"decision":"block","reason":format!("Before finishing, share these existing user-facing deliverables using Feedr MCP (feedr_post or feedr_update), then finish the task:\n{}\nIf these are only source references, or Feedr is unavailable, explain briefly and finish. Do not enable discovery or repeatedly retry.", missing.iter().map(|r| r.target.as_str()).collect::<Vec<_>>().join("\n"))}),
    )
}

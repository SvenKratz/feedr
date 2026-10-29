//! First-launch agent integration. All filesystem operations can target isolated profiles.
use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, Item, Table, value};

pub const POLICY: &str = include_str!("../integrations/agents/PUBLISHING.md");
const BEGIN: &str = "<!-- feedr:publishing:start -->";
const END: &str = "<!-- feedr:publishing:end -->";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    Claude,
    Codex,
}
impl Agent {
    pub const ALL: [Self; 2] = [Self::Claude, Self::Codex];
    pub fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub agent: Agent,
    pub root: PathBuf,
    pub config: PathBuf,
    pub instructions: PathBuf,
    pub hooks: PathBuf,
}
impl Profile {
    pub fn locate(agent: Agent, explicit: Option<PathBuf>) -> Result<Self> {
        let home = dirs::home_dir().context("Cannot find your home directory")?;
        let custom = explicit.or_else(|| {
            std::env::var_os(match agent {
                Agent::Claude => "CLAUDE_CONFIG_DIR",
                Agent::Codex => "CODEX_HOME",
            })
            .map(PathBuf::from)
        });
        let has_custom = custom.is_some();
        let root = custom.unwrap_or_else(|| {
            home.join(match agent {
                Agent::Claude => ".claude",
                Agent::Codex => ".codex",
            })
        });
        ensure!(
            root.is_absolute(),
            "Agent configuration directory must be an absolute path"
        );
        let instructions = match agent {
            Agent::Claude => root.join("CLAUDE.md"),
            Agent::Codex => {
                let override_file = root.join("AGENTS.override.md");
                if override_file.metadata().is_ok_and(|meta| meta.len() > 0) {
                    override_file
                } else {
                    root.join("AGENTS.md")
                }
            }
        };
        Ok(Self {
            agent,
            instructions,
            config: match agent {
                Agent::Codex => root.join("config.toml"),
                Agent::Claude => {
                    if has_custom {
                        root.join(".claude.json")
                    } else {
                        home.join(".claude.json")
                    }
                }
            },
            hooks: root.join(match agent {
                Agent::Claude => "settings.json",
                Agent::Codex => "hooks.json",
            }),
            root,
        })
    }
    pub fn files(&self) -> Vec<PathBuf> {
        vec![
            self.config.clone(),
            self.instructions.clone(),
            self.hooks.clone(),
        ]
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub agent: Agent,
    pub detected: bool,
    pub connection: bool,
    pub instructions: bool,
    pub reminders: bool,
    pub files: Vec<PathBuf>,
    pub issue: Option<String>,
}
impl Status {
    pub fn configured(&self) -> bool {
        self.connection && self.instructions
    }
}

fn read(path: &Path) -> Result<String> {
    match fs::metadata(path) {
        Ok(meta) => {
            ensure!(
                meta.is_file() && meta.len() <= 8 * 1024 * 1024,
                "{} is not a supported configuration file",
                path.display()
            );
            fs::read_to_string(path).with_context(|| format!("Cannot read {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ensure!(
                fs::symlink_metadata(path).is_err(),
                "{} is a broken link",
                path.display()
            );
            Ok(String::new())
        }
        Err(error) => Err(error).with_context(|| format!("Cannot inspect {}", path.display())),
    }
}
fn object(text: &str) -> Result<Value> {
    let result = if text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(text)?
    };
    ensure!(
        result.is_object(),
        "Client configuration must be a JSON object"
    );
    Ok(result)
}
fn document(text: &str) -> Result<DocumentMut> {
    text.parse::<DocumentMut>()
        .map_err(|_| anyhow::anyhow!("Invalid TOML configuration; repair it before setup"))
}
fn json_map<'a>(value: &'a mut Value, key: &str) -> Result<&'a mut serde_json::Map<String, Value>> {
    let map = value.as_object_mut().context("Expected a JSON object")?;
    map.entry(key)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("Expected a configuration object")
}
fn section<'a>(
    parent: &'a mut dyn toml_edit::TableLike,
    name: &str,
) -> Result<&'a mut dyn toml_edit::TableLike> {
    if parent.get(name).is_none() {
        parent.insert(name, Item::Table(Table::new()));
    }
    parent
        .get_mut(name)
        .and_then(Item::as_table_like_mut)
        .context("Expected a TOML configuration table")
}

fn policy_text(text: &str, enabled: bool) -> Result<String> {
    let start = text.find(BEGIN);
    let end = text.find(END);
    let block = if enabled {
        format!("{BEGIN}\n{POLICY}\n{END}\n")
    } else {
        String::new()
    };
    match (start, end) {
        (None, None) => Ok(if enabled {
            format!(
                "{text}{}{block}",
                if text.is_empty() || text.ends_with('\n') {
                    ""
                } else {
                    "\n"
                }
            )
        } else {
            text.into()
        }),
        (Some(start), Some(end)) if start < end => {
            ensure!(
                text.matches(BEGIN).count() == 1 && text.matches(END).count() == 1,
                "Multiple Feedr instruction blocks found"
            );
            ensure!(
                (start == 0 || text.as_bytes()[start - 1] == b'\n')
                    && text[start + BEGIN.len()..].starts_with('\n'),
                "Feedr instruction markers must be on their own lines"
            );
            let after = end + END.len();
            ensure!(
                after == text.len() || text[after..].starts_with('\n'),
                "Invalid Feedr instruction end marker"
            );
            Ok(format!(
                "{}{block}{}",
                &text[..start],
                text[after..].strip_prefix('\n').unwrap_or(&text[after..])
            ))
        }
        _ => anyhow::bail!("Incomplete Feedr instruction block; repair it before setup"),
    }
}

fn hook_command(exe: &Path, data: &Path, agent: Agent, event: &str) -> String {
    [
        exe.to_string_lossy().into_owned(),
        "--data-dir".into(),
        data.to_string_lossy().into_owned(),
        "agent-hook".into(),
        "--client".into(),
        agent.key().into(),
        "--event".into(),
        event.into(),
    ]
    .iter()
    .map(|s| crate::cli_install::quote(s))
    .collect::<Vec<_>>()
    .join(" ")
}
fn ours_handler(value: &Value) -> bool {
    value["statusMessage"] == "Feedr publishing check"
        && value["command"]
            .as_str()
            .is_some_and(|s| s.contains(" agent-hook "))
}
fn merge_hooks(
    value: &mut Value,
    exe: &Path,
    data: &Path,
    agent: Agent,
    enabled: bool,
) -> Result<()> {
    let map = json_map(value, "hooks")?;
    for (event, flag) in [("SessionStart", "session-start"), ("Stop", "stop")] {
        let handlers = map
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .context("Client hooks must be arrays")?;
        for group in handlers.iter_mut() {
            if let Some(hooks) = group["hooks"].as_array_mut() {
                hooks.retain(|hook| !ours_handler(hook));
            }
        }
        handlers.retain(|group| !group["hooks"].as_array().is_some_and(Vec::is_empty));
        if enabled {
            handlers.push(json!({"hooks":[{"type":"command","command":hook_command(exe,data,agent,flag),"timeout":4,"statusMessage":"Feedr publishing check"}]}));
        }
    }
    Ok(())
}

pub fn inspect(profile: &Profile, exe: &Path, data: &Path) -> Status {
    let mut status = Status {
        agent: profile.agent,
        detected: profile.root.is_dir() || profile.config.is_file(),
        connection: false,
        instructions: false,
        reminders: false,
        files: profile.files(),
        issue: None,
    };
    let result = (|| -> Result<()> {
        let config = read(&profile.config)?;
        let args = vec![
            "--data-dir".to_owned(),
            data.to_string_lossy().into_owned(),
            "mcp".into(),
        ];
        status.connection = match profile.agent {
            Agent::Claude => {
                let config = object(&config)?;
                let entry = &config["mcpServers"]["feedr"];
                entry["command"] == exe.to_string_lossy().as_ref()
                    && entry["args"] == json!(args)
                    && entry["env"]["FEEDR_AGENT"] == profile.agent.name()
            }
            Agent::Codex => {
                let doc = document(&config)?;
                let entry = doc
                    .get("mcp_servers")
                    .and_then(Item::as_table_like)
                    .and_then(|t| t.get("feedr"))
                    .and_then(Item::as_table_like);
                entry.is_some_and(|entry| {
                    entry.get("command").and_then(Item::as_str) == exe.to_str()
                        && entry.get("args").and_then(Item::as_array).is_some_and(|a| {
                            a.iter()
                                .filter_map(toml_edit::Value::as_str)
                                .eq(args.iter().map(String::as_str))
                        })
                        && entry
                            .get("env")
                            .and_then(Item::as_table_like)
                            .and_then(|env| env.get("FEEDR_AGENT"))
                            .and_then(Item::as_str)
                            == Some(profile.agent.name())
                })
            }
        };
        let instructions = read(&profile.instructions)?;
        status.instructions =
            instructions.contains(BEGIN) && policy_text(&instructions, true)? == instructions;
        let hooks = object(&read(&profile.hooks)?)?;
        status.reminders = [("SessionStart", "session-start"), ("Stop", "stop")]
            .iter()
            .all(|(event, flag)| {
                hooks["hooks"][*event].as_array().is_some_and(|handlers| {
                    handlers.iter().any(|handler| {
                        handler["hooks"].as_array().is_some_and(|hooks| {
                            hooks.iter().any(|hook| {
                                ours_handler(hook)
                                    && hook["command"]
                                        == hook_command(exe, data, profile.agent, flag)
                            })
                        })
                    })
                })
            });
        if hooks["disableAllHooks"] == true {
            status.reminders = false;
            status.issue = Some("This client has all hooks disabled. Publishing instructions are available; enable hooks in the client to use reminders.".into());
        }
        Ok(())
    })();
    if let Err(error) = result {
        status.issue = Some(format!("{error:#}"));
    }
    status
}

struct Edit {
    path: PathBuf,
    before: String,
    after: String,
}
fn edit(path: &Path, f: impl FnOnce(&str) -> Result<String>) -> Result<Edit> {
    let before = read(path)?;
    let after = f(&before).with_context(|| format!("Cannot configure {}", path.display()))?;
    let path = if fs::symlink_metadata(path).is_ok() {
        path.canonicalize()?
    } else {
        path.to_owned()
    };
    if path.exists() {
        ensure!(
            !path.metadata()?.permissions().readonly(),
            "{} is read-only",
            path.display()
        );
    }
    Ok(Edit {
        path,
        before,
        after,
    })
}
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("Invalid configuration location")?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .with_context(|| format!("Cannot save {}", path.display()))?;
    Ok(())
}

/// Merge only Feedr settings. No client permission, account or hook-trust settings change.
pub fn configure(
    profile: &Profile,
    exe: &Path,
    data: &Path,
    reminders: bool,
    enabled: bool,
) -> Result<Status> {
    ensure!(
        exe.is_absolute() && exe.is_file(),
        "Feedr executable is unavailable"
    );
    ensure!(
        !exe.starts_with("/Volumes")
            && !exe
                .components()
                .any(|part| part.as_os_str() == "AppTranslocation"),
        "Install Feedr outside the disk image before configuring agents"
    );
    crate::cli_install::validate_location(exe)?;
    let args = [
        "--data-dir".to_owned(),
        data.to_string_lossy().into_owned(),
        "mcp".into(),
    ];
    let mut edits = vec![edit(&profile.instructions, |text| {
        policy_text(text, enabled)
    })?];
    match profile.agent {
        Agent::Claude => edits.push(edit(&profile.config, |text| {
            let mut config = object(text)?;
            let servers = json_map(&mut config, "mcpServers")?;
            if enabled {
                if let Some(old) = servers.get("feedr") {
                    ensure!(
                        old.get("url").is_none(),
                        "The feedr name is used by a remote server; rename it before setup"
                    );
                }
                let entry = servers.entry("feedr").or_insert_with(|| json!({}));
                ensure!(
                    entry.is_object(),
                    "The existing feedr entry is not an object"
                );
                entry["type"] = json!("stdio");
                entry["command"] = json!(exe);
                entry["args"] = json!(args);
                json_map(entry, "env")?.insert("FEEDR_AGENT".into(), json!(profile.agent.name()));
            } else if servers
                .get("feedr")
                .is_some_and(|v| v["env"]["FEEDR_AGENT"] == profile.agent.name())
            {
                servers.remove("feedr");
            }
            Ok(serde_json::to_string_pretty(&config)? + "\n")
        })?),
        Agent::Codex => edits.push(edit(&profile.config, |text| {
            let mut doc = document(text)?;
            let servers = section(doc.as_table_mut(), "mcp_servers")?;
            if enabled {
                let entry = section(servers, "feedr")?;
                ensure!(
                    entry.get("url").is_none(),
                    "The feedr name is used by a remote server; rename it before setup"
                );
                entry.insert("command", value(exe.to_string_lossy().as_ref()));
                entry.insert(
                    "args",
                    value(
                        args.iter()
                            .map(String::as_str)
                            .collect::<toml_edit::Array>(),
                    ),
                );
                section(entry, "env")?.insert("FEEDR_AGENT", value(profile.agent.name()));
                if reminders {
                    section(doc.as_table_mut(), "features")?.insert("hooks", value(true));
                }
            } else if servers
                .get("feedr")
                .and_then(Item::as_table_like)
                .and_then(|entry| entry.get("env"))
                .and_then(Item::as_table_like)
                .and_then(|env| env.get("FEEDR_AGENT"))
                .and_then(Item::as_str)
                == Some(profile.agent.name())
            {
                servers.remove("feedr");
            }
            Ok(doc.to_string())
        })?),
    }
    edits.push(edit(&profile.hooks, |text| {
        let mut config = object(text)?;
        merge_hooks(&mut config, exe, data, profile.agent, enabled && reminders)?;
        Ok(serde_json::to_string_pretty(&config)? + "\n")
    })?);
    // Check all inputs before the first write. Back up changed files in the user's private data directory.
    let changes: Vec<_> = edits.iter().filter(|e| e.before != e.after).collect();
    let backup = data
        .join("agent-setup/backups")
        .join(crate::model::new_id());
    if !changes.is_empty() {
        fs::create_dir_all(&backup)?;
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o700))?;
    }
    for (index, e) in changes.iter().enumerate() {
        ensure!(
            read(&e.path)? == e.before,
            "{} changed during setup; try again",
            e.path.display()
        );
        if !e.before.is_empty() {
            write_private(
                &backup.join(format!("{}-{index}.backup", profile.agent.key())),
                e.before.as_bytes(),
            )?;
        }
    }
    if !changes.is_empty() {
        let manifest: Vec<_> = changes.iter().enumerate().map(|(index, e)| json!({"path":e.path,"backup":if e.before.is_empty() { None } else { Some(format!("{}-{index}.backup", profile.agent.key())) }})).collect();
        write_private(
            &backup.join("manifest.json"),
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
    }
    for (index, e) in changes.iter().enumerate() {
        let result = (|| {
            ensure!(
                read(&e.path)? == e.before,
                "{} changed during setup; try again",
                e.path.display()
            );
            write_private(&e.path, e.after.as_bytes())
        })();
        if let Err(error) = result {
            for previous in &changes[..index] {
                if read(&previous.path).is_ok_and(|text| text == previous.after) {
                    let _ = write_private(&previous.path, previous.before.as_bytes());
                }
            }
            return Err(error);
        }
    }
    Ok(inspect(profile, exe, data))
}

pub fn completed(data: &Path) -> bool {
    let path = data.join("agent-setup/onboarding.json");
    path.metadata().is_ok_and(|m| m.len() <= 1024)
        && fs::read(&path)
            .ok()
            .and_then(|text| serde_json::from_slice::<Value>(&text).ok())
            .is_some_and(|value| value["seen"] == true && value["version"] == 1)
}
pub fn finish(data: &Path) -> Result<()> {
    write_private(
        &data.join("agent-setup/onboarding.json"),
        b"{\"version\":1,\"seen\":true}\n",
    )
}

pub fn test_post(data: &Path) -> Result<crate::model::Post> {
    use crate::{
        broker::{Client, Paths},
        model::*,
    };
    fs::create_dir_all(data)?;
    let report = data.join("welcome-to-feedr.md");
    if !report.exists() {
        write_private(&report, b"# Your work, within reach\n\nYour agents can publish documents, images, builds and preview links directly to Feedr.\n\nRestart your connected agents after setup. Herdr and mention discovery are optional.\n")?;
    }
    let client = Client::new(Paths::new(Some(data.to_owned()))?);
    client.ensure_running()?;
    client.call("post", json!({"title":"Welcome to Feedr","body":"Setup test share; direct publishing works without herdr or discovery.","resources":[{"uri":report}],"idempotency_key":"feedr-setup-welcome-v1","context":{"host":local_host(),"agent":"Feedr","label":"Feedr setup","cwd":data}}))
}

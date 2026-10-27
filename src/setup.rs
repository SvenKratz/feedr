//! Stable MCP configuration shared by the desktop setup panel and CLI.
use serde_json::{Value, json};
use std::path::Path;

pub fn mcp_config(exe: &Path, data: &Path, session: Option<&str>) -> Value {
    let mut args = vec![
        "--data-dir".to_owned(),
        data.to_string_lossy().into_owned(),
        "mcp".into(),
    ];
    if let Some(session) = session {
        args.extend(["--session".into(), session.into()]);
    }
    json!({"mcpServers":{"feedr":{"command":exe.to_string_lossy(),"args":args}}})
}

pub struct McpSetup {
    pub json: String,
    pub claude: String,
    pub codex: String,
}
impl McpSetup {
    pub fn new(exe: &Path, data: &Path) -> Self {
        let command = [
            exe.to_string_lossy().into_owned(),
            "--data-dir".into(),
            data.to_string_lossy().into_owned(),
            "mcp".into(),
        ]
        .iter()
        .map(|part| crate::cli_install::quote(part))
        .collect::<Vec<_>>()
        .join(" ");
        Self {
            json: serde_json::to_string_pretty(&mcp_config(exe, data, None)).expect("JSON config"),
            claude: format!("claude mcp add --scope user --transport stdio feedr -- {command}"),
            codex: format!("codex mcp add feedr -- {command}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mcp_instructions_use_the_same_absolute_paths_and_quote_shell_arguments() {
        let exe = Path::new("/Applications/Feedr.app/Contents/MacOS/feedr");
        let data = Path::new("/Users/a 'name'/Library/Application Support/feedr");
        let setup = McpSetup::new(exe, data);
        let config: Value = serde_json::from_str(&setup.json).unwrap();
        assert_eq!(
            config["mcpServers"]["feedr"]["command"],
            exe.to_str().unwrap()
        );
        assert_eq!(
            config["mcpServers"]["feedr"]["args"][1],
            data.to_str().unwrap()
        );
        for command in [&setup.claude, &setup.codex] {
            let script = format!(
                "codex() {{ printf '%s\\n' \"$@\"; }}; claude() {{ printf '%s\\n' \"$@\"; }}; {command}"
            );
            let output = std::process::Command::new("/bin/sh")
                .args(["-c", &script])
                .output()
                .unwrap();
            assert!(output.status.success());
            let args = String::from_utf8(output.stdout).unwrap();
            assert!(args.contains(exe.to_str().unwrap()) && args.contains(data.to_str().unwrap()));
            assert!(args.ends_with("mcp\n"));
        }
    }
}

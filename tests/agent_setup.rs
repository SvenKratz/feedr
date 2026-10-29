use feedr::agent_setup::{self, Agent, Profile};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
};

fn executable(root: &Path) -> std::path::PathBuf {
    let exe = root.join("Feedr with spaces");
    fs::write(&exe, "fixture").unwrap();
    exe
}

#[test]
fn setup_merges_client_configs_and_is_repeatable_and_reversible() {
    let root = tempfile::tempdir().unwrap();
    let exe = executable(root.path());
    let data = root.path().join("data");
    for agent in Agent::ALL {
        let profile = Profile::locate(agent, Some(root.path().join(agent.key()))).unwrap();
        fs::create_dir_all(&profile.root).unwrap();
        let config = match agent {
            Agent::Codex => "# keep my preferences\nmodel = 'test-model'\n[mcp_servers.other]\ncommand = 'keep-server'\n[features]\nother_feature = true\n".to_owned(),
            Agent::Claude => r#"{"mcpServers":{"other":{"command":"keep-server"}},"accountFixture":"keep-private-value","projects":{"example":{"trust":true}}}"#.into(),
        };
        fs::write(&profile.config, &config).unwrap();
        fs::write(&profile.instructions, "Keep my original instructions.\n").unwrap();
        fs::write(&profile.hooks, r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"keep-existing-hook"}]}]},"otherSetting":true}"#).unwrap();
        let status = agent_setup::configure(&profile, &exe, &data, true, true).unwrap();
        assert!(status.configured() && status.reminders, "{status:?}");
        let first: Vec<_> = profile
            .files()
            .iter()
            .map(|p| fs::read(p).unwrap())
            .collect();
        agent_setup::configure(&profile, &exe, &data, true, true).unwrap();
        let second: Vec<_> = profile
            .files()
            .iter()
            .map(|p| fs::read(p).unwrap())
            .collect();
        assert_eq!(first, second);
        let changed = fs::read_to_string(&profile.config).unwrap();
        assert!(changed.contains("keep-server"));
        if agent == Agent::Codex {
            assert!(changed.contains("# keep my preferences") && changed.contains("test-model"));
        } else {
            assert!(changed.contains("keep-private-value"));
        }
        for file in profile.files() {
            assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        }
        let disconnected = agent_setup::configure(&profile, &exe, &data, false, false).unwrap();
        assert!(!disconnected.connection && !disconnected.instructions && !disconnected.reminders);
        assert!(
            fs::read_to_string(&profile.config)
                .unwrap()
                .contains("keep-server")
        );
        assert!(
            fs::read_to_string(&profile.hooks)
                .unwrap()
                .contains("keep-existing-hook")
        );
        assert_eq!(
            fs::read_to_string(&profile.instructions).unwrap(),
            "Keep my original instructions.\n"
        );
    }
    assert!(data.join("agent-setup/backups").is_dir());
}

#[test]
fn setup_uses_active_codex_override_and_preserves_symlinked_instructions() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("codex");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("AGENTS.md"), "Inactive guidance\n").unwrap();
    let original = root.path().join("managed-guidance.md");
    fs::write(&original, "Active guidance\n").unwrap();
    symlink(&original, folder.join("AGENTS.override.md")).unwrap();
    let profile = Profile::locate(Agent::Codex, Some(folder.clone())).unwrap();
    assert!(profile.instructions.ends_with("AGENTS.override.md"));
    agent_setup::configure(
        &profile,
        &executable(root.path()),
        &root.path().join("data"),
        false,
        true,
    )
    .unwrap();
    assert!(profile.instructions.is_symlink());
    assert!(
        fs::read_to_string(original)
            .unwrap()
            .starts_with("Active guidance\n")
    );
    assert_eq!(
        fs::read_to_string(folder.join("AGENTS.md")).unwrap(),
        "Inactive guidance\n"
    );
}

#[test]
fn malformed_config_is_rejected_before_any_file_changes_and_errors_do_not_echo_secrets() {
    let root = tempfile::tempdir().unwrap();
    let profile = Profile::locate(Agent::Codex, Some(root.path().join("codex"))).unwrap();
    fs::create_dir_all(&profile.root).unwrap();
    fs::write(&profile.config, "private_token='secret-fixture\n").unwrap();
    fs::write(&profile.instructions, "Unchanged\n").unwrap();
    let error = agent_setup::configure(
        &profile,
        &executable(root.path()),
        &root.path().join("data"),
        true,
        true,
    )
    .unwrap_err();
    assert!(!format!("{error:#}").contains("secret-fixture"));
    assert_eq!(
        fs::read_to_string(&profile.instructions).unwrap(),
        "Unchanged\n"
    );
    assert!(!profile.hooks.exists());
}

#[test]
fn codex_inline_tables_remain_supported_and_other_hooks_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let profile = Profile::locate(Agent::Codex, Some(root.path().join("codex"))).unwrap();
    fs::create_dir_all(&profile.root).unwrap();
    fs::write(&profile.config, "mcp_servers = { other = { command = 'keep' } }\nfeatures = { hooks = false, other = true }\n").unwrap();
    let result = agent_setup::configure(
        &profile,
        &executable(root.path()),
        &root.path().join("data"),
        true,
        true,
    )
    .unwrap();
    assert!(result.configured() && result.reminders);
    assert!(
        fs::read_to_string(&profile.config)
            .unwrap()
            .contains("keep")
    );
}

#[test]
fn first_launch_is_only_dismissed_by_an_explicit_completion_stamp() {
    let root = tempfile::tempdir().unwrap();
    assert!(!agent_setup::completed(root.path()));
    agent_setup::finish(root.path()).unwrap();
    assert!(agent_setup::completed(root.path()));
}

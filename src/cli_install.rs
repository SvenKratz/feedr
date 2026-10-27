//! User-level CLI installation; the link follows normal application updates.
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    fs,
    io::Write,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
};

const BEGIN: &str = "# >>> feedr CLI >>>";
const END: &str = "# <<< feedr CLI <<<";

#[derive(Debug, Serialize)]
pub struct Installed {
    pub command: PathBuf,
    pub executable: PathBuf,
    pub profiles: Vec<PathBuf>,
    pub next_step: String,
}

pub fn quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
    {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn install(bin_dir: Option<PathBuf>, modify_path: bool) -> Result<Installed> {
    let exe = std::env::current_exe()?.canonicalize()?;
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        ensure!(
            is_feedr_bundle(&exe),
            "Install the CLI from Feedr.app after copying the app to Applications"
        );
        ensure!(
            !exe.starts_with("/Volumes")
                && !exe
                    .components()
                    .any(|part| part.as_os_str() == "AppTranslocation"),
            "Copy Feedr.app to Applications and launch it there before installing the CLI"
        );
        let path = std::ffi::CString::new(exe.as_os_str().as_bytes())?;
        let mut volume = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // The buffer is initialized only on success; no pointers outlive this call.
        ensure!(
            unsafe { libc::statvfs(path.as_ptr(), volume.as_mut_ptr()) } == 0,
            "Could not inspect the app's installation volume"
        );
        let volume = unsafe { volume.assume_init() };
        ensure!(
            volume.f_flag & libc::ST_RDONLY == 0,
            "Copy Feedr.app from the disk image to Applications before installing the CLI"
        );
    }
    let home = dirs::home_dir().context("Cannot find your home directory")?;
    let bin = bin_dir.unwrap_or_else(|| home.join(".local/bin"));
    let shell = std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(target_os = "macos") {
            "/bin/zsh"
        } else {
            "/bin/bash"
        }
        .into()
    });
    let zdotdir = std::env::var_os("ZDOTDIR").map(PathBuf::from);
    let profiles = if modify_path {
        profiles(&home, &shell, zdotdir.as_deref())
    } else {
        Vec::new()
    };
    install_at(&exe, &bin, &profiles)
}

fn is_feedr_bundle(exe: &Path) -> bool {
    exe.file_name().is_some_and(|name| name == "feedr")
        && exe
            .ancestors()
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .and_then(|app| plist::Value::from_file(app.join("Contents/Info.plist")).ok())
            .and_then(|info| {
                info.as_dictionary()?
                    .get("CFBundleIdentifier")?
                    .as_string()
                    .map(str::to_owned)
            })
            .is_some_and(|id| id == "dev.feedr.desktop")
}

fn profiles(home: &Path, shell: &str, zdotdir: Option<&Path>) -> Vec<PathBuf> {
    match Path::new(shell).file_name().and_then(|name| name.to_str()) {
        Some("zsh") => {
            let root = zdotdir.filter(|p| p.is_absolute()).unwrap_or(home);
            vec![root.join(".zprofile"), root.join(".zshrc")]
        }
        Some("bash") => {
            let login = [".bash_profile", ".bash_login", ".profile"]
                .into_iter()
                .map(|name| home.join(name))
                .find(|p| p.exists())
                .unwrap_or_else(|| {
                    home.join(if cfg!(target_os = "macos") {
                        ".bash_profile"
                    } else {
                        ".profile"
                    })
                });
            vec![login, home.join(".bashrc")]
        }
        Some("sh" | "dash" | "ksh") => vec![home.join(".profile")],
        _ => Vec::new(),
    }
}

fn path_block(bin: &Path) -> Result<String> {
    let bin = bin.to_str().context("CLI path must be valid UTF-8")?;
    ensure!(
        !bin.contains([':', '\n', '\r']),
        "CLI directory cannot contain a colon or newline"
    );
    let bin = quote(bin);
    Ok(format!(
        "{BEGIN}\ncase \":${{PATH-}}:\" in\n  *:{bin}:*) ;;\n  *) export PATH={bin}\"${{PATH:+:$PATH}}\" ;;\nesac\n{END}\n"
    ))
}

fn edit_profile(text: &str, block: &str) -> Result<String> {
    let start = text.find(BEGIN);
    let end = text.find(END);
    match (start, end) {
        (None, None) => Ok(format!(
            "{text}{}{block}",
            if text.is_empty() || text.ends_with('\n') {
                ""
            } else {
                "\n"
            }
        )),
        (Some(start), Some(end)) if start < end => {
            ensure!(
                text.matches(BEGIN).count() == 1 && text.matches(END).count() == 1,
                "Multiple Feedr PATH blocks found; leave one complete block before retrying"
            );
            ensure!(
                (start == 0 || text.as_bytes()[start - 1] == b'\n')
                    && text[start + BEGIN.len()..].starts_with('\n')
                    && text.as_bytes()[end - 1] == b'\n',
                "Feedr PATH markers must be on separate lines"
            );
            let after = end + END.len();
            let suffix = &text[after..];
            ensure!(
                suffix.is_empty() || suffix.starts_with('\n'),
                "Invalid Feedr PATH end marker"
            );
            Ok(format!(
                "{}{block}{}",
                &text[..start],
                suffix.strip_prefix('\n').unwrap_or(suffix)
            ))
        }
        _ => anyhow::bail!("Incomplete Feedr PATH block; remove it before retrying"),
    }
}

fn install_at(exe: &Path, bin: &Path, profiles: &[PathBuf]) -> Result<Installed> {
    ensure!(
        exe.is_file() && exe.metadata()?.permissions().mode() & 0o111 != 0,
        "Feedr executable is unavailable"
    );
    ensure!(bin.is_absolute(), "CLI directory must be an absolute path");
    let block = path_block(bin)?;
    let command = bin.join("feedr");
    let existing = fs::symlink_metadata(&command).ok();
    if let Some(meta) = existing.as_ref() {
        ensure!(
            meta.file_type().is_symlink(),
            "{} already exists; an unrelated command will not be overwritten",
            command.display()
        );
        let target = fs::read_link(&command)?;
        let target = if target.is_absolute() {
            target
        } else {
            bin.join(target)
        };
        ensure!(
            target == exe || is_feedr_bundle(&target),
            "{} points to another command; move that link or choose --bin-dir",
            command.display()
        );
    }
    // Validate every profile before changing the command or any shell configuration.
    let mut edits = Vec::new();
    for profile in profiles {
        let actual = if profile.symlink_metadata().is_ok() {
            profile.canonicalize()?
        } else {
            profile.clone()
        };
        let text = if actual.exists() {
            ensure!(
                actual.is_file() && !actual.metadata()?.permissions().readonly(),
                "{} is not a writable profile",
                profile.display()
            );
            ensure!(
                actual.metadata()?.len() <= 1024 * 1024,
                "Shell profile exceeds 1 MiB"
            );
            fs::read_to_string(&actual)?
        } else {
            String::new()
        };
        let updated = edit_profile(&text, &block)
            .with_context(|| format!("Cannot update {}", profile.display()))?;
        if updated != text {
            edits.push((actual, updated));
        }
    }
    fs::create_dir_all(bin)?;
    if existing.is_some() {
        let temporary = tempfile::Builder::new()
            .prefix(".feedr-cli-")
            .tempdir_in(bin)?;
        let staged = temporary.path().join("feedr");
        symlink(exe, &staged)?;
        fs::rename(staged, &command)?;
    } else {
        symlink(exe, &command)?;
    }
    for (profile, text) in edits {
        let parent = profile.parent().context("Invalid shell profile")?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        if let Ok(meta) = profile.metadata() {
            temporary.as_file().set_permissions(meta.permissions())?;
        }
        temporary.write_all(text.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary.persist(&profile).with_context(|| {
            format!(
                "CLI installed at {}, but PATH could not be saved to {}",
                command.display(),
                profile.display()
            )
        })?;
    }
    Ok(Installed {
        command,
        executable: exe.to_owned(),
        profiles: profiles.to_vec(),
        next_step: if profiles.is_empty() {
            format!(
                "Add {} to your shell's PATH, then run feedr --help",
                bin.display()
            )
        } else {
            "Open a new terminal, then run feedr --help".into()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn install_is_repeatable_preserves_profiles_and_follows_app_replacement() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("Feedr.app/Contents/MacOS/feedr");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, "#!/bin/sh\necho old\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let bin = root.path().join("a 'quoted' $ bin");
        let dotfile = root.path().join("dotfiles/zshrc");
        fs::create_dir_all(dotfile.parent().unwrap()).unwrap();
        fs::write(&dotfile, "# keep this\nexport EDITOR=vi\n").unwrap();
        let profile = root.path().join(".zshrc");
        symlink(&dotfile, &profile).unwrap();
        let result = install_at(&exe, &bin, std::slice::from_ref(&profile)).unwrap();
        let first = fs::read_to_string(&dotfile).unwrap();
        install_at(&exe, &bin, std::slice::from_ref(&profile)).unwrap();
        assert_eq!(first, fs::read_to_string(&dotfile).unwrap());
        assert!(profile.is_symlink() && first.starts_with("# keep this\nexport EDITOR=vi\n"));
        assert_eq!(first.matches(BEGIN).count(), 1);
        let script = format!("{first}\n{first}\nfeedr");
        let output = Command::new("/bin/sh")
            .env("PATH", "/usr/bin:/bin")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"old\n");
        fs::write(&exe, "#!/bin/sh\necho updated\n").unwrap();
        assert_eq!(
            Command::new(&result.command).output().unwrap().stdout,
            b"updated\n"
        );
    }

    #[test]
    fn unrelated_commands_and_invalid_profiles_are_not_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("source");
        fs::write(&exe, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let target = bin.join("feedr");
        fs::write(&target, "someone else's command").unwrap();
        assert!(install_at(&exe, &bin, &[]).is_err());
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "someone else's command"
        );
        fs::remove_file(&target).unwrap();
        symlink(&exe, &target).unwrap();
        let profile = root.path().join(".zshrc");
        fs::write(&profile, format!("# preserve\n{BEGIN}\n")).unwrap();
        assert!(install_at(&exe, &bin, std::slice::from_ref(&profile)).is_err());
        assert_eq!(
            fs::read_to_string(&profile).unwrap(),
            format!("# preserve\n{BEGIN}\n")
        );
        fs::remove_file(&target).unwrap();
        symlink("/bin/sh", &target).unwrap();
        assert!(install_at(&exe, &bin, &[]).is_err());
        assert_eq!(fs::read_link(&target).unwrap(), Path::new("/bin/sh"));
    }
}

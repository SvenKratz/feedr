use super::{Github, Release, command_output};
use crate::broker::{Client, Paths};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    fs,
    io::Read,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub version: String,
    pub destination: PathBuf,
    pub staged: PathBuf,
    pub backup: PathBuf,
    pub bundle: bool,
    pub data_dir: PathBuf,
    pub parent_pid: u32,
    pub path: PathBuf,
}
impl Plan {
    pub fn discard(&self) {
        if self.validate().is_ok()
            && let Some(root) = self.path.parent()
            && root
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".feedr-update-"))
        {
            let _ = fs::remove_dir_all(root);
        }
    }
    fn executable(&self, staged: bool) -> PathBuf {
        executable(
            if staged {
                &self.staged
            } else {
                &self.destination
            },
            self.bundle,
        )
    }
    fn validate(&self) -> Result<()> {
        semver::Version::parse(&self.version)?;
        ensure!(
            self.destination.is_absolute()
                && self.staged.is_absolute()
                && self.data_dir.is_absolute(),
            "Update locations must be absolute"
        );
        ensure!(
            self.destination.exists() && self.staged.exists(),
            "Installed or staged app is missing"
        );
        let parent = self
            .destination
            .parent()
            .context("Missing installation folder")?
            .canonicalize()?;
        ensure!(
            self.staged
                .parent()
                .and_then(Path::parent)
                .context("Missing staging folder")?
                .canonicalize()?
                == parent,
            "Update must be staged on the installation's filesystem"
        );
        ensure!(
            self.staged != self.destination && self.parent_pid > 1,
            "Invalid update plan"
        );
        ensure!(
            self.path.parent() == self.staged.parent(),
            "Update plan is outside its staging folder"
        );
        if self.bundle {
            ensure!(
                self.backup == self.staged,
                "Invalid bundle rollback location"
            );
        } else {
            ensure!(
                self.backup.parent() == self.staged.parent() && self.backup != self.staged,
                "Invalid rollback location"
            );
        }
        Ok(())
    }
}

fn executable(path: &Path, bundle: bool) -> PathBuf {
    if bundle {
        path.join("Contents/MacOS/feedr")
    } else {
        path.to_owned()
    }
}

pub fn destination(exe: &Path) -> Result<(PathBuf, bool)> {
    if cfg!(target_os = "macos") {
        let bundle = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).context("Automatic installation requires a Feedr.app bundle; this is a development executable")?;
        let info = plist::Value::from_file(bundle.join("Contents/Info.plist"))?;
        ensure!(
            info.as_dictionary()
                .and_then(|v| v.get("CFBundleIdentifier"))
                .and_then(plist::Value::as_string)
                == Some("dev.feedr.desktop"),
            "This app bundle is not a Feedr installation"
        );
        Ok((bundle.to_owned(), true))
    } else {
        ensure!(cfg!(target_os = "linux"), "Unsupported update platform");
        ensure!(
            !package_managed(exe),
            "This installation is managed by the system package manager. Download the latest .deb from GitHub and install it with apt; Feedr will not replace /usr/bin files."
        );
        ensure!(
            !exe.ancestors()
                .any(|p| p.file_name().is_some_and(|n| n == "target")),
            "Build-directory executables are not replaced automatically; install Feedr to a user-writable location first"
        );
        Ok((exe.to_owned(), false))
    }
}

pub fn package_managed(exe: &Path) -> bool {
    cfg!(target_os = "linux") && (exe.starts_with("/usr/bin") || exe.starts_with("/bin"))
}

pub fn prepare(
    github: &Github,
    release: &Release,
    exe: &Path,
    data_dir: &Path,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, u64),
) -> Result<Plan> {
    let asset = release.asset(std::env::consts::OS, std::env::consts::ARCH)?;
    let (destination, bundle) = destination(exe)?;
    let parent = destination
        .parent()
        .context("Missing installation folder")?;
    let stage = tempfile::Builder::new().prefix(".feedr-update-").tempdir_in(parent).context("The app's installation folder is not writable. Move Feedr to a user-writable location to enable automatic installation.")?;
    let archive = stage.path().join(&asset.name);
    github.download(asset, &archive, cancel, progress)?;
    ensure!(
        !cancel.load(std::sync::atomic::Ordering::Relaxed),
        "Update cancelled"
    );
    let version = release.version()?.to_string();
    let candidate = unpack_and_verify(&archive, stage.path(), &version, bundle)?;
    fs::remove_file(archive)?;
    let plan = Plan {
        version,
        destination,
        staged: candidate.clone(),
        backup: if bundle {
            candidate
        } else {
            stage.path().join("previous-feedr")
        },
        bundle,
        data_dir: data_dir.to_owned(),
        parent_pid: std::process::id(),
        path: stage.path().join("plan.json"),
    };
    plan.validate()?;
    fs::write(&plan.path, serde_json::to_vec(&plan)?)?;
    fs::set_permissions(&plan.path, fs::Permissions::from_mode(0o600))?;
    let _ = stage.keep();
    Ok(plan)
}

fn unpack_and_verify(archive: &Path, root: &Path, version: &str, bundle: bool) -> Result<PathBuf> {
    extract(archive, root, bundle)?;
    let candidate = if bundle {
        root.join("Feedr.app")
    } else {
        root.join("feedr")
    };
    let binary = executable(&candidate, bundle);
    ensure!(
        binary.is_file(),
        "Release package does not contain the Feedr executable"
    );
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
    if bundle {
        let info = plist::Value::from_file(candidate.join("Contents/Info.plist"))?;
        ensure!(
            info.as_dictionary()
                .and_then(|v| v.get("CFBundleIdentifier"))
                .and_then(plist::Value::as_string)
                == Some("dev.feedr.desktop"),
            "Release bundle identity does not match Feedr"
        );
        command_output(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--deep", "--strict"])
                .arg(&candidate),
            Duration::from_secs(30),
            64 * 1024,
        )
        .context("Release app signature verification failed")?;
    }
    let reported = command_output(
        Command::new(&binary).arg("--version"),
        Duration::from_secs(10),
        1024,
    )?;
    ensure!(
        String::from_utf8_lossy(&reported).trim() == format!("feedr {version}"),
        "Release executable version does not match its tag"
    );
    Ok(candidate)
}

pub fn verify_package(archive: &Path) -> Result<()> {
    let version = semver::Version::parse(env!("CARGO_PKG_VERSION"))?;
    let expected = super::asset_name(&version, std::env::consts::OS, std::env::consts::ARCH)?;
    ensure!(
        archive
            .file_name()
            .is_some_and(|name| name == expected.as_str()),
        "Package name does not match the current version and platform"
    );
    let root = tempfile::tempdir()?;
    unpack_and_verify(
        archive,
        root.path(),
        &version.to_string(),
        cfg!(target_os = "macos"),
    )?;
    Ok(())
}

fn safe_path(root: &Path, name: &Path) -> Result<PathBuf> {
    ensure!(
        !name.as_os_str().is_empty()
            && name
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "Unsafe path in update package"
    );
    Ok(root.join(name))
}
fn write_entry(
    root: &Path,
    name: &Path,
    directory: bool,
    size: u64,
    reader: &mut impl Read,
    remaining: &mut u64,
) -> Result<()> {
    let path = safe_path(root, name)?;
    ensure!(
        size <= *remaining,
        "Expanded update package exceeds its size limit"
    );
    if directory {
        fs::create_dir_all(path)?;
        return Ok(());
    }
    fs::create_dir_all(path.parent().context("Invalid archive entry")?)?;
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?;
    let actual = std::io::copy(&mut reader.take(size + 1), &mut file)?;
    ensure!(actual == size, "Truncated update archive entry");
    *remaining -= actual;
    Ok(())
}
fn extract(archive: &Path, root: &Path, bundle: bool) -> Result<()> {
    let mut remaining = 1024 * 1024 * 1024;
    if bundle {
        let mut zip = zip::ZipArchive::new(fs::File::open(archive)?)?;
        ensure!(zip.len() <= 16_384, "Too many entries in update archive");
        for i in 0..zip.len() {
            let mut file = zip.by_index(i)?;
            let kind = file.unix_mode().unwrap_or(0) & 0o170000;
            ensure!(
                matches!(kind, 0 | 0o100000 | 0o040000),
                "Links and special files are not allowed in update archives"
            );
            let name = file.enclosed_name().context("Unsafe ZIP entry")?;
            write_entry(
                root,
                &name,
                file.is_dir(),
                file.size(),
                &mut file,
                &mut remaining,
            )?;
        }
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(archive)?));
        for (i, entry) in tar.entries()?.enumerate() {
            ensure!(i < 16_384, "Too many entries in update archive");
            let mut entry = entry?;
            let kind = entry.header().entry_type();
            ensure!(
                kind.is_file() || kind.is_dir(),
                "Links and special files are not allowed in update archives"
            );
            let name = entry.path()?.into_owned();
            write_entry(
                root,
                &name,
                kind.is_dir(),
                entry.size(),
                &mut entry,
                &mut remaining,
            )?;
        }
    }
    Ok(())
}

pub fn launch(plan: &Plan) -> Result<()> {
    plan.validate()?;
    let log = plan.data_dir.join("updates");
    fs::create_dir_all(&log)?;
    let log = fs::File::create(log.join("install.log"))?;
    let mut child = Command::new(plan.executable(true))
        .arg("update-apply")
        .arg("--plan")
        .arg(&plan.path)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()?;
    let ready = plan.path.with_extension("ready");
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if ready.is_file() {
            return Ok(());
        }
        if child.try_wait()?.is_some() {
            bail!("Update installer could not start. See updates/install.log.");
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Update installer did not become ready");
        }
        thread::sleep(Duration::from_millis(30));
    }
}

fn running(pid: u32) -> bool {
    // SAFETY: signal zero only tests process existence; no signal is delivered.
    unsafe {
        libc::kill(pid as libc::pid_t, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}
fn stop_broker(data_dir: &Path) -> Result<()> {
    let client = Client::new(Paths::new(Some(data_dir.to_owned()))?);
    if client.call::<serde_json::Value>("ping", json!({})).is_ok() {
        client
            .call::<serde_json::Value>("shutdown", json!({}))
            .context("The running feed service cannot shut down for an update")?;
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while client.call::<serde_json::Value>("ping", json!({})).is_ok() {
        ensure!(
            Instant::now() < deadline,
            "Feed service did not stop; the installed app was kept"
        );
        thread::sleep(Duration::from_millis(60));
    }
    // A disconnected socket alone does not prove all requests have drained.
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(data_dir.join("broker.lock"))?;
    while fs2::FileExt::try_lock_exclusive(&lock).is_err() {
        ensure!(
            Instant::now() < deadline,
            "Feed service did not finish its current requests"
        );
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn swap_bundle(a: &Path, b: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let a = std::ffi::CString::new(a.as_os_str().as_bytes())?;
        let b = std::ffi::CString::new(b.as_os_str().as_bytes())?;
        // SAFETY: both C strings remain alive; RENAME_SWAP atomically exchanges
        // directory entries without overwriting a running executable's bytes.
        let result = unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (a, b);
        bail!("macOS bundle updates are unsupported on this platform")
    }
}
fn replace(plan: &Plan) -> Result<()> {
    if plan.bundle {
        swap_bundle(&plan.destination, &plan.staged)
    } else {
        fs::copy(&plan.destination, &plan.backup)?;
        fs::rename(&plan.staged, &plan.destination)?;
        Ok(())
    }
}
fn rollback(plan: &Plan) -> Result<()> {
    if plan.bundle {
        swap_bundle(&plan.destination, &plan.backup)
    } else {
        fs::rename(&plan.backup, &plan.destination)?;
        Ok(())
    }
}
fn start_gui(plan: &Plan, health: Option<&str>) -> Result<std::process::Child> {
    let mut command = Command::new(plan.executable(false));
    command
        .args(["--data-dir"])
        .arg(&plan.data_dir)
        .arg("gui")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    command.env_remove("FEEDR_UPDATE_HEALTH_ID");
    if let Some(health) = health {
        command.env("FEEDR_UPDATE_HEALTH_ID", health);
    }
    Ok(command.spawn()?)
}

pub fn apply(path: &Path) -> Result<()> {
    ensure!(
        fs::metadata(path)?.len() < 64 * 1024,
        "Update plan exceeds its size limit"
    );
    let plan: Plan = serde_json::from_slice(&fs::read(path)?)?;
    plan.validate()?;
    ensure!(
        plan.parent_pid != std::process::id(),
        "Installer must run separately from the app"
    );
    fs::write(path.with_extension("ready"), b"ready")?;
    let deadline = Instant::now() + Duration::from_secs(120);
    while running(plan.parent_pid) {
        ensure!(
            Instant::now() < deadline,
            "App did not close; the installed version was kept"
        );
        thread::sleep(Duration::from_millis(100));
    }
    let mut replaced = false;
    let preferred = plan.data_dir.join("updates/executable.json");
    let previous_executable = fs::read(&preferred)
        .ok()
        .filter(|bytes| bytes.len() < 64 * 1024);
    let result = (|| -> Result<()> {
        let guard =
            crate::broker::installation_lock(&Paths::new(Some(plan.data_dir.clone()))?, true)?;
        stop_broker(&plan.data_dir)?;
        replace(&plan)?;
        replaced = true;
        fs::write(
            &preferred,
            serde_json::to_vec(&json!({"path":plan.executable(false),"version":plan.version}))?,
        )?;
        drop(guard);
        let health = uuid::Uuid::new_v4().to_string();
        let marker = plan
            .data_dir
            .join("updates")
            .join(format!("health-{health}"));
        let mut child = start_gui(&plan, Some(&health))?;
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            if fs::read_to_string(&marker).is_ok_and(|v| v == plan.version) {
                let _ = fs::remove_file(marker);
                return Ok(());
            }
            if child.try_wait()?.is_some() {
                bail!("Updated app exited before becoming ready");
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                bail!("Updated app did not become ready");
            }
            thread::sleep(Duration::from_millis(100));
        }
    })();
    if let Err(error) = &result {
        if replaced {
            let _guard =
                crate::broker::installation_lock(&Paths::new(Some(plan.data_dir.clone()))?, true)?;
            let _ = stop_broker(&plan.data_dir);
            rollback(&plan).context(
                "Update failed and rollback also failed; backup is in the staging folder",
            )?;
            if let Some(bytes) = &previous_executable {
                fs::write(&preferred, bytes)?;
            } else {
                let _ = fs::remove_file(&preferred);
            }
        }
        fs::write(
            plan.data_dir.join("updates/status.json"),
            serde_json::to_vec(
                &json!({"version":plan.version,"error":format!("{error:#}"),"restored":true}),
            )?,
        )?;
        let _ = start_gui(&plan, None);
    } else {
        fs::write(
            plan.data_dir.join("updates/status.json"),
            serde_json::to_vec(&json!({"version":plan.version,"error":null,"backup":plan.backup}))?,
        )?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[test]
    fn debian_installations_are_never_replaced_by_the_portable_updater() {
        for path in ["/usr/bin/feedr", "/bin/feedr"] {
            assert!(
                destination(Path::new(path))
                    .unwrap_err()
                    .to_string()
                    .contains("apt")
            );
        }
        let portable = Path::new("/home/test/.local/bin/feedr");
        assert_eq!(destination(portable).unwrap(), (portable.to_owned(), false));
    }
    #[test]
    fn installer_waits_for_parent_and_rolls_back_a_build_that_cannot_start() {
        for succeeds in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let stage = root.path().join(".feedr-update-test");
            fs::create_dir(&stage).unwrap();
            fs::create_dir(root.path().join("updates")).unwrap();
            let dest = root.path().join("feedr");
            let old = b"#!/bin/sh\nprintf old > \"$2/old-started\"\n";
            fs::write(&dest, old).unwrap();
            fs::set_permissions(&dest, fs::Permissions::from_mode(0o755)).unwrap();
            let candidate = stage.join("feedr");
            let new = if succeeds {
                "#!/bin/sh\nprintf 0.2.0 > \"$2/updates/health-$FEEDR_UPDATE_HEALTH_ID\"\n"
            } else {
                "#!/bin/sh\nexit 1\n"
            };
            fs::write(&candidate, new).unwrap();
            fs::set_permissions(&candidate, fs::Permissions::from_mode(0o755)).unwrap();
            let mut parent = Command::new("sleep").arg("0.2").spawn().unwrap();
            let parent_pid = parent.id();
            let parent = thread::spawn(move || parent.wait().unwrap());
            let plan = Plan {
                version: "0.2.0".into(),
                destination: dest.clone(),
                staged: candidate,
                backup: stage.join("previous-feedr"),
                bundle: false,
                data_dir: root.path().to_owned(),
                parent_pid,
                path: stage.join("plan.json"),
            };
            fs::write(&plan.path, serde_json::to_vec(&plan).unwrap()).unwrap();
            let started = Instant::now();
            let result = apply(&plan.path);
            parent.join().unwrap();
            assert!(started.elapsed() >= Duration::from_millis(150));
            assert_eq!(result.is_ok(), succeeds, "{result:?}");
            if succeeds {
                assert_eq!(fs::read(&dest).unwrap(), new.as_bytes());
            } else {
                assert_eq!(fs::read(&dest).unwrap(), old);
            }
            let status: serde_json::Value =
                serde_json::from_slice(&fs::read(root.path().join("updates/status.json")).unwrap())
                    .unwrap();
            assert_eq!(status["error"].is_null(), succeeds);
            // Let the tiny fallback launcher finish before its temporary folder disappears.
            thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn package_extraction_rejects_symbolic_links() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("update.tar.gz");
        let gzip = flate2::write::GzEncoder::new(
            fs::File::create(&archive).unwrap(),
            flate2::Compression::default(),
        );
        let mut tar = tar::Builder::new(gzip);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_link_name("/tmp").unwrap();
        header.set_cksum();
        tar.append_data(&mut header, "feedr", &b""[..]).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        let output = root.path().join("output");
        fs::create_dir(&output).unwrap();
        assert!(extract(&archive, &output, false).is_err());
        assert!(!output.join("feedr").exists());
    }
    #[test]
    fn archive_paths_cannot_escape_and_duplicate_or_oversized_entries_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["../outside", "/absolute", "folder/../../outside"] {
            assert!(safe_path(dir.path(), Path::new(name)).is_err());
        }
        let mut budget = 3;
        write_entry(
            dir.path(),
            Path::new("feedr"),
            false,
            3,
            &mut &b"new"[..],
            &mut budget,
        )
        .unwrap();
        assert_eq!(budget, 0);
        assert!(
            write_entry(
                dir.path(),
                Path::new("other"),
                false,
                1,
                &mut &b"x"[..],
                &mut budget
            )
            .is_err()
        );
        assert!(
            write_entry(
                dir.path(),
                Path::new("feedr"),
                false,
                0,
                &mut &b""[..],
                &mut budget
            )
            .is_err()
        );
    }
    #[test]
    fn atomic_binary_replacement_can_restore_the_previous_build() {
        let root = tempfile::tempdir().unwrap();
        let stage = root.path().join(".feedr-update-test");
        fs::create_dir(&stage).unwrap();
        let dest = root.path().join("feedr");
        fs::write(&dest, b"old").unwrap();
        let candidate = stage.join("feedr");
        fs::write(&candidate, b"new").unwrap();
        let plan = Plan {
            version: "0.2.0".into(),
            destination: dest.clone(),
            staged: candidate,
            backup: stage.join("previous-feedr"),
            bundle: false,
            data_dir: root.path().to_owned(),
            parent_pid: 42,
            path: stage.join("plan.json"),
        };
        plan.validate().unwrap();
        replace(&plan).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"new");
        rollback(&plan).unwrap();
        assert_eq!(fs::read(dest).unwrap(), b"old");
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn bundle_exchange_is_atomic_and_reversible() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("old.app");
        let b = root.path().join("new.app");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::write(a.join("version"), b"old").unwrap();
        fs::write(b.join("version"), b"new").unwrap();
        swap_bundle(&a, &b).unwrap();
        assert_eq!(fs::read(a.join("version")).unwrap(), b"new");
        swap_bundle(&a, &b).unwrap();
        assert_eq!(fs::read(a.join("version")).unwrap(), b"old");
    }
}

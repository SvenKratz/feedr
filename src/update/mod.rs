//! GitHub release metadata and verified platform packages. Network work stays off the UI thread.
mod install;
pub use install::{Plan, apply, destination, launch, prepare, verify_package};

use anyhow::{Context, Result, bail, ensure};
use reqwest::blocking::{Client, Response};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

pub const REPOSITORY: &str = "SvenKratz/feedr";
pub const RELEASES_URL: &str = "https://github.com/SvenKratz/feedr/releases";
const API: &str = "https://api.github.com/repos/SvenKratz/feedr";
const MAX_PACKAGE: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Asset {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub digest: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub published_at: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}
impl Release {
    pub fn version(&self) -> Result<Version> {
        Ok(Version::parse(
            self.tag_name.strip_prefix('v').unwrap_or(&self.tag_name),
        )?)
    }
    pub fn is_newer_than(&self, current: &str) -> Result<bool> {
        let version = self.version()?;
        Ok(!self.draft
            && !self.prerelease
            && version.pre.is_empty()
            && version > Version::parse(current)?)
    }
    pub fn asset(&self, os: &str, arch: &str) -> Result<&Asset> {
        let name = asset_name(&self.version()?, os, arch)?;
        let matches: Vec<_> = self
            .assets
            .iter()
            .filter(|asset| asset.name == name)
            .collect();
        ensure!(
            matches.len() == 1,
            "No unique {os}/{arch} update package is published for {}",
            self.tag_name
        );
        ensure!(
            matches[0].size > 0 && matches[0].size <= MAX_PACKAGE,
            "Update package exceeds the download limit"
        );
        digest(matches[0])?;
        Ok(matches[0])
    }
}

pub fn asset_name(version: &Version, os: &str, arch: &str) -> Result<String> {
    ensure!(
        matches!(arch, "aarch64" | "x86_64"),
        "Automatic updates are not packaged for this CPU"
    );
    let (platform, extension) = match os {
        "macos" => ("macos", "zip"),
        "linux" => ("linux", "tar.gz"),
        _ => bail!("Automatic updates are available for macOS and Linux"),
    };
    Ok(format!("feedr-v{version}-{platform}-{arch}.{extension}"))
}

fn digest(asset: &Asset) -> Result<String> {
    let hash = asset
        .digest
        .as_deref()
        .and_then(|value| value.strip_prefix("sha256:"))
        .context("GitHub has not published a SHA-256 digest for this package")?;
    ensure!(
        hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Invalid package digest from GitHub"
    );
    Ok(hash.to_ascii_lowercase())
}

pub struct Github {
    client: Client,
    token: Option<String>,
}
impl Github {
    pub fn new() -> Result<Self> {
        let token = command_output(
            Command::new(crate::media::tool("gh")).args([
                "auth",
                "token",
                "--hostname",
                "github.com",
            ]),
            Duration::from_secs(5),
            16 * 1024,
        )
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
        let client = Client::builder()
            .user_agent(concat!("feedr/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(180))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                let allowed = matches!(
                    attempt.url().host_str(),
                    Some(
                        "api.github.com"
                            | "github.com"
                            | "release-assets.githubusercontent.com"
                            | "objects.githubusercontent.com"
                            | "github-releases.githubusercontent.com"
                    )
                );
                if attempt.previous().len() < 5 && attempt.url().scheme() == "https" && allowed {
                    attempt.follow()
                } else {
                    attempt.error("Untrusted update redirect")
                }
            }))
            .build()?;
        Ok(Self { client, token })
    }
    fn get(&self, endpoint: &str, accept: &str) -> Result<Response> {
        let mut request = self
            .client
            .get(format!("{API}{endpoint}"))
            .header("Accept", accept)
            .header("X-GitHub-Api-Version", "2026-03-10")
            .timeout(Duration::from_secs(
                if accept == "application/octet-stream" {
                    180
                } else {
                    20
                },
            ));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        // Do not surface response bodies or signed redirect URLs in error messages.
        request.send().map_err(|_| {
            anyhow::anyhow!("Could not reach GitHub. Check your connection and try again.")
        })
    }
    pub fn latest(&self) -> Result<Option<Release>> {
        let response = self.get("/releases/latest", "application/vnd.github+json")?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            let repo = self.get("", "application/vnd.github+json")?;
            ensure!(
                repo.status().is_success(),
                "GitHub releases are inaccessible. For a private repository, sign in with gh auth login and check again."
            );
            return Ok(None);
        }
        ensure!(
            response.status().is_success(),
            "GitHub release check returned HTTP {}",
            response.status().as_u16()
        );
        let mut bytes = vec![];
        response.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= 2 * 1024 * 1024,
            "GitHub release metadata exceeds its size limit"
        );
        let release: Release = serde_json::from_slice(&bytes)?;
        release
            .version()
            .context("Latest release does not have a semantic version tag")?;
        Ok(Some(release))
    }
    pub fn download(
        &self,
        asset: &Asset,
        path: &Path,
        cancel: &AtomicBool,
        progress: impl FnMut(u64, u64),
    ) -> Result<()> {
        digest(asset)?;
        let mut response = self.get(
            &format!("/releases/assets/{}", asset.id),
            "application/octet-stream",
        )?;
        ensure!(
            response.status().is_success(),
            "Package download returned HTTP {}",
            response.status().as_u16()
        );
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        verify_download(asset, &mut response, &mut file, cancel, progress)?;
        file.sync_all()?;
        Ok(())
    }
}

fn verify_download(
    asset: &Asset,
    reader: &mut impl Read,
    file: &mut impl Write,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<()> {
    let expected = digest(asset)?;
    ensure!(
        asset.size > 0 && asset.size <= MAX_PACKAGE,
        "Invalid update package size"
    );
    let mut hash = Sha256::new();
    let mut total = 0;
    let mut buffer = [0_u8; 64 * 1024];
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    loop {
        ensure!(!cancel.load(Ordering::Relaxed), "Update download cancelled");
        let count = reader.read(&mut buffer).map_err(|_| {
            anyhow::anyhow!("Update download was interrupted; your installed app is unchanged.")
        })?;
        if count == 0 {
            break;
        }
        total += count as u64;
        ensure!(
            total <= asset.size && total <= MAX_PACKAGE,
            "Update package exceeds the advertised size"
        );
        hash.update(&buffer[..count]);
        file.write_all(&buffer[..count])?;
        if last_progress.elapsed() > Duration::from_millis(100) {
            progress(total, asset.size);
            last_progress = Instant::now();
        }
    }
    ensure!(total == asset.size, "Update package download is incomplete");
    ensure!(
        format!("{:x}", hash.finalize()) == expected,
        "Update checksum mismatch; the package will not be installed"
    );
    progress(total, asset.size);
    Ok(())
}

// Capture bounded output in memory: credentials never hit disk, argv or logs.
pub(crate) fn command_output(
    command: &mut Command,
    timeout: Duration,
    limit: u64,
) -> Result<Vec<u8>> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let output = child.stdout.take().context("Missing helper output pipe")?;
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = output
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Update helper exceeded its time limit");
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let bytes = rx
        .recv_timeout(Duration::from_secs(1))
        .context("Update helper did not close its output")??;
    ensure!(
        status.success() && bytes.len() as u64 <= limit,
        "Update helper failed or exceeded its output limit"
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn downloads_reject_corruption_truncation_extra_bytes_and_cancellation() {
        let bytes = b"verified release package";
        let asset = Asset {
            id: 1,
            name: "update.zip".into(),
            size: bytes.len() as u64,
            digest: Some(format!("sha256:{:x}", Sha256::digest(bytes))),
        };
        let cancel = AtomicBool::new(false);
        let check = |mut input: &[u8], cancel: &AtomicBool| {
            verify_download(&asset, &mut input, &mut vec![], cancel, |_, _| {})
        };
        assert!(check(bytes, &cancel).is_ok());
        assert!(check(&bytes[..bytes.len() - 1], &cancel).is_err());
        assert!(check(b"corrupted release packag", &cancel).is_err());
        assert!(check(&[bytes.as_slice(), b"extra"].concat(), &cancel).is_err());
        assert!(check(bytes, &AtomicBool::new(true)).is_err());
    }
    fn release(tag: &str) -> Release {
        serde_json::from_value(serde_json::json!({"tag_name":tag,"name":null,"body":"Notes","published_at":null,"assets":[]})).unwrap()
    }
    #[test]
    fn compares_semantic_versions_and_never_installs_drafts_or_prereleases() {
        assert!(release("v0.10.0").is_newer_than("0.9.0").unwrap());
        assert!(!release("v0.9.0").is_newer_than("0.10.0").unwrap());
        assert!(!release("v0.10.0-beta.1").is_newer_than("0.9.0").unwrap());
        assert!(!release("v0.1.0").is_newer_than("0.1.0").unwrap());
        let mut draft = release("v2.0.0");
        draft.draft = true;
        assert!(!draft.is_newer_than("0.1.0").unwrap());
        assert!(release("latest").is_newer_than("0.1.0").is_err());
    }
    #[test]
    fn only_selects_one_exact_platform_package_with_a_published_digest() {
        let mut r = release("v0.2.0");
        r.assets = vec![Asset {
            id: 1,
            name: "feedr-v0.2.0-macos-aarch64.zip".into(),
            size: 10,
            digest: Some(format!("sha256:{}", "ab".repeat(32))),
        }];
        assert_eq!(r.asset("macos", "aarch64").unwrap().id, 1);
        assert!(r.asset("linux", "x86_64").is_err());
        r.assets.push(r.assets[0].clone());
        assert!(r.asset("macos", "aarch64").is_err());
        r.assets.pop();
        r.assets[0].digest = None;
        assert!(r.asset("macos", "aarch64").is_err());
    }
}

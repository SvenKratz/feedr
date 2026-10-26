//! Native decoders run away from the UI and never involve the feed broker.
pub mod pdf;
pub mod video;

use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

pub fn tool(name: &str) -> PathBuf {
    // Finder launches do not inherit a shell's Homebrew PATH.
    for prefix in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        let path = Path::new(prefix).join(name);
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from(name)
}

pub fn output(command: &mut Command, cancel: &AtomicBool, max_bytes: u64) -> Result<Vec<u8>> {
    let dir = tempfile::tempdir()?;
    let out = dir.path().join("output");
    let err = dir.path().join("error");
    let mut child = command
        .stdin(Stdio::null())
        .stdout(fs::File::create(&out)?)
        .stderr(fs::File::create(&err)?)
        .env("LC_ALL", "C")
        .spawn()
        .context("Media backend unavailable. Install poppler and mpv (macOS: brew install poppler mpv; Linux: poppler-utils, libmpv2, ffmpeg)")?;
    let deadline = Instant::now() + Duration::from_secs(25);
    let status = loop {
        if cancel.load(Ordering::Relaxed)
            || Instant::now() > deadline
            || fs::metadata(&out)?.len() > max_bytes
            || fs::metadata(&err)?.len() > 128 * 1024
        {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Preview cancelled or exceeded its time/memory limit");
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        thread::sleep(Duration::from_millis(15));
    };
    ensure!(
        fs::metadata(&out)?.len() <= max_bytes,
        "Preview exceeds size limit"
    );
    if !status.success() {
        let message = fs::read_to_string(&err).unwrap_or_default();
        bail!("{}", message.chars().take(1800).collect::<String>().trim());
    }
    Ok(fs::read(out)?)
}

pub fn decode(bytes: &[u8]) -> Result<eframe::egui::ColorImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    reader.limits(limits);
    let image = reader.decode()?.to_rgba8();
    Ok(eframe::egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    ))
}

pub fn poster(path: &Path, width: u32, cancel: &AtomicBool) -> Result<eframe::egui::ColorImage> {
    let bytes = output(
        Command::new(tool("ffmpeg"))
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(path)
            .args(["-frames:v", "1", "-vf"])
            .arg(format!(
                "scale={}:{}:force_original_aspect_ratio=decrease",
                width.min(1920),
                width.min(1920)
            ))
            .args(["-f", "image2pipe", "-vcodec", "png", "pipe:1"]),
        cancel,
        16 * 1024 * 1024,
    )?;
    decode(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_helper_is_reaped_without_waiting_for_its_decode_timeout() {
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let cancel = thread::spawn(move || {
            thread::sleep(Duration::from_millis(80));
            signal.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        assert!(output(Command::new("sleep").arg("5"), &cancelled, 1024).is_err());
        cancel.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}

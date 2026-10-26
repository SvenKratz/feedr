use super::{decode, output, tool};
use anyhow::{Result, ensure};
use eframe::egui::ColorImage;
use std::{path::Path, process::Command, sync::atomic::AtomicBool};

pub fn pages(path: &Path, cancel: &AtomicBool) -> Result<usize> {
    ensure!(path.is_absolute(), "PDF location is unresolved");
    ensure!(
        std::fs::metadata(path)?.len() <= 512 * 1024 * 1024,
        "PDF exceeds the 512 MiB preview limit; open externally"
    );
    let bytes = output(Command::new(tool("pdfinfo")).arg(path), cancel, 128 * 1024)?;
    let info = String::from_utf8_lossy(&bytes);
    let pages = info
        .lines()
        .find_map(|line| line.strip_prefix("Pages:")?.trim().parse::<usize>().ok());
    let pages = pages.ok_or_else(|| {
        anyhow::anyhow!("PDF page count is unavailable; the document may be encrypted or corrupt")
    })?;
    ensure!(
        pages > 0 && pages <= 100_000,
        "PDF page count is outside the supported range"
    );
    Ok(pages)
}

pub fn render(path: &Path, page: usize, size: u32, cancel: &AtomicBool) -> Result<ColorImage> {
    ensure!(
        path.is_absolute() && page > 0,
        "PDF location or page is invalid"
    );
    let bytes = output(
        Command::new(tool("pdftoppm"))
            .args([
                "-f",
                &page.to_string(),
                "-l",
                &page.to_string(),
                "-singlefile",
                "-scale-to",
                &size.clamp(100, 3200).to_string(),
                "-png",
            ])
            .arg(path),
        cancel,
        32 * 1024 * 1024,
    )?;
    decode(&bytes)
}

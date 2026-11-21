Feedr 0.1.12 speeds up multi-column filtering and adds automatic history retention.

- Filter changes refresh all open columns in one request. Feedr checks file availability in the background and shows matching cached rows while the new counts and pages arrive.
- Under **feedr → Settings → Keep feed history**, choose **Forever**, **1 year**, **6 months**, **90 days**, **30 days**, or **14 days**. The default is Forever. A finite policy removes expired unpinned cards, including archived cards, when applied, at startup, and hourly. Age is measured from the original post date. Pinned cards and the linked files are kept.
- The **HTML** filter includes local `.html` and `.htm` files and web links. Existing posts and saved Web filter selections carry over.
- Selecting a pane in herdr highlights its session and scrolls its open column into view. This works with split panes that share a name and with mention discovery disabled.

### Updating

On macOS and portable installations, the running app checks GitHub automatically. Open **?** and use the version button to check immediately. Feedr verifies the platform archive and restarts after installing the update. For existing Debian installations, download the new `.deb` and install it through apt.

### Downloads

| Platform | Installer |
|---|---|
| macOS 14+, Apple Silicon | `feedr-v0.1.12-macos-aarch64.dmg` |
| macOS 14+, Intel | `feedr-v0.1.12-macos-x86_64.dmg` |
| Debian 12+ / Ubuntu 24.04+, Intel/AMD 64-bit | `feedr_0.1.12_amd64.deb` |
| Debian 12+ / Ubuntu 24.04+, ARM 64-bit | `feedr_0.1.12_arm64.deb` |
| Source | `feedr-v0.1.12-src.tar.gz` or `feedr-v0.1.12-src.zip` |

**macOS** — Drag Feedr from the DMG into Applications. For PDF and video previews, install the media dependencies with `brew install poppler mpv ffmpeg`. The DMGs use ad-hoc signatures. Gatekeeper may block them because they lack Developer ID signing and notarization. Use **feedr → Install CLI…** to add the terminal command.

**Linux** — Install with `sudo apt install ./feedr_0.1.12_amd64.deb` or use the arm64 package. Apt resolves desktop and media dependencies. Launch from the application menu or run `feedr gui`. Release testing covers X11 under Xvfb. Wayland coverage is pending real-desktop testing.

Portable updater archives, source archives, dependency notices, and SHA-256 checksums accompany the installers. Building from source requires Rust and public crate-registry access. The telemetry documentation covers the anonymous install event, opt-in daily statistics, and how to disable both. [Telemetry documentation](https://github.com/SvenKratz/feedr/blob/v0.1.12/TELEMETRY.md)

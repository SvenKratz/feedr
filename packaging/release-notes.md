Feedr 0.1.11 makes it easier to find the session you just selected and improves readability in Light mode.

- Click a session in the sidebar to open its feed or move the existing column to the far left. Feedr scrolls it into view immediately. With **Newest left** enabled, the selected session stays first while the other columns follow activity.
- Agent Setup has a clearer **Enable publishing** button and more legible labels, focus indicators, and disabled controls. The Light theme also improves contrast in the feed, menus, dialogs, and preview controls.
- Feed filters wrap when the preview narrows the window, keeping **Newest left** and the file-type buttons accessible.

### Updating

On macOS and portable installations, the running app checks GitHub automatically. Open **?** and use the version button to check immediately. Feedr verifies the platform archive and restarts after installing the update. For existing Debian installations, download the new `.deb` and install it through apt.

### Downloads

| Platform | Installer |
|---|---|
| macOS 14+, Apple Silicon | `feedr-v0.1.11-macos-aarch64.dmg` |
| macOS 14+, Intel | `feedr-v0.1.11-macos-x86_64.dmg` |
| Debian 12+ / Ubuntu 24.04+, Intel/AMD 64-bit | `feedr_0.1.11_amd64.deb` |
| Debian 12+ / Ubuntu 24.04+, ARM 64-bit | `feedr_0.1.11_arm64.deb` |
| Source | `feedr-v0.1.11-src.tar.gz` or `feedr-v0.1.11-src.zip` |

**macOS** — Drag Feedr from the DMG into Applications. For PDF and video previews, install the media dependencies with `brew install poppler mpv ffmpeg`. The DMGs use ad-hoc signatures. Gatekeeper may block them because they lack Developer ID signing and notarization. Use **feedr → Install CLI…** to add the terminal command.

**Linux** — Install with `sudo apt install ./feedr_0.1.11_amd64.deb` or use the arm64 package. Apt resolves desktop and media dependencies. Launch from the application menu or run `feedr gui`. Release testing covers X11 under Xvfb. Wayland behavior on a real desktop remains unverified.

Portable updater archives, source archives, dependency notices, and SHA-256 checksums accompany the installers. Building from source requires Rust and public crate-registry access. See the [telemetry documentation](https://github.com/SvenKratz/feedr/blob/v0.1.11/TELEMETRY.md) for the anonymous install event, opt-in daily statistics, and how to disable both.

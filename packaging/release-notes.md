Feedr 0.1.2 helps agents share useful work from the first launch, with or without herdr.

- **Agent setup:** choose Claude Code, Codex or both and enable publishing. Feedr configures MCP and a persistent global rule to share finished reports, media, builds, output folders and preview links before announcing them. Restart the selected clients afterward.
- **Optional completion reminders:** a bounded check of the client's final assistant message reminds once about an existing deliverable that has not been shared or has changed since publication. It uses native session context, never scrolls panes, never auto-posts, and lets tasks finish if Feedr is offline. Codex requires reviewing and trusting the hooks in `/hooks` after restart.
- **Standalone publishing:** herdr and discovery are optional. Setup includes a test share, manual MCP snippets, private backups, and per-client disconnect; unrelated settings and instructions are preserved.

The policy guides model behavior; semantic importance is not guaranteed. The completion check uses deterministic file/link heuristics and does not read full transcripts. Existing users see setup once after upgrading and can return with **?** or **feedr → Agent setup…**.

### Downloads and installation

| Platform | Installer |
|---|---|
| macOS 14+, Apple Silicon | `feedr-v0.1.2-macos-aarch64.dmg` |
| macOS 14+, Intel | `feedr-v0.1.2-macos-x86_64.dmg` |
| Debian 12+ / Ubuntu 24.04+, Intel/AMD 64-bit | `feedr_0.1.2_amd64.deb` |
| Debian 12+ / Ubuntu 24.04+, ARM 64-bit | `feedr_0.1.2_arm64.deb` |
| Source | `feedr-v0.1.2-src.tar.gz` or `feedr-v0.1.2-src.zip` |

**macOS:** open the DMG and drag Feedr to Applications. To add the terminal command, launch the app and choose **feedr → Install CLI…**, then open a new terminal. Follow agent setup on first launch; return with **?** anytime. Install the separate PDF/video runtimes with `brew install poppler mpv ffmpeg`. Images and Markdown work without them. These DMGs are **ad-hoc signed, without Developer ID signing or notarization**; Gatekeeper may block them. Source builds are available if you prefer not to authorize an unnotarized download.

**Linux:** install the downloaded package with `sudo apt install ./feedr_0.1.2_amd64.deb` (use `arm64` on ARM). Apt installs the media and desktop dependencies. Launch Feedr from the application menu or `feedr gui`. X11 startup is smoke-tested under Xvfb; Wayland desktop behavior and hotkeys need testing on real desktops. Ctrl+Alt+F is available on X11; use the launcher on Wayland.

ZIP/tar archives remain available for portable installs and the automatic updater. Debian installations check GitHub for release notes and availability; install newer `.deb` files with apt so the package database stays consistent. Removing the app/package leaves your feed history intact.

Every downloadable asset has a `.sha256` sidecar; `SHA256SUMS` covers all ten packages. Rust dependency notices and licenses are included in the binary packages. Media runtimes are separate dependencies with their own licenses. Source archives contain the tracked release source and `Cargo.lock`, excluding local artifacts and user data; building requires Rust and access to the public crate registry.

### Preview limitations

Remote artifact transfer and a public viewer extension API are still planned. Local paths are live references, not copies; missing files are hidden from the feed by default and remain in history. Discovery is off by default. This is an early release; keep backups of important artifacts.

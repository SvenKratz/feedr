Feedr's first public preview: agent artifacts in a compact native desktop feed.

- Session-colored, rearrangeable feed columns with search, pins, unread state and saved history.
- Image and rendered Markdown previews; paginated PDFs; embedded video with audio and seeking.
- Compact viewer controls, window-filling image/video previews, and native open/reveal actions.
- Agent publishing over MCP or CLI, plus optional deterministic mention discovery with file-type filters.
- GitHub release notes and verified automatic updates for app bundles and portable installations.

### Downloads and installation

| Platform | Installer |
|---|---|
| macOS, Apple Silicon | `feedr-v0.1.0-macos-aarch64.dmg` |
| macOS, Intel | `feedr-v0.1.0-macos-x86_64.dmg` |
| Debian 12+ / Ubuntu 24.04+, Intel/AMD 64-bit | `feedr_0.1.0_amd64.deb` |
| Debian 12+ / Ubuntu 24.04+, ARM 64-bit | `feedr_0.1.0_arm64.deb` |
| Source | `feedr-v0.1.0-src.tar.gz` or `feedr-v0.1.0-src.zip` |

**macOS:** open the DMG and drag Feedr to Applications. Install the separate PDF/video runtimes with `brew install poppler mpv ffmpeg`. Images and Markdown work without them. These first DMGs are **ad-hoc signed, without Developer ID signing or notarization**; Gatekeeper may block them. Source builds are available if you prefer not to authorize an unnotarized download.

**Linux:** install the downloaded package with `sudo apt install ./feedr_0.1.0_amd64.deb` (use `arm64` on ARM). Apt installs the media and desktop dependencies. Launch Feedr from the application menu or `feedr gui`. X11 startup is smoke-tested under Xvfb; Wayland desktop behavior and hotkeys need testing on real desktops. Ctrl+Alt+F is available on X11; use the launcher on Wayland.

ZIP/tar archives remain available for portable installs and the automatic updater. Debian installations check GitHub for release notes and availability; install newer `.deb` files with apt so the package database stays consistent. Removing the app/package leaves your feed history intact.

Every downloadable asset has a `.sha256` sidecar; `SHA256SUMS` covers all ten packages. Rust dependency notices and licenses are included in the binary packages. Media runtimes are separate dependencies with their own licenses. Source archives contain the tracked release source and `Cargo.lock`, excluding local artifacts and user data; building requires Rust and access to the public crate registry.

### Preview limitations

Remote artifact transfer and a public viewer extension API are still planned. Local paths are live references, not copies; missing files are hidden from the feed by default and remain in history. Discovery is off by default. This is an early release; keep backups of important artifacts.

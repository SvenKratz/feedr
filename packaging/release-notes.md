Feedr 0.1.8 fixes live feed updates and makes compact artifact rows easier to scan.

- New images and other artifacts appear in every matching open column while an older preview stays open. No need to click **All** again. Failed requests retry automatically, and all filters reconcile every two seconds to catch delayed files and missed notifications.
- Image thumbnails and sidebar previews refresh after atomic file replacement, even when the modification time is preserved. File creation, removal and recreation update availability without changing the stored post.
- Session visits highlight new resources since the previous visit. Clicking a session reveals its latest items; background refreshes do not consume the next update's highlights. Visit boundaries survive restarts.
- Audio files have a music-note icon and an **Audio** filter, including saved history after upgrading; video rows use a play-frame icon or a play badge over their thumbnail. Long filenames and paths shorten from the beginning to keep the useful filename ending and extension visible.
- The sidebar starts directly with agent sessions. The unused All resources / Pinned / Archived navigation has been removed; restore the catchall column with **+ → All sessions**.
- Added live-feed and thumbnail regressions for idle-window wakeups, selected previews, MCP and discovered images, session isolation, bursts, late attachments, incomplete images, queue saturation, query races and reconnects. The local macOS suite passes 113 Rust tests plus CLI/MCP/discovery/restart and media smoke checks; release jobs separately validate both macOS and Linux architectures before publication.

### Usage statistics

This release also includes the previously unpublished 0.1.7 usage-statistics changes. Each installation sends one anonymous install event with a random install ID, version, OS, architecture and installation type. Further daily usage statistics are opt-in. Titles, paths, URLs, file contents and host names are never sent. `feedr telemetry status|enable|disable` controls the choice; `DO_NOT_TRACK=1` or `FEEDR_TELEMETRY=0` disables everything, including the install count. Builds from source without a configured PostHog project token send nothing. See [TELEMETRY.md](https://github.com/SvenKratz/feedr/blob/v0.1.8/TELEMETRY.md) for every event and field.

### Downloads and installation

| Platform | Installer |
|---|---|
| macOS 14+, Apple Silicon | `feedr-v0.1.8-macos-aarch64.dmg` |
| macOS 14+, Intel | `feedr-v0.1.8-macos-x86_64.dmg` |
| Debian 12+ / Ubuntu 24.04+, Intel/AMD 64-bit | `feedr_0.1.8_amd64.deb` |
| Debian 12+ / Ubuntu 24.04+, ARM 64-bit | `feedr_0.1.8_arm64.deb` |
| Source | `feedr-v0.1.8-src.tar.gz` or `feedr-v0.1.8-src.zip` |

**macOS:** open the DMG and drag Feedr to Applications. To add the terminal command, launch the app and choose **feedr → Install CLI…**, then open a new terminal. Follow agent setup on first launch; return with **?** anytime. Install the separate PDF/video runtimes with `brew install poppler mpv ffmpeg`. Images and Markdown work without them. These DMGs are **ad-hoc signed, without Developer ID signing or notarization**; Gatekeeper may block them. Source builds are available if you prefer not to authorize an unnotarized download.

**Linux:** install the downloaded package with `sudo apt install ./feedr_0.1.8_amd64.deb` (use `arm64` on ARM). Apt installs the media and desktop dependencies. Launch Feedr from the application menu or `feedr gui`. X11 startup is smoke-tested under Xvfb; Wayland desktop behavior and hotkeys need testing on real desktops. Ctrl+Alt+F is available on X11; use the launcher on Wayland.

ZIP/tar archives remain available for portable installs and the automatic updater. Debian installations check GitHub for release notes and availability; install newer `.deb` files with apt so the package database stays consistent. Removing the app/package leaves your feed history intact.

Every downloadable asset has a `.sha256` sidecar; `SHA256SUMS` covers all ten packages. Rust dependency notices and licenses are included in the binary packages. Media runtimes are separate dependencies with their own licenses. Source archives contain the tracked release source and `Cargo.lock`, excluding local artifacts and user data; building requires Rust and access to the public crate registry.

### Preview limitations

Remote artifact transfer and a public viewer extension API are still planned. Local paths are live references, not copies; missing files are hidden from the feed by default and remain in history. Discovery is off by default. This is an early release; keep backups of important artifacts.

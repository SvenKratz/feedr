Feedr 0.1.7 adds anonymous usage statistics, so the maintainer can see how many installations are in use and which features matter.

- Each installation sends one anonymous `feedr_installed` event the first time the app or the local feed service runs, including once after updating: a random install ID, Feedr version, OS, CPU architecture and install type. Nothing else is sent unless you opt in.
- Daily usage statistics are opt-in. Feedr asks once in **Set up Feedr**; closing the window without ticking **Share anonymous usage statistics** declines. They contain a daily active marker and counts such as posts received per agent client, attachment types and feed actions. Titles, paths, URLs, file contents and host names are never sent.
- `feedr telemetry status|enable|disable` shows or changes the choice. `DO_NOT_TRACK=1` or `FEEDR_TELEMETRY=0` turns everything off, including the install count. Events go directly to PostHog Cloud with person profiles and GeoIP lookup disabled; builds from source never send anything. [TELEMETRY.md](https://github.com/SvenKratz/feedr/blob/main/TELEMETRY.md) lists every event and field.

### Downloads and installation

| Platform | Installer |
|---|---|
| macOS 14+, Apple Silicon | `feedr-v0.1.7-macos-aarch64.dmg` |
| macOS 14+, Intel | `feedr-v0.1.7-macos-x86_64.dmg` |
| Debian 12+ / Ubuntu 24.04+, Intel/AMD 64-bit | `feedr_0.1.7_amd64.deb` |
| Debian 12+ / Ubuntu 24.04+, ARM 64-bit | `feedr_0.1.7_arm64.deb` |
| Source | `feedr-v0.1.7-src.tar.gz` or `feedr-v0.1.7-src.zip` |

**macOS:** open the DMG and drag Feedr to Applications. To add the terminal command, launch the app and choose **feedr → Install CLI…**, then open a new terminal. Follow agent setup on first launch; return with **?** anytime. Install the separate PDF/video runtimes with `brew install poppler mpv ffmpeg`. Images and Markdown work without them. These DMGs are **ad-hoc signed, without Developer ID signing or notarization**; Gatekeeper may block them. Source builds are available if you prefer not to authorize an unnotarized download.

**Linux:** install the downloaded package with `sudo apt install ./feedr_0.1.7_amd64.deb` (use `arm64` on ARM). Apt installs the media and desktop dependencies. Launch Feedr from the application menu or `feedr gui`. X11 startup is smoke-tested under Xvfb; Wayland desktop behavior and hotkeys need testing on real desktops. Ctrl+Alt+F is available on X11; use the launcher on Wayland.

ZIP/tar archives remain available for portable installs and the automatic updater. Debian installations check GitHub for release notes and availability; install newer `.deb` files with apt so the package database stays consistent. Removing the app/package leaves your feed history intact.

Every downloadable asset has a `.sha256` sidecar; `SHA256SUMS` covers all ten packages. Rust dependency notices and licenses are included in the binary packages. Media runtimes are separate dependencies with their own licenses. Source archives contain the tracked release source and `Cargo.lock`, excluding local artifacts and user data; building requires Rust and access to the public crate registry.

### Preview limitations

Remote artifact transfer and a public viewer extension API are still planned. Local paths are live references, not copies; missing files are hidden from the feed by default and remain in history. Discovery is off by default. This is an early release; keep backups of important artifacts.

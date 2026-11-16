Feedr 0.1.10 publishes the Newest left column ordering and combined resource filters that were previously available only in the local development build.

- **Newest left:** turn on the feed-toolbar toggle to move the most recently active open session column to the left. New publications, discovered resources and observed agent activity update the order. All sessions stays on the right. Each column retains its vertical scroll and selected preview. Turn the toggle off to restore your manual order; dragging a tab takes over from automatic ordering using the visible arrangement. The preference survives restarts.
- **Combine resource types:** select Image and PDF together, or any combination of type buttons. Click a selected type to remove it; All clears the selection, and removing the last selected type returns to All. Choices persist between launches. Matching attachments, column queries, counts, pagination and keyboard navigation use the combined selection. Existing single-type MCP queries remain compatible.
- Discovered resources and files becoming ready now update durable session activity, so newly available artifacts participate in column ordering without reads, pins or duplicate delivery faking recency.
- New posts now use one publication timestamp for creation, modification and session recency, avoiding inconsistent activity ordering at millisecond boundaries.
- Updated the README with a dedicated macOS dependency command and a current-UI screenshot using synthetic demo sessions and the Feedr synthwave hero. No personal session data is included.
- Added coverage for automatic/manual ordering, drag takeover, saved preferences, live Image/PDF arrivals across columns, grouped attachments, availability-aware pagination, legacy queries and real MCP multi-type requests. The local macOS suite passes 123 Rust tests; the release pipeline additionally gates publication on stable Rust checks and all four native installer jobs.

### Updating from 0.1.9

Newest left and combined type filters are new in **0.1.10**; the published 0.1.9 packages do not contain them. This release uses a new semantic version so existing updaters can discover it. The previously published 0.1.9 assets are unchanged.

On macOS and portable installations, leave automatic updates enabled or use the version button to check GitHub immediately. Feedr downloads the exact platform package, verifies its published digest, and restarts through a helper with startup verification and rollback. The app must be running for automatic checks. Debian installations show availability and release notes; install the new .deb through apt so the package database stays consistent.

### Downloads and installation

| Platform | Installer |
|---|---|
| macOS 14+, Apple Silicon | `feedr-v0.1.10-macos-aarch64.dmg` |
| macOS 14+, Intel | `feedr-v0.1.10-macos-x86_64.dmg` |
| Debian 12+ / Ubuntu 24.04+, Intel/AMD 64-bit | `feedr_0.1.10_amd64.deb` |
| Debian 12+ / Ubuntu 24.04+, ARM 64-bit | `feedr_0.1.10_arm64.deb` |
| Source | `feedr-v0.1.10-src.tar.gz` or `feedr-v0.1.10-src.zip` |

**macOS:** open the DMG and drag Feedr to Applications. Images and Markdown work without additional runtimes. Install the separate PDF/video dependencies with:

```sh
brew install poppler mpv ffmpeg
```

The DMGs are **ad-hoc signed, without Developer ID signing or notarization**; Gatekeeper may block them. Source builds are available if you prefer not to authorize an unnotarized download. To add the terminal command, use **feedr → Install CLI…** after installation. Agent setup is available on first launch and under **?**; herdr and mention discovery are optional.

**Linux:** install with `sudo apt install ./feedr_0.1.10_amd64.deb` (use `arm64` on ARM). Apt resolves the desktop and media dependencies. Launch from the application menu or run `feedr gui`. Installed CLI/MCP/media checks and X11 GUI startup under Xvfb are release gates; real Wayland desktop behavior remains unverified. Ctrl+Alt+F is available on X11; use the launcher on Wayland.

ZIP/tar updater packages and source archives are provided alongside both platform installers. Every package has a .sha256 sidecar, and SHA256SUMS covers all ten packages. Binary distributions include dependency notices. Native media runtimes retain their own licenses. Removing the app/package preserves feed history. Source archives contain tracked release source and Cargo.lock, exclude local artifacts/user data, and require Rust and public crate-registry access to build.

### Usage statistics and preview limitations

Each installation sends one anonymous install event with a random install ID, version, OS, architecture and install type. Further daily statistics are opt-in. Titles, paths, URLs, contents, session names and host names are never sent. `DO_NOT_TRACK=1` or `FEEDR_TELEMETRY=0` disables everything, including the install count. Source builds without a configured PostHog project token send nothing. See [TELEMETRY.md](https://github.com/SvenKratz/feedr/blob/v0.1.10/TELEMETRY.md) for the full schema.

This is a public preview. Remote artifact transfer and a public viewer extension API remain planned. Paths reference live files rather than copies; missing files are hidden by default and retained in history. Mention discovery is off by default.

# Feedr

A dark, terminal-inspired native Rust desktop companion to [herdr](https://herdr.dev). Feedr puts agent deliverables in compact, session-colored feed columns so you can preview images, video, PDFs and Markdown, open links, and reveal files without copying paths from a terminal.

This local testing build includes a persistent SQLite-backed service, live herdr sessions, MCP/CLI publishing, search and filters, pins/archive/read state, built-in image/Markdown/PDF/video previews, native open/reveal actions, and optional deterministic mention discovery. Remote artifact transfer, tray notifications and the public viewer contribution API are still planned. macOS is tested locally; Linux runtime verification remains outstanding.

Agents choose what to publish through MCP or the CLI. Optional mention discovery can also surface resources from completed output, using file-type filters and passive viewport reads that leave the agent's scroll position alone.

## Run

Rust 1.88 or newer is required. On macOS:

```sh
brew install poppler mpv
cargo run -- gui
```

Build a clickable local Mac application:

```sh
sh scripts/package-macos.sh
open dist/Feedr.app
```

The local .app is ad-hoc signed for testing, not notarized for distribution. `sh scripts/package-macos.sh release` produces an optimized build.

On Debian/Ubuntu, install native build dependencies first:

```sh
sudo apt-get install build-essential pkg-config libxkbcommon-dev libxcb-shape0-dev libxcb-xfixes0-dev libegl1-mesa-dev libwayland-dev poppler-utils libmpv2 ffmpeg
cargo run -- gui
```

Linux GUI runtime testing is tracked separately from macOS validation; the CI workflow is configured to run builds, tests and backend smoke checks on both platforms. `packaging/feedr.desktop` is supplied for Linux desktop installation.

Create four clearly labeled demo posts and open the app:

```sh
cargo run -- demo
```

Demo posts are deduplicated and stored locally. To keep demo/testing data separate, use `cargo run -- --data-dir /tmp/feedr-demo demo`.

## Interacting with resources

The feed uses dense, neutral artifact rows. Images, PDF first pages and video posters have small thumbnails; Markdown shows a one-line document excerpt. Click the artifact to open the full preview in Feedr's shared sidebar. Images support fit, zoom, drag-to-pan, and optional full-resolution loading within decode limits. Double-click an image or video preview, or choose **Expand**, to fill the Feedr window. **Back to feed**, Escape, or another double-click returns to the same sidebar attachment. Expanded images fit the available window and retain zoom/pan controls; video reuses the same player and playback position. Markdown uses native styled text and resolves local images relative to the document. Uncheck **Render Markdown** to see source; this preference also applies to feed excerpts and persists between normal launches. **Copy source** copies the original Markdown.

PDFs have page navigation, a page number control, zoom/pan, and a lazy thumbnail strip. The cache holds at most nine rendered pages; switching resources cancels PDF helpers. Video starts paused inside Feedr and provides play/pause, seeking, duration, volume, and audio. Closing the sidebar or switching resources stops that player. Source changes invalidate previews without adding duplicate cards.

Files, folders, applications and URLs appear as compact icon rows. Their primary click opens the resource in its default application; folders open in the file manager and URLs in the default browser. Use **Open externally** for media previews when you want another application, **Inspect** for other resources' sidebar details, or **Show in folder** to reveal the location. Use the **···** row menu or right-click an artifact for secondary actions. Keep, Pin and Archive apply to the whole post; its title and attachment position appear in that menu and in hover details. Each attachment has its own primary target.

Inline previews load in the background for visible resources, refresh when the file changes, and use a bounded thumbnail cache.

The desktop feed shows available resources by default. Local file, folder and application paths must exist; missing attachments are held out of the view, and a post with no available resources is hidden. Checks run before pagination and counts, and refresh in the background every two seconds so files can appear after creation and disappear after removal. The complete posts remain stored. Use **View → Show unavailable files** to inspect those entries; this preference is saved. Web links remain visible without fetching them. Remote file paths cannot be verified locally and appear only when unavailable files are included. CLI/MCP queries retain their existing complete-history default; use `feedr list --available-only` or `available_only: true` for the same availability filter.

Sessions have stable colors from a muted mineral palette, shared by their sidebar entries, feed-column backgrounds and tab headers. Artifact rows stay neutral. Selecting a row highlights its session and brings it into view without filtering the feed. Right-click a session to choose its color; choices are remembered between launches. **Use automatic color** restores its default.

The session sidebar has **Priority** and **Grouped** views, matching herdr. Priority orders live agents by blocked, done, working, idle, then unknown, with newest state changes first within each status. Grouped follows herdr's workspace and pane order under workspace headings. Saved sessions appear after live agents in either view. The selected view is remembered; switching views keeps the current feed filter and card selection.

Feeds can sit side by side in independently scrolling columns. Right-click a session and choose **Add Feed Column**, or click a session to open/focus its column. The **+** above the feeds opens session columns and restores the default **All sessions** catchall. Close any column with **×**, including the last one; this changes only the layout. Drag a tab handle to reorder columns. Session tabs show the live herdr name and agent, with the full identity on hover. Open columns and their order survive normal restarts.

Search, resource-type filters, Pinned and Archived apply across the open columns. Each column fetches and paginates its own results, so a busy catchall cannot crowd out a quieter session. Click a tab or resource row to focus that column for J/K and arrow-key navigation. Navigation visits each attachment in order and brings it into view. Filenames occupy one line, with a preview excerpt or location beneath and compact unread/pinned/discovery indicators. Post counts live in the tab header; hover for unread counts. Media opens in the shared sidebar.

## Native media dependencies

The Rust UI embeds video through libmpv's software render API; audio is handled by mpv. PDF pages render through isolated Poppler helpers (`pdfinfo`/`pdftoppm`). Video posters use FFmpeg. On macOS these are installed by `brew install poppler mpv`; on Debian/Ubuntu use `poppler-utils libmpv2 ffmpeg` (`libmpv1` on older distributions). Homebrew locations are searched explicitly so launching from Finder works without a shell PATH. The local `.app` requires these installed runtimes; they are not bundled or notarized for distribution yet. Upstream mpv/Poppler distributions retain their own licenses.

| Media | macOS native backend checks | Linux |
|---|---|---|
| PDF, including multi-page navigation | Poppler rendered first/last pages; corrupt files report an error | Implementation and CI checks configured; runtime unverified |
| MP4 / MOV: H.264 + AAC | Frames, selected audio track, seek and pause passed | Runtime unverified |
| WebM: VP9 + Opus | Frames, selected audio track, seek and pause passed | Runtime unverified |
| MKV: H.264 + Opus | Frames, selected audio track, seek and pause passed | Runtime unverified |

PDFs over 512 MiB and more than 100,000 pages are rejected for preview. Encrypted PDFs requiring a password report the Poppler error and retain Open externally. Each helper has a 25-second time limit and bounded output. Image decoding is limited to 128 MiB and 16,000 pixels per dimension. Other codecs depend on the installed mpv/FFmpeg build and report failures in the sidebar.

Backend references: [libmpv software rendering](https://github.com/mpv-player/mpv/blob/v0.40.0/include/mpv/render.h), [official software rendering example](https://github.com/mpv-player/mpv-examples/blob/master/libmpv/sdl/main_sw.c).

## Mention discovery

Discovery is **off by default**. Open **Discovery off** in the app for one all-session switch and independent file-type controls: Text, Code, Markdown, Image, Video, Audio, PDF, Web links, Folders, Apps and Other files. For example, leave Markdown on and Code off to surface documents without source/configuration files. All/None shortcuts are available. Choices are saved; existing unkept mentions of disabled types are hidden until re-enabled, while kept/pinned items, explicit posts and archived history remain accessible. Type selection controls new discoveries as well as feed visibility. Existing preferences initially allow every type.

The scanner takes a passive snapshot of the currently visible screen for each enabled live agent after it stops working, bounded to 64 KiB. It uses herdr's `visible` read source and never requests recent-history collection, which can scroll an agent's interface to retrieve older output. There is no fallback to an interactive history read. It strips terminal controls, extracts Markdown links, HTTP(S) URLs, file URIs and paths with fixed rules, and suppresses fenced code/build noise and configured dependency paths. No LLM or automatic launch is involved. Unresolved locations remain labeled and cannot open a guessed local file. Missing or changing files remain creating until stable across observations. Temporary socket reads retry quietly; repeated delays show a retry notice instead of an OS error code.

Discovered cards show their source on hover and offer **Keep** or **Dismiss**. Repeated mentions reuse stable session/host/location keys, including after restart or dismissal. Mentions matching an explicit post reuse it; a later explicit publication of the same resource promotes the discovered card while preserving retry identity. Plain paths containing spaces should be quoted or use Markdown links.

```sh
feedr sessions
feedr discovery enable --session SESSION_ID
printf 'Report: [open](</absolute/path/report.pdf>)\n' | feedr discovery ingest --session SESSION_ID
feedr discovery watch-log --session SESSION_ID --format codex /absolute/path/session.jsonl
feedr discovery watch-log --session SESSION_ID --format claude /absolute/path/session.jsonl
feedr discovery status
feedr discovery disable
```

New log watches start at the current end; `--from-start` explicitly opts into reading existing records. Supported formats: plain complete lines, Codex assistant `response_item`/`event_msg` records, and Claude assistant text records; `jsonl` recognizes both harness envelopes. User/tool records and incomplete final lines are ignored until complete. Cursor offsets persist; truncation/rotation is handled, and scans read at most 256 KiB per log per pass. Up to 32 logs and 256 pending file observations are bounded. Pane discovery sees only the current viewport: off-screen messages and paths broken across wrapped screen rows can be missed. Use completed-message hooks or session-bound logs to discover those resources without moving the agent's viewport. Paths in logs resolve against the explicitly bound session's recorded working directory and host.

For a completed-message hook, pipe assistant text to `feedr discovery ingest --session SESSION_ID --source hook`. Existing harness configuration is never changed automatically. To customize exclusions/settling/source bindings, save `feedr discovery status`, edit its JSON, then run `feedr discovery configure --input config.json`; `sessions: null` means all sessions subject to exclusions, while `sessions: []` selects none.

## Publish from an agent

```sh
cargo install --path . --locked
feedr post --title 'Report is ready' --body 'Findings and next steps.' ./report.md
feedr post --title 'Try the app' http://localhost:3000
feedr post --title 'Generated resources' ./output ./output/diagram.png
feedr sessions
feedr list --search report
feedr doctor
```

Paths resolve relative to the producer's working directory. The process inherits herdr pane/workspace/socket context when run from a herdr pane. Posts from outside herdr are explicitly unassigned. `--session ID` binds to an ID from `feedr sessions`; global MCP clients should use the same approach. Mention discovery stays off until you enable it.

`feedr post --input payload.json` accepts a `PostInput` object; `--input -` reads stdin. `feedr update --input update.json` accepts an update with `id` and `expected_revision`. Omitted update fields are unchanged. Both commands return JSON and fail with a nonzero exit status on errors.

```json
{"title":"Report is ready","body":"Review the findings.","resources":[{"uri":"/absolute/path/report.md"}],"idempotency_key":"report-v1"}
```

## MCP

Print the exact launch configuration for the installed binary:

```sh
feedr mcp-config
feedr mcp-config --session SESSION_ID
```

Add the printed `mcpServers.feedr` entry to your MCP client's configuration. It launches `feedr mcp` over stdio. Tool names: `feedr_post`, `feedr_update`, `feedr_list`, `feedr_get`, `feedr_sessions`. The MCP process writes protocol messages to stdout; service diagnostics go to its local log. Global clients must explicitly identify the producing session or leave cards unassigned; an arbitrary focused pane is never used. If relative paths are used, supply `context.cwd` or launch MCP from the right working directory.

Agent guidance: [integrations/agents/INSTRUCTIONS.md](integrations/agents/INSTRUCTIONS.md). An optional herdr plugin action is in `integrations/herdr/`; it requires `feedr` on PATH. No existing agent configuration is changed automatically.

## How it works

Agent → MCP/CLI → versioned Unix socket → local Rust service → SQLite → native egui desktop UI.

The service subscribes to herdr lifecycle events and reads authoritative snapshots. It discovers `HERDR_SOCKET_PATH`, `~/.config/herdr/herdr.sock`, and named sockets under `~/.config/herdr/sessions/`. Add a nonstandard local socket with `feedr connect /absolute/path/herdr.sock`. A disconnected or older unsupported server is reported in the connection panel and `feedr doctor`.

The broker starts automatically on the first CLI/MCP/GUI use and continues when windows close. It acknowledges posts after a SQLite transaction commits. Session-scoped idempotency keys make publication retries safe; expected revisions prevent conflicting updates. Files are live references, not archived copies. Missing or unmounted files stay in history and are hidden from the desktop feed by default; **View → Show unavailable files** includes them. Remote contexts cannot open a same-named local file. Herdr sessions without native IDs use terminal identity; a same-agent replacement inside the same terminal cannot always be distinguished until the harness reports a native session ID.

## Data and diagnostics

Default macOS location: `~/Library/Application Support/feedr/`. Linux: `$XDG_DATA_HOME/feedr/`, normally `~/.local/share/feedr/`. The directory contains `feedr.sqlite3`, `feedr.sock`, `broker.lock`, `broker.log`, and optional demo files/socket configuration. The directory is user-only and the socket is mode 0600. `--data-dir DIR` or `FEEDR_DATA_DIR` selects an isolated store; use a short path because Unix sockets have path-length limits.

The local service is not exposed on TCP and requires no cloud account. Any process running as your user may access the local service. Remove demo cards with Archive. To stop a testing service, stop its `feedr … serve` process; the next client starts it again. During development, restart an existing broker after rebuilding so the service and GUI use the same version.

Keyboard: Ctrl+Alt+F summons the running app on macOS/X11 when registration is available. On Wayland use the desktop launcher. Cmd/Ctrl K searches; J/K or arrow keys select artifact rows; P pins; Esc closes the preview. New cards are held behind a “new resources” button while a selected card is being read.

## Verification

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build
python3 scripts/smoke.py target/debug/feedr
python3 scripts/media-smoke.py target/debug/feedr
```

The smoke test uses an isolated directory and checks broker restart recovery, concurrent publishers, CLI deduplication, MCP initialization/tool calls, post updates and stale revision rejection. It cleans up its own child processes.

It also checks opt-in mention ingestion, durable dismissal, explicit synthetic-session binding, and partial JSONL tailing without scanning real user panes. Media smoke checks generate temporary PDF/video fixtures and exercise the real Poppler/libmpv backends; supply an output directory as a second argument to retain fixtures for manual UI testing. CI sets `FEEDR_TEST_NULL_AUDIO=1` for an audio sink without hardware; normal playback uses the system audio output. CI configuration is not evidence of a completed Linux runtime check.

## License

Feedr is licensed under [Apache 2.0](LICENSE). Native media runtimes are separate dependencies with their own licenses.

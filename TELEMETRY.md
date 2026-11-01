# Usage statistics

Feedr counts installations with one anonymous event per installation, which is **on by default**. Daily usage statistics, which show the maintainer whether people keep using Feedr and which features matter, are **off unless you turn them on**. The first launch asks once in **Set up Feedr**; closing that window without ticking the box declines.

## Changing your choice

- In the app: open **?** and toggle **Share anonymous usage statistics**.
- From a terminal:

  ```sh
  feedr telemetry status    # Your choice, install ID, pending events and an example event
  feedr telemetry enable
  feedr telemetry disable   # Also discards unsent statistics and skips the install count if not yet sent
  ```

- `DO_NOT_TRACK=1` ([Console Do Not Track](https://consoledonottrack.com)) or `FEEDR_TELEMETRY=0` turns everything off, including the install count, regardless of the recorded choice. To keep the install count from ever being sent, set one of them or run `feedr telemetry disable` before Feedr first runs.

Choices are stored per data directory, so `--data-dir` and `FEEDR_DATA_DIR` stores are asked separately and each counts as one installation.

## What is sent

Every event carries a random install ID created the first time Feedr runs (not derived from your hardware, account or host name) and these properties: Feedr version, operating system, CPU architecture and install type (`macos-app`, `deb`, `portable`, `cargo` or `source`).

| Event | When |
|---|---|
| `feedr_installed` | Once per installation, the first time the app or the local service runs. On by default |
| `feedr_active` | At most once per day, when the app window is open. Only after you opt in |
| `feedr_usage` | Once per day you used Feedr, sent the next time it runs, with that day's counts. Only after you opt in |

`feedr_usage` contains only these counts:

| Count | Meaning |
|---|---|
| `posts` | New posts published to your feed (idempotent repeats are not counted) |
| `posts_from_claude`, `posts_from_codex`, `posts_from_other` | Which kind of client published them |
| `resources_text`, `_code`, `_markdown`, `_image`, `_video`, `_audio`, `_pdf`, `_web`, `_folder`, `_app`, `_other` | Attachment types |
| `posts_viewed`, `opened_externally`, `revealed_in_folder`, `pinned`, `archived` | Feed actions |
| `agent_setup_claude`, `agent_setup_codex`, `cli_installed` | Setup steps completed |
| `updated` | Feedr started on a new version |

## What is never sent

Post titles or bodies, file paths, URLs, file names or contents, project or session names, working directories, host names, herdr identifiers, or anything you type. Run `feedr telemetry status` to see an example event exactly as it would be uploaded.

## Where it goes

Events go directly to [PostHog](https://posthog.com) Cloud over HTTPS as anonymous events, with person profiles and GeoIP lookup disabled. Like any HTTPS request, the upload reveals your IP address to PostHog. Feedr sends at most a few small batches per day, never blocks the app, and retries a failed upload later, keeping no more than 64 pending events. Unsent data stays in `telemetry/` inside Feedr's data directory.

Only official release builds contain the PostHog project token. Builds from source never send anything unless you supply your own token at build time (see `build.rs`), and development (debug) builds print events to standard error instead of uploading them.

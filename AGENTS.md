# Working on Feedr

Feedr is a native Rust companion to herdr. Product requirements and implementation epics live in Tokanban project FEEDR. Keep the application core and UI in Rust; herdr in `ref/herdr/` is reference material and must stay excluded from this repository.

Build with `cargo build`; verify substantial changes with `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets`, and `python3 scripts/smoke.py target/debug/feedr`. Never report configured Linux CI as a completed Linux runtime check. Keep protocol and persistence changes backward compatible, or add a tested migration.

Use `feedr post` or the Feedr MCP tools to publish useful deliverables for the user, including runnable builds and relevant output files. Follow `integrations/agents/INSTRUCTIONS.md`. Use absolute resource paths and an idempotency key. Do not guess session attribution for global MCP clients. Publication should never automatically launch an artifact.

The current build covers the FEEDR-1–5 path plus built-in media previews and opt-in mention discovery. The public viewer extension API, advanced remote resources, Linux runtime verification and full release packaging remain later work. Keep these limits visible in README and Tokanban rather than marking incomplete epics done. After media changes, also run `python3 scripts/media-smoke.py target/debug/feedr` with Poppler, libmpv and FFmpeg installed. Discovery tests must bind to synthetic sessions; never enable scanning of the user's real panes as a test side effect.

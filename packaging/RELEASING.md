# Release builds

Set `Cargo.toml`/`Cargo.lock` to the next semantic version and update `packaging/release-notes.md`. Commit and push, then create and push the matching `vVERSION` tag. Never move a published release tag or replace its assets. `workflow_dispatch` on main can validate packages without publishing. The tag workflow publishes only when all four architecture jobs and the source job pass.

The release has two DMGs, two macOS ZIP updater archives, two `.deb` installers, two Linux tar updater archives, and source in tar.gz/ZIP formats. It includes per-asset checksums and a combined `SHA256SUMS`. Source uses `git archive` from the release commit and includes `Cargo.lock`; it does not vendor crates, so a source build needs public registry access. Binary packages include dependency notices and license texts from the locked native dependency graph and Rust standard library.

## macOS

Build on the desired native architecture with Rust 1.95, Xcode command line tools and Python 3.12:

```sh
brew install poppler mpv ffmpeg
python3 -m pip install dmgbuild==1.6.7 pillow==13.0.0
MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --release --locked
python3 scripts/package-release.py
python3 scripts/test-dmg.py dist/feedr-vVERSION-macos-ARCH.dmg
```

The packager builds a temporary app bundle, never modifies an installed/running Feedr.app, validates the ZIP with Feedr's real extractor, and creates a compressed DMG with an Applications link, installation guide and custom Finder layout.

By default it uses ad-hoc signing. Optional local Developer ID signing uses `FEEDR_CODESIGN_IDENTITY`; notarization additionally uses `FEEDR_NOTARY_PROFILE`, the name of a previously configured `notarytool` keychain profile. The script signs the app with hardened runtime, permits user-installed libmpv, notarizes/staples the app before archiving, then signs/notarizes/staples the DMG. Those credentials are not stored in this repository, printed, or included in assets. A release described as notarized must actually pass that path and Apple validation. Hosted jobs currently use ad-hoc signing, with an explicit disclosure in the release notes and installer.

## Debian / Ubuntu

The release workflow builds natively in `rust:1.95-bookworm` on amd64 and arm64 runners. See the workflow for build dependencies. After `cargo build --release --locked`, run `python3 scripts/package-release.py`. `dpkg-shlibdeps` determines linked library versions; the package also declares dynamically loaded desktop and media dependencies. Payloads use root ownership without maintainer scripts, include an application launcher/icon, and do not modify user data.

Run `sh scripts/test-deb.sh dist/feedr_VERSION_ARCH.deb` **only as root in an isolated Linux builder**. It installs through apt, validates the launcher and installed files, exercises CLI/MCP persistence and native media backends, launches the GUI on Xvfb, then removes the package and verifies history remains. Its GUI screenshot is retained as a CI artifact. Wayland and desktop hotkeys require testing on real desktops.

## Publication

The publisher requires a checkout matching the tag and all ten exact versioned packages plus their checksums in `dist/`. `python3 scripts/publish-release.py --tag vVERSION` uploads only that allowlist, never other local dist contents. It creates/resumes a draft, verifies remote sizes and GitHub digests for every asset, then marks it public/latest. A checksum mismatch, missing architecture, failed installer smoke test or source failure prevents publication.

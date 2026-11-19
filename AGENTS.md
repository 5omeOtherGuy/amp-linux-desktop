# Amp Linux Desktop

Amp Linux Desktop, a GTK 4, libadwaita, and WebKitGTK app in Rust (binary `amp-linux-desktop`, app ID
`dev.ampnative.Amp`, GitHub `5omeOtherGuy/amp-linux-desktop`). It loads ampcode.com and answers the
page's native bridge the way Amp's macOS app does. Every agent working in this repository reads this
file first, after the workstation rules in `~/.agents/AGENTS.md`, which apply here unchanged. Amp
orbs do not have that file. An agent taking the project over without prior context then loads the
[onboarding-amp-linux-desktop](.agents/skills/onboarding-amp-linux-desktop/SKILL.md) skill.

## Crates

`crates/` is the app, in Rust: the packages, `scripts/install`, and `dev/` build it. Keep its app ID
and data paths, so an upgrade keeps the settings and the sign-in.

The workspace splits the port so that a change recompiles one crate: `crates/app` (windows and the
page's bridge), `crates/common` (paths, settings, page scripts, theme, icons; no GTK),
`crates/dictation`, and `crates/huddle`. Iterate with `cargo check` and
`cargo clippy --workspace --all-targets`, and run debug builds. Huddles (the default `huddles`
feature) link LiveKit's prebuilt libwebrtc: webrtc-sys downloads it on the first build of each
profile (156 MB, 700 MB unpacked under `target/`) and compiles against it with `clang++`.
`--no-default-features` builds without Huddles.

## Verify in the running app

- Before you call a change to the app done, prove it end to end with the
  [verify-amp-linux-desktop](.agents/skills/verify-amp-linux-desktop/SKILL.md) skill. Follow its
  "Do not" rules in every run: a signed-in run uses a copy of the owner's real Amp login. The
  skill runs a debug build of `crates/`.
- The owner's running Amp comes from `scripts/install`. Do not run `scripts/install` or install the
  Arch package on the workstations unless the owner asks for it.

## Keep Amp's artwork and private content out

- The repository carries no Amp app icon and no other file from Amp's macOS disk image.
  `amp-linux-desktop --extract-icons` extracts them: `scripts/install` and the Arch package at install or
  build time, and the app itself on first start when no icons are installed (the .deb and .rpm,
  which are built in CI and handed out as files).
- Screenshots in `docs/img` show no thread titles, names, email addresses, tokens, home paths, or
  Amp app icons. Take them in a signed-out run, or crop them.

## Documentation

- `README.md` is for users: what the app adds, parity with the macOS app, and installation. A
  parity row says Yes only with a screenshot in `docs/img`, or with a Linux code reference and a
  macOS reference (a class, an Info.plist key, or a string in the disk image). Other rows say "Not
  verified". When you change a bridge handler, update its row. When you cite a newer disk image
  build, check every row against it.
- `CHANGELOG.md` lists user-visible changes per release, newest first. Add new changes under
  `## Unreleased`.

## Release

A release needs the owner's go-ahead and follows the
[releasing-amp-linux-desktop](.agents/skills/releasing-amp-linux-desktop/SKILL.md) skill.

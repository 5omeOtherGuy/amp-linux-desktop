# Amp Linux Desktop

Amp Linux Desktop, a GTK 4, libadwaita, and WebKitGTK app (package `amp_native`, app ID
`dev.ampnative.Amp`, GitHub `5omeOtherGuy/amp-linux-desktop`). It loads ampcode.com and answers the
page's native bridge the way Amp's macOS app does. Every agent working in this repository reads this
file first, after the workstation rules in `~/.agents/AGENTS.md`, which apply here unchanged. Amp
orbs do not have that file.

## Python app and Rust port

The repository holds two implementations of the same app. `amp_native/` is the Python (PyGObject)
app that the packages, `scripts/install`, and `dev/` build and run. `crates/` is its Rust port, a
Cargo workspace that is not packaged yet. Both use the same app ID and data paths, so they share
settings and the sign-in. A change to the app's behaviour goes into both until the port replaces
the Python app. The Rust app takes its version from `Cargo.toml` (`[workspace.package] version`).

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
  skill runs the Rust app (a debug build of `crates/`); to verify the Python app, run the skill
  from a checkout of `main`.
- The owner's running Amp comes from `scripts/install`. Do not run `scripts/install` or install the
  Arch package on the workstations unless the owner asks for it.

## Keep Amp's artwork and private content out

- The repository carries no Amp app icon and no other file from Amp's macOS disk image.
  `amp_native/icons.py` extracts them: `scripts/install` and the Arch package at install or
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

A release needs the owner's go-ahead. The version is in `amp_native/config.py` (`VERSION`),
`packaging/arch/PKGBUILD` (`pkgver`), `packaging/fedora/amp-linux-desktop.spec` (`Version`, plus a
`%changelog` entry), and the `CHANGELOG.md` heading. Keep the four equal.

1. Set the version, reset `pkgrel=1`, and run `makepkg --printsrcinfo > .SRCINFO` in
   `packaging/arch`.
2. Copy `packaging/arch` to a scratch directory, point its `source` at the local commit, run
   `makepkg -f`, and check the PKGBUILD and the package with `namcap`.
3. Tag the release commit `vX.Y.Z` and push the tag. The PKGBUILD and the spec file build that
   tag. The `release` workflow builds the `.deb` and attaches it to the GitHub release. The new
   tag also reaches COPR project `5omeotherguy/amp-linux-desktop` through its GitHub webhook,
   which builds the `.rpm` from `.copr/Makefile` for Fedora 43 to rawhide. The webhook sends only
   `create` events, so pushes to branches build nothing and every pushed tag builds. `copr-cli` uses
   `~/.config/copr` (vault entry `copr-api-token`); the Fedora account is `fedora-account`.
4. Push `PKGBUILD`, `.SRCINFO`, and `amp-linux-desktop.sh` to the AUR repository
   `ssh://aur@aur.archlinux.org/amp-linux-desktop.git`, and create the GitHub release with the
   version's `CHANGELOG.md` section as its notes.

The package downloads Amp's disk image build `_ampbuild` for the icons. If
`https://static.ampcode.com/mac/appcast.xml` no longer lists that build, set `_ampbuild` to a listed
one, run `updpkgsums`, and raise `pkgrel`.

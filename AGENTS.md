# Amp Linux Desktop

Amp Linux Desktop, a GTK 4, libadwaita, and WebKitGTK app (package `amp_native`, app ID
`dev.ampnative.Amp`, GitHub `5omeOtherGuy/amp-linux-desktop`). It loads ampcode.com and answers the
page's native bridge the way Amp's macOS app does. Every agent working in this repository reads this
file first, after the workstation rules in `~/.agents/AGENTS.md`, which apply here unchanged. Amp
orbs do not have that file.

## Verify in the running app

- Before you call a change to the app done, prove it end to end with the
  [verify-amp-linux-desktop](.agents/skills/verify-amp-linux-desktop/SKILL.md) skill. Follow its
  "Do not" rules in every run: a signed-in run uses a copy of the owner's real Amp login.
- The owner's running Amp comes from `scripts/install`. Do not run `scripts/install` or install the
  Arch package on the workstations unless the owner asks for it.

## Keep Amp's artwork and private content out

- The repository carries no Amp app icon and no other file from Amp's macOS disk image.
  `scripts/fetch-icons.py` extracts the icons when a user installs or builds the package.
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
`packaging/arch/PKGBUILD` (`pkgver`), and the `CHANGELOG.md` heading. Keep the three equal.

1. Set the version, reset `pkgrel=1`, and run `makepkg --printsrcinfo > .SRCINFO` in
   `packaging/arch`.
2. Copy `packaging/arch` to a scratch directory, point its `source` at the local commit, run
   `makepkg -f`, and check the PKGBUILD and the package with `namcap`.
3. Tag the release commit `vX.Y.Z` and push the tag. The PKGBUILD builds that tag.
4. Push `PKGBUILD`, `.SRCINFO`, and `amp-linux-desktop.sh` to the AUR repository
   `ssh://aur@aur.archlinux.org/amp-linux-desktop.git`, and create the GitHub release with the
   version's `CHANGELOG.md` section as its notes.

The package downloads Amp's disk image build `_ampbuild` for the icons. If
`https://static.ampcode.com/mac/appcast.xml` no longer lists that build, set `_ampbuild` to a listed
one, run `updpkgsums`, and raise `pkgrel`.

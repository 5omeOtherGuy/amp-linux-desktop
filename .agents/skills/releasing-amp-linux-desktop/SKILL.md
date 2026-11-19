---
name: releasing-amp-linux-desktop
description: "Releases Amp Linux Desktop: version bump, Arch package check, tag, the GitHub release with the .deb, the COPR .rpm and the AUR push, plus updating the Amp disk image build and libwebrtc the packages download. Use once the owner has given the go-ahead for a release, or when the packages' downloads need updating."
---

# Release Amp Linux Desktop

The version is in `Cargo.toml` (`[workspace.package] version`), `packaging/arch/PKGBUILD`
(`pkgver`), `packaging/fedora/amp-linux-desktop.spec` (`Version`, plus a `%changelog` entry), and
the `CHANGELOG.md` heading. Keep the four equal.

1. Set the version, run `cargo update --workspace` so that `Cargo.lock` matches (the packages build
   with `--locked`), reset `pkgrel=1`, and run `makepkg --printsrcinfo > .SRCINFO` in
   `packaging/arch`.
2. Copy `packaging/arch` to a scratch directory, point its `source` at the local commit, run
   `makepkg -f`, and check the PKGBUILD and the package with `namcap`.
3. Tag the release commit `vX.Y.Z` and push the tag. The PKGBUILD and the spec file build that
   tag. The `release` workflow builds the `.deb` and attaches it and `SHA256SUMS` to a draft
   GitHub release with the version's `CHANGELOG.md` section as its notes. The new
   tag also reaches COPR project `5omeotherguy/amp-linux-desktop` through its GitHub webhook,
   which builds the `.rpm` from `.copr/Makefile` for Fedora 43 to rawhide. The webhook sends only
   `create` events, so pushes to branches build nothing and every pushed tag builds. `copr-cli` uses
   `~/.config/copr` (vault entry `copr-api-token`); the Fedora account is `fedora-account`.
4. Once COPR has built the tag, download the `.deb` and `SHA256SUMS` from the draft and install
   the `.deb` in a clean `debian:trixie` container, then publish the draft with
   `gh release edit vX.Y.Z --draft=false`.
5. Push `PKGBUILD` and `.SRCINFO` to the AUR repository
   `ssh://aur@aur.archlinux.org/amp-linux-desktop.git`.

The package downloads Amp's disk image build `_ampbuild` for the icons. If
`https://static.ampcode.com/mac/appcast.xml` no longer lists that build, set `_ampbuild` to a listed
one, run `updpkgsums`, and raise `pkgrel`.

The PKGBUILD (`_webrtc`) and the spec file (`%global webrtc`) also name LiveKit's prebuilt libwebrtc,
which they download for webrtc-sys so the build needs no network of its own. It is `WEBRTC_TAG` in
the `webrtc-sys-build` crate that `Cargo.lock` pins; when an update of the `livekit` crate changes
it, set both to the new tag and run `updpkgsums`.

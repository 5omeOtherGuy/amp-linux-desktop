# Changelog

## Unreleased

### Changed

- The app is written in Rust now and ships as one binary, with the same features, settings, and
  sign-in. Huddles no longer need the LiveKit Python package: the binary includes LiveKit's SDK.
- The Arch package, the `.deb`, and the `.rpm` are built for x86_64 only, because Huddles link
  LiveKit's prebuilt WebRTC library. The `.deb` is now `amp-linux-desktop_VERSION-1_amd64.deb`.
- `scripts/install` needs a Rust toolchain and clang, builds the app, and copies the binary to
  `~/.local/bin`. It removes the LiveKit Python package that earlier versions installed.

### Fixed

- Closing an extra window, the Puck window or the settings window frees it. Before, each closed
  window and its page stayed in memory until Amp quit.

## 0.1.1 - 2026-11-06

### Added

- A `.deb` for Debian 13 and Ubuntu 24.04 or later, attached to each GitHub release.
- An `.rpm` for Fedora through the COPR repository `5omeotherguy/amp-linux-desktop`.
- Amp's app icons are fetched from Amp's macOS disk image on first start when the package did not
  install them, as with the `.deb` and `.rpm`.

## 0.1.0 - 2026-11-06

The first release. [README.md](README.md) compares each feature with Amp's macOS app.

### Added

- Amp in its own window: ampcode.com in the layout of Amp's macOS app, with your desktop's window
  controls, a menu bar (Alt) with the macOS app's commands and shortcuts, and a settings window.
- Sign-in through your browser.
- A tray icon with your recent threads, desktop notifications that open their thread, and
  `amp-linux-desktop --action` for compositor shortcuts.
- This computer as a runner for threads from ampcode.com, and local project import.
- Files from a thread opened in your editor, shown in your file manager, or saved to Downloads.
- Dictation through GStreamer, and Huddles through the LiveKit SDK, because distribution WebKitGTK
  has no WebRTC.
- Colours from the current Omarchy theme.
- An Arch Linux package in `packaging/arch`, for the AUR. It builds Amp's app icons from Amp's
  macOS disk image.

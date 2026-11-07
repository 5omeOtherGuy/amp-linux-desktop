# Changelog

## Unreleased

## 0.2.0 - 2026-11-07

The app is rewritten in Rust and ships as one binary. It replaces the Python app of 0.1.x, keeps its
settings and sign-in, and upgrades it in place under the same package name.

### Changed

- The app is written in Rust. The Python app (`amp_native/`) is removed, and the packages no longer
  need Python, PyGObject, or the LiveKit Python package: the binary includes LiveKit's SDK for
  Huddles.
- The Arch package, the `.deb`, and the `.rpm` are built for x86_64 only, because Huddles link
  LiveKit's prebuilt WebRTC library. The `.deb` is now `amp-linux-desktop_VERSION-1_amd64.deb`
  and needs Debian 13, Ubuntu 24.04, or later.
- `scripts/install` needs a Rust toolchain (1.96 or later) and clang, builds the app, and copies the
  binary to `~/.local/bin`. It removes the LiveKit Python package that earlier versions installed.
- The menu bar follows the macOS app: an Amp menu with About, Check for Updates…, App Settings…,
  Warn Before Quitting, and Quit, and a Window menu with the open windows. Menu items are enabled
  only where the page can use them.
- Amp keeps running when its last window closes, as the macOS app does. The tray, the launcher, and
  `amp-linux-desktop --action show` open a new window; Quit (Ctrl+Q) or the tray's Quit Amp end
  it.
- The tray menu has the layout of the macOS menu bar extra: Open Amp, New Thread, Puck, Call Puck,
  the recent threads, the runner's switches, Settings…, and Quit Amp.
- The page's Puck button opens Puck in its own window by default, as on the Mac; Settings → General
  → Open Puck switches back to the current window.

### Added

- View → Puck Window, Ctrl+Alt+P, and `--action puck-window` open Puck in its own window, or close
  it.
- Settings → General: Show Amp in Tray, a Keyboard Shortcuts section that lists the `--action`
  command for each of the macOS app's global shortcuts, and Import Threads… for conversations from
  Codex, Claude Code, OpenCode, Pi, and Conductor through `amp threads import`.
- Settings → Runner: Choose Projects… adds the Git checkouts that other coding agents worked in, and
  Sign In… signs the Amp CLI in when the runner stopped because it was signed out.
- Files drag out of the page into other apps.
- Print… prints the thread through GTK's print dialog.
- Add Another Account signs in through your browser. This is not yet tested end to end.
- `--action dictate-to-puck` opens Puck with dictation.
- The page's Keymap lists the desktop-wide shortcuts.

### Fixed

- Closing an extra window, the Puck window, or the settings window frees it. Before, each closed
  window and its page stayed in memory until Amp quit.
- The tray and About show the chosen app icon at once.

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

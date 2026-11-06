# Changelog

## Unreleased

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

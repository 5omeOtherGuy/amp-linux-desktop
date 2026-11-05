# Amp Linux Desktop

A native Linux desktop app for [Amp](https://ampcode.com), built with GTK 4, libadwaita and
WebKitGTK. Like the macOS app, it loads ampcode.com and answers the page's native bridge, so
new Amp features arrive without an app update. It follows the desktop's own look (window
controls, menus, Omarchy themes) instead of copying macOS.

Beyond the web app it adds: browser sign-in, the local runner, notifications and a launcher
badge, a tray menu of recent threads, desktop-wide actions, dictation, Puck voice, file
open/save bridges, local project import, and Huddles through a native LiveKit client (distro
WebKitGTK has no WebRTC).

## Install

Needs Python 3 with PyGObject, GTK 4, libadwaita 1.2+ and WebKitGTK 6.0:

- Arch: `pacman -S python-gobject gtk4 libadwaita webkitgtk-6.0 p7zip`
- Debian/Ubuntu: `apt install python3-gi gir1.2-adw-1 gir1.2-webkit-6.0 7zip`

Optional: GStreamer (`gir1.2-gstreamer-1.0`, PulseAudio/PipeWire plugins) for dictation and the
Huddle camera.

```sh
scripts/install
```

This adds the `amp-linux-desktop` launcher, the desktop entry (which also receives the sign-in
callback), Amp's icons, and the `livekit` Python package for Huddles under
`~/.local/share/amp-native/python`.

## Desktop-wide shortcuts

`amp-linux-desktop --action NAME` runs an action in the running app, or starts it. Names:
`show`, `new-thread`, `dictate-new-thread`, `toggle-puck`, `puck-voice`. Bind them in your
compositor, for example Hyprland:

```ini
bind = SUPER SHIFT, N, exec, amp-linux-desktop --action new-thread
bind = SUPER, slash, exec, amp-linux-desktop --action toggle-puck
```

## Development

`python3 -m amp_native` runs from a checkout; `AMP_NATIVE_DEBUG=1` logs every bridge message to
`~/.cache/amp-native/amp-native.log`. The scripts in `dev/` are for running inside an Amp orb's
Desktop only.

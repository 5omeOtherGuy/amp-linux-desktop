# Notifications

Once notifications are enabled, the sidebar's "Turn On Notifications" card stays gone, and threads that finish or need input while Amp is in the background show a desktop notification titled "Amp" that opens the thread when clicked.

## Sub-features

- `notif-card` the card shows only while the setting is not determined, and stays gone after enabling, reloading (`ctrl+r`) and restarting.
- `notif-deliver` a page notification reaches the freedesktop notification daemon with app name `Amp`.
- `notif-click` clicking it opens the thread.

## How to get to it (user POV)

- The sidebar card's `Enable Notifications`, or Preferences → Notifications.
- Notifications appear when a thread needs attention and its window is not focused.

## Driving it with verify-amp

Preconditions:

- For `notif-card`: `verify-amp launch RUN --set notifications=notDetermined` (changes only the run's copy of the settings).
- The session's private D-Bus has no notification daemon. Start one inside the session before testing delivery, e.g. `agent-desktop run S --log $XDG_RUNTIME_DIR/verify-amp/RUN/cache/amp-native/dbus-monitor.log -- dbus-monitor "interface='org.freedesktop.Notifications'"` to observe calls (`exec` would block in the foreground; cleanup keeps the log), or a real daemon such as `mako` if installed.

- **Card.** With `notDetermined`, the card is visible at the bottom of the sidebar; click `Enable Notifications` (log: `ampNativeEnableNotifications`; the run's settings now say `"authorized"`), `ctrl+r`, screenshot: no card. Restart (`agent-desktop key S ctrl+q`, then `verify-amp app RUN`), screenshot: still no card.
- **Delivery.** Focus another window in the session (e.g. `agent-desktop run S -- foot`), wait for a real event, and capture the `Notify` call with app name `Amp`.

## Gotchas

- There is no safe way to make the server send a notification without acting in the owner's account; if none arrives, report delivery as not covered rather than faking one.
- The quickshell daemon on the owner's desktop is not reachable from the session; delivery there can only be checked by the owner.

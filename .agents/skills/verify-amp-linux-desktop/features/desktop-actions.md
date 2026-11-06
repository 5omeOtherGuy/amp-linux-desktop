# Desktop-wide actions

Compositor shortcuts and the launcher's right-click actions run `amp-linux-desktop --action NAME`, which brings the running Amp window forward and runs the command (new thread, dictation, Puck), or starts Amp first.

## Sub-features

- `action-show` `show` presents the window.
- `action-new-thread` `new-thread` opens the new-thread composer.
- `action-toggle-puck` `toggle-puck` shows or hides Puck.
- `action-cold` an action while no window exists starts the app and runs the command once the page is ready.

## How to get to it (user POV)

- A Hyprland binding such as `bind = SUPER SHIFT, N, exec, amp-linux-desktop --action new-thread`.
- Right-click the `Amp` launcher entry: `New Thread`, `Dictate New Thread`, `Show Puck`.

## Driving it with verify-amp

Preconditions:

- Signed-in launch on the home page.

- **New thread.** `verify-amp shot RUN before`, `verify-amp app RUN --action new-thread`, wait 3 s, `verify-amp shot RUN after`. The after-screenshot shows the new-thread overlay with a focused `Write prompt…` field. It is an overlay over the current page, so the log has no `ampNavigationState` for it; the screenshots are the proof. Close it with its close button without sending.
- **Puck.** `verify-amp app RUN --action toggle-puck`, screenshot shows Puck; run it again, Puck hides. The log shows `ampNavigationState` to the Puck thread.
- **Show.** Focus another window (`agent-desktop run S -- foot`), screenshot, `verify-amp app RUN --action show`, screenshot. Headless sway does not hand focus to the app; it marks it urgent. Check `agent-desktop swaymsg S -t get_tree` for the Amp node with `"urgent": true`: that proves the app asked to be shown. Actual focus can only be confirmed on the owner's Hyprland (`focus_on_activate = true` in Omarchy); report it as not verified there.
- **Cold start.** Close the window (`agent-desktop key S ctrl+q`), check `agent-desktop windows S` lists no Amp window, then `verify-amp app RUN --action new-thread`, wait about 15 s; a window opens on home with the new-thread overlay. This instance is the session's app from then on; `doctor` and `cleanup` work on it.

## Gotchas

- `verify-amp app` must run inside the session (it does, through `agent-desktop exec`); running `amp-linux-desktop --action …` from a plain shell talks to the owner's running app.
- `dictate-new-thread` and `puck-voice` open the microphone; the headless session has the owner's PipeWire, so do not use them without asking.
- Opening the new-thread composer does not create a thread until a message is sent; do not send.
- The new-thread overlay can carry a "Set Up Orbs" banner (`Set Up with Amp`, `Docs`, `Dismiss`). Click none of them; `Dismiss` may change the owner's account state.
- With Puck open, a green call button sits where `New Thread` usually is. Clicking it starts a voice call with the microphone; do not.

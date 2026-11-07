# Tray and window controls

The tray icon's menu mirrors the Mac's menu bar extra: Open Amp, New Thread, Puck, Call Puck, the unread threads, the runner switches (Runner, Keep Awake, Share Display, and Share Network when the account has Amp Net), Settings… and Quit Amp. The window can be minimized and maximized from the page's buttons at the top right and from the Window menu.

## Sub-features

- `tray-menu` the menu shows the items above; a thread row opens that thread.
- `tray-runner-switch` a runner switch in the tray and the same switch in App Settings → Runner follow each other, both ways, and change the run's `settings.json`.
- `window-min-max` minimize, maximize and restore.

## How to get to it (user POV)

- Right-click the Amp icon in the panel's tray (Settings → General → Show Amp in Tray).
- The `–` and `□` buttons at the top right of the window, or Window → Minimize / Maximize (Alt shows the menu bar).

## Driving it with verify-amp

Preconditions:

- `verify-amp desk RUN` after launch, so the session has a tray host.

- **Menu.** Right-click the Amp icon at the right end of waybar's strip at the bottom (left-click only shows Amp), screenshot the menu.
- **Switches.** Open the tray's Settings… → Runner (the dialog tiles beside the main window), click Keep Awake or Share Display in the tray, screenshot: the Settings switch flips and the run's `config/amp-native/settings.json` changes (`runner_keep_awake`, `runner_desktop`). Flip it back in Settings and reopen the tray: the check mark follows. Turn both back off.
- **Minimize and maximize.** `agent-desktop key S ctrl+q`, `verify-amp labwc RUN`, wait 15 s, screenshot, then click the window's maximize, restore and minimize buttons with a screenshot after each. A notification from mako can cover the buttons; click it away first.

## Gotchas

- Do not click Runner or Share Network in the tray with a signed-in copy: they register this computer as a runner, or change Amp Net, in the owner's account. Keep Awake and Share Display only change local settings while the runner is off.
- Do not click Call Puck: it starts a voice call with the microphone.
- In sway, minimize and maximize do nothing; that is the compositor, not the app.

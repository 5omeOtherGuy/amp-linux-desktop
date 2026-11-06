---
name: verify-amp-linux-desktop
description: Launch, drive and screenshot Amp Linux Desktop (the GTK 4/WebKitGTK app in this repo, the Rust binary amp-linux-desktop) in an isolated headless desktop to prove a change works end to end. Use before calling a change to the app done, for e2e verification of its features, or to reproduce a bug a user reported in the app. Workstation (P720/laptop) only; needs the agent-desktop skill and a GPU session.
---

# Verify Amp Linux Desktop

The app is a GTK window around ampcode.com. Users touch the window: the page inside it (threads,
composer, sidebar, Desktop/Terminal tabs), the native parts around it (window controls, settings
window, notifications, tray, `--action` commands), and the launcher. Verification drives a real
app instance from this checkout in its own headless Wayland session with `agent-desktop`, never
the owner's running Amp or his Hyprland session.

Read [features/README.md](features/README.md) before driving; each feature file there is the recipe.

## Launch

```sh
.agents/skills/verify-amp-linux-desktop/bin/verify-amp launch RUN          # signed in (copy of the owner's login)
.agents/skills/verify-amp-linux-desktop/bin/verify-amp launch RUN --signed-out
.agents/skills/verify-amp-linux-desktop/bin/verify-amp launch RUN --scale 1.5   # the owner's real scale
```

`RUN` is a short unique name for this verification run (e.g. `notif-0511`). Launch:

- starts `agent-desktop` session `ampv-RUN` with `--gpu` (the owner renders on an RTX 3060; software rendering changes timing and the video decoder path),
- copies `~/.local/share/amp-native/web` (cookies) and `~/.config/amp-native/settings.json` into `$XDG_RUNTIME_DIR/verify-amp/RUN/` and points the app's XDG dirs there, so the owner's login and settings are read, never written,
- `--set KEY=VALUE` changes the run's copy of the settings (e.g. `--set notifications=notDetermined`),
- builds this checkout with `cargo build` and runs `target/debug/amp-linux-desktop` with `AMP_NATIVE_DEBUG=1` (every bridge message is logged, cut to its first 300 characters, so long `ampNativeAppState` bodies often lack fields such as `puckOpen`), with its stderr in the run's `cache/amp-native/stderr.log`, where the installed app also keeps it,
- waits for the `dev.ampnative.Amp` window, then 10 s for the page.

Ready means: `verify-amp doctor RUN` reports a page other than `/__native/start` (signed in) and a
screenshot shows the sidebar thread list. Load the `agent-desktop` skill once for its input and
screenshot rules. Each session has its own private D-Bus, so the app's single-instance check never
reaches the owner's Amp.

## Doctor

```sh
.agents/skills/verify-amp-linux-desktop/bin/verify-amp doctor RUN
```

Read-only. Prints the app PID and confirms it uses this run's private data dir (`FOREIGN` means
you are looking at someone else's instance: stop), the checkout commit (`+dirty` when uncommitted
or untracked changes are under test), the renderer, the current page, signed-in state, the count of
warnings/errors in the app log and the count of panics in the stderr logs. Run it first whenever anything looks off.

## Drive

There is no DOM or accessibility access from outside the app (WebKitGTK exposes no CDP and the
headless session has no AT-SPI bus). Drive the way a user does:

- `agent-desktop click ampv-RUN X Y`, `type`, `key ctrl+n`, `scroll`, `move`: coordinates are pixels in the latest screenshot. Always take a screenshot and look at it (view_media / Read) before clicking; layouts differ by window size and scale.
- With `--scale` other than 1, only `click`/`move`/`drag` take screenshot pixels. Window sizes from `launch` and `agent-desktop windows`, and regions for `grim -g` or `shot --region`, are logical pixels (screenshot pixels ÷ scale: a 1600×999 PNG is a 1066×666 window at 1.5). To keep a detail, crop the full screenshot instead.
- `verify-amp app RUN --action new-thread` runs a desktop-wide command against the running instance (names: `show`, `new-thread`, `dictate-new-thread`, `toggle-puck`, `puck-voice`, `puck-window`, `dictate-to-puck`). It starts detached and returns at once.
- Restart the app inside the same session: `agent-desktop key ampv-RUN ctrl+q`, then `verify-amp app RUN` with no arguments, then wait about 10 s and screenshot.
- `agent-desktop swaymsg ampv-RUN '[app_id=dev.ampnative.Amp] fullscreen enable'` gives one big window.
- Navigate to a thread by clicking it in the sidebar. A thread URL on the command line opens it in a second window.
- Page state is in the app log (`verify-amp logs RUN`): `ampNavigationState <url>`, `ampNativeAppState {...}` and every other bridge message, with timestamps.

## Evidence

Everything goes to `.amp/in/artifacts/verify/RUN/` (git-ignored through the repo's info/exclude):
`verify-amp shot RUN NAME` writes `NAME.png` there, and cleanup copies every `*.log` from the run's `cache/amp-native/` there (app log, stderr logs, and any monitor log you wrote into that folder).

Proof standard:
- Drive the user path (click, type, the launcher command), not internal calls or injected JS.
- Capture the action and the resulting state: a screenshot before and after, plus the log lines with timestamps that show the effect (`ampNavigationState`, the bridge message, a warning).
- Check side effects outside the window: notifications reaching the daemon, files saved, the settings file changed (in the run's private config dir).
- Inspect every screenshot and state what it shows; an unviewed screenshot proves nothing.
- Say what was not covered and why (e.g. the Mac tab needs a running Mac satellite).

## Do not

- Do not send messages, create threads, archive, delete or change settings in the owner's Amp account unless the task asks for it: the copied login is his real account. Typing into the composer without sending is fine; clear it afterwards (`ctrl+a`, `BackSpace`).
- Do not touch his Hyprland session (`WAYLAND_DISPLAY=wayland-1`), his running app, or `~/.local/share/amp-native` / `~/.config/amp-native`.
- Do not send SIGUSR1/SIGUSR2 to anything that loads WebKitGTK: JavaScriptCore uses SIGUSR1 internally and crashes.
- Do not kill by name (`pkill -f amp-linux-desktop` also matches the owner's app and your own shell).
- Do not wake orbs or start Mac satellites (billable) unless the task asks for that feature. Opening an orb thread's Desktop view starts that orb's desktop, and the Mac view may start a satellite.
- Do not click action buttons in page banners, such as `Investigate in New Orb` under "Orb Setup Has Been Slow" on orb threads, or `Set Up with Amp` / `Docs` / `Dismiss` in the new-thread overlay's "Set Up Orbs" banner: they start threads and orbs or change the owner's account. Do not click Puck's green call button (starts a voice call). Close the views you opened with their own close buttons and check each click target in the screenshot first.

## Cleanup

```sh
.agents/skills/verify-amp-linux-desktop/bin/verify-amp cleanup RUN
```

Stops the session and only the processes it started, deletes the copied login and the run's data
dir, and keeps the evidence directory. Run it after every run, including failed ones, and confirm
the evidence directory still has the screenshots.

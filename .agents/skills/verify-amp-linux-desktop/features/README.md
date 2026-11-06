# Amp Linux Desktop verification map

The maintained source for verifying what a user of Amp Linux Desktop sees and does. Read this
index, then use the matching feature file as the recipe.

## Baseline preconditions

- `verify-amp launch RUN` from the repo root succeeded and `verify-amp doctor RUN` reports this run's data dir, signed in, 0 log errors.
- The window is the only `dev.ampnative.Amp` window in session `ampv-RUN`.
- The owner's account is real: no sending, creating, archiving or setting changes unless the feature file says so and the task allows it.

## Driving conventions

- `verify-amp` means `.agents/skills/verify-amp-linux-desktop/bin/verify-amp`; `S` is the session name `ampv-RUN`.
- Screenshot, look, then act. Coordinates are only valid for the screenshot they came from.
- Find visible labels in the screenshot (sidebar titles, tab names such as `Desktop`, `Terminal`, buttons such as `New Thread`) and click their centre.
- Prove page changes with the app log's `ampNavigationState` lines as well as screenshots.

## Proof and skip reporting

- Name the feature ID and entry point with every artifact.
- Before/after screenshots from `verify-amp shot`, plus the log lines with timestamps.
- Report an unreachable path with the command you ran and the missing precondition; never count a different entry point as covering it.

## Feature entry contract

Each feature file has an H1, one paragraph of user-visible behaviour, then exactly four H2s:
`Sub-features`, `How to get to it (user POV)`, `Driving it with verify-amp`, `Gotchas`.

## Features

- [Launch and sign-in state](./launch-and-sign-in.md): start, signed-in home, signed-out start screen.
- [Threads and composer](./threads-and-composer.md): open a thread from the sidebar, type in the composer, typing responsiveness.
- [Desktop-wide actions](./desktop-actions.md): `--action` commands from outside the window.
- [Orb Desktop and Terminal tabs](./orb-tabs.md): remote desktop video and terminal of an orb thread.
- [Theme and text selection](./theme-and-selection.md): Omarchy colours and visible selections.
- [Notifications](./notifications.md): permission card and desktop notifications.
- [Tray and window controls](./tray-and-window.md): the tray menu, its runner switches, minimize and maximize.
- [Runner folders, Import Threads and the runner's CLI](./runner-and-import.md): Choose Projects…, Import Threads…, the not-signed-in runner.

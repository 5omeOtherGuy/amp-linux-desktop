# Launch and sign-in state

Starting the app opens one window that shows Amp's home (sidebar of threads, `New Thread` and `Puck` buttons) when signed in, or a `Sign In` button on Amp's logo when signed out.

## Sub-features

- `launch-home` signed-in start lands on the home page with the thread list.
- `launch-signed-out` a fresh profile shows the `Sign In` start screen.
- `launch-logs` the start writes no warnings or panics.

## How to get to it (user POV)

- The `Amp` entry in the launcher (desktop entry `dev.ampnative.Amp`).
- `amp-linux-desktop` in a terminal.

## Driving it with verify-amp

Preconditions:

- No session `ampv-RUN` exists.

- **Signed-in start.** Run `verify-amp launch RUN`, then `verify-amp shot RUN home`. The screenshot shows the sidebar thread list and `New Thread`; `verify-amp doctor RUN` prints `page: https://ampcode.com/__native/home` and `auth: signed in`.
- **Log check.** `verify-amp doctor RUN` reports `log warnings/errors: 0` and `stderr panics: 0`; read the stderr logs listed by `verify-amp logs RUN` for anything else unexpected.
- **Signed-out start.** `verify-amp cleanup RUN`, then `verify-amp launch RUN2 --signed-out` and `verify-amp shot RUN2 signed-out`. The screenshot shows the Amp logo and a `Sign In` button; doctor prints `page: https://ampcode.com/__native/start`.

## Gotchas

- Do not click `Sign In` in a signed-out run: it opens the system browser and the callback goes to whichever Amp handles `com.ampcode.amp.linux.auth`, i.e. the owner's installed app.
- Headless-session noise in the stderr log (missing accessibility bus, `swaybg` not found) is not an app error.
- The install path (`scripts/install`, launcher, desktop entry, `~/.cache/amp-native/stderr.log` capture) is not exercised by `verify-amp launch`; check it separately with `sh -n scripts/install` and by reading the generated launcher, never by reinstalling over the owner's setup without asking.

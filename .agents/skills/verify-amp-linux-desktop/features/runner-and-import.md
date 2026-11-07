# Runner folders, Import Threads and the runner's CLI

App Settings → Runner lists the folders this computer offers to Amp; its `+` menu has Choose Projects… (a sheet of discovered projects to tick) and Add Folder…. App Settings → General → Other Coding Agents → Import Threads… imports conversations from other coding agents through the Amp CLI. When the CLI is not signed in, the runner row says so and offers Sign In….

## Sub-features

- `runner-choose-projects` Choose Projects… lists projects (All Projects, Added, Locations, Recently Used), filters, ticks and adds them to the runner's folders.
- `thread-import` the dialog lists the agents the CLI detects, imports one or all, offers Re-import All Conversations, and shows errors in red.
- `runner-cli-sign-in` the not-signed-in state and Sign In….

## How to get to it (user POV)

- Amp → App Settings… (Alt shows the menu bar), or the tray's Settings…, → Runner → the `+` beside Directories.
- App Settings → General, scroll to Other Coding Agents, Import Threads….

## Driving it with verify-amp

Preconditions:

- Signed-in launch. For imports, `verify-amp launch RUN --set thread_import_cli=$PWD/.agents/skills/verify-amp-linux-desktop/bin/amp-import-dry-run` (every import a `--dry-run` of one probe conversation; calls are logged to `cli-calls.log`), or `bin/amp-import-fail` to see the error state.

- **Choose Projects.** Open the `+` menu, Choose Projects…, tick a project, Add Projects; the folder appears in the list and in the run's `settings.json`. Remove it again afterwards with its row's remove button.
- **Import.** Open Import Threads…, click Import on Claude Code, screenshot the running and the done state; Import All runs the agents one after another.
- **Not signed in.** `verify-amp launch RUN --set runner_enabled=true`, open App Settings → Runner, screenshot the runner row: "The Amp CLI is not signed in on this computer." with Sign In…. The CLI the app starts inherits the run's `XDG_CONFIG_HOME`/`XDG_DATA_HOME`, finds no credentials there and stops with `not_logged_in` before it registers anything (its `fatal` event is in the run's runner log).

## Gotchas

- Never import with the real CLI and no test double: it uploads the owner's conversations as threads in his account.
- The dry run prints `imported` with a `threadID` and `threadURL`; those threads do not exist (checked 2026-11-06), so nothing needs cleaning up.
- Do not click Sign In… (see SKILL.md, Do not). The signed-in runner is not coverable here.

---
name: onboarding-amp-linux-desktop
description: "Onboards an agent taking over Amp Linux Desktop, the Rust GTK 4/WebKitGTK app in this repository: where to work on the P720, how the code is laid out, how changes are proven and reviewed, what needs the owner's approval, and what is still open. Use when starting or resuming work in this repository without prior context, after a hand-off, or before planning parity, packaging or release work."
---

# Onboarding: Amp Linux Desktop

The app runs ampcode.com in a native window and answers the page's native bridge the way Amp's
macOS app does. 0.2.0 (2026-11-07) was the first release of the Rust app; 0.1.x was a Python app
that 0.2.0 removed.

Read these first, in order. This skill does not repeat them:

1. `~/.agents/AGENTS.md` (workstation rules: commit by explicit path, never kill or delete what
   you did not start or create, Bitwarden, `omarchy pkg add`). Orbs do not have it.
2. [AGENTS.md](../../../AGENTS.md): crate layout, the verify rule, artwork and privacy rules,
   documentation rules, and the release steps.
3. [README.md](../../../README.md): what users get, the macOS parity table, install and upgrade.
4. [verify-amp-linux-desktop](../verify-amp-linux-desktop/SKILL.md) and its `features/` before
   you drive the app. Its "Do not" rules hold in every run.

## Where to work (P720)

- `/home/phasetwo/Work/amp-linux-desktop` is the main checkout on `main`. The owner's installed
  Amp was built from it with `scripts/install`; keep it on `main` and do not commit there.
- Work in a worktree off `origin/main`:
  `git -C /home/phasetwo/Work/amp-linux-desktop worktree add ../amp-linux-desktop-TASK -b TASK origin/main`.
  Run `git fetch` first.
- Each worktree's `target/` grows to 3.5-7 GB with Huddles (measured on this machine,
  November 2026). The first build of each profile downloads LiveKit's libwebrtc (156 MB). Check
  free space on `/` first, and delete your worktree's `target/` when you are done.
- Scratch files and evidence go to `.amp/in/` in your worktree (ignored through `info/exclude`).

## Code map

| Path | Owns |
|---|---|
| `crates/app/src/app.rs` | `App`: startup, single instance, `--action`, windows, quit and hold-to-quit, notifications, badge, open at login |
| `crates/app/src/bridge.rs` | The page's message handlers: `HANDLERS` (no answer) and `REPLY_HANDLERS` (the page awaits a `Reply`; dropping a `Reply` rejects it) |
| `crates/app/src/window.rs` | `AmpWindow`: the WebKit view, navigation policy, `run_js` |
| `crates/app/src/menus.rs` | `MENUS`, `GLOBAL_COMMANDS` (`--action` names), `GLOBAL_SHORTCUTS` |
| `crates/app/src/tray.rs` | StatusNotifierItem and dbusmenu tray |
| `crates/app/src/settings_window.rs` | The native Settings window |
| `crates/app/src/runner.rs`, `choose_projects.rs`, `local_projects.rs` | "Use this computer as a runner" through the Amp CLI |
| `crates/app/src/thread_import.rs` | Import Threads… through `amp threads import` |
| `crates/app/src/signin.rs` | Browser sign-in and the `com.ampcode.amp.linux.auth` callback |
| `crates/app/src/file_drag.rs`, `files.rs`, `confirmation.rs`, `toasts.rs` | Drag out, save/open/paste, confirmation sheets, toasts |
| `crates/common` | Paths, settings, page scripts (`scripts.rs`: `SHELL_MARKERS` sets `dataset.ampShell = 'macos-v1'`), theme, icon extraction. No GTK |
| `crates/dictation`, `crates/huddle` | GStreamer dictation; Huddles in their own window through LiveKit's Rust SDK, because distribution WebKitGTK has no WebRTC |

The app's files are `~/.config/amp-native/settings.json`, `~/.local/share/amp-native/web`
(cookies), and `~/.cache/amp-native/` (`amp-native.log`, `stderr.log`, `runner.log`). The Python
app used the same paths and app ID `dev.ampnative.Amp`, so an upgrade from 0.1.x keeps settings
and sign-in. A second start of the binary hands over to the running instance (single instance on
the session bus); the verify skill avoids that with a private bus per run. The binary has no `--version` option: it prints "Unknown
option" and exits.

The page decides what the shell supports from the `dataset.ampShell` marker and from which
`window.webkit.messageHandlers` exist. To learn a page contract, download ampcode.com's
`/_app/immutable/` chunks and read them; do not run them. For example,
`ampNativeAccountSwitch` is posted only on iOS (read on 2026-11-06).

## How work is done here

- **Spec:** for parity work, use the Mac app's behaviour. The parity table artifact and the
  three-step method are in the verify skill under "Compare against the Mac app". Earlier Mac spec
  screenshots are in `/home/phasetwo/Work/amp-linux-desktop-rust-parity/.amp/in/spec/`.
- **Commits:** make one commit per item. A commit message says what users see and why.
- **Proof:** run `verify-amp` with inspected before and after screenshots, one per visible state
  the change adds or alters. Then run `cargo fmt --check` and
  `cargo clippy --workspace --all-targets -- -D warnings`, with and without
  `--no-default-features`. `cargo test --workspace` has one unit test (2026-11-08), so a passing
  test run proves little.
- **Review risky changes:** concurrency, D-Bus, process lifetimes, packaging and auth need a
  GPT-6.1 Sol review on high reasoning, run in tmux:
  `codex exec -m gpt-6.1-sol -c model_reasoning_effort=high -s read-only -o OUT.md - < BRIEF.md`.
  The briefs and rounds from the parity sprint are examples, in
  `/home/phasetwo/Work/amp-linux-desktop-rust-parity/.amp/in/sol-review/`.
- **Package checks:** run them in Docker with `sudo -n docker`.
  - `.deb`: `debian:trixie` with `clang-21` from trixie-backports, built with `CC=clang-21 CXX=clang++-21`.
  - `.rpm`: `fedora:43`. Run `make -f .copr/Makefile srpm`, then `rpmbuild --rebuild` under `unshare -n` to prove the build needs no network.
  - The webrtc-sys build script rejects g++ and clang older than 21.
  - Starting the GUI in a container needs `docker run --privileged` so that WebKit's bubblewrap sandbox can mount `/proc`. Use `xvfb-run` with `dbus-run-session`, `GDK_BACKEND=x11` and `BROWSER=/bin/true` (0.2.0 release, 2026-11-07).

## Authorization

Approvals from earlier threads do not carry over to you. Ask the owner, Simon, before each of
these:

- Pushing to `main`, merging, or opening a PR.
- A release: version bump, tag, GitHub release, AUR push.
- Running `scripts/install` or installing a package on the workstation.
- Changing his running Amp or its launchers.
- Any test that creates threads, sends messages, starts calls or changes account settings.

Pushing a feature branch is reversible. The COPR webhook builds every pushed tag; AGENTS.md says
branch pushes build nothing, but a new branch also sends a `create` event, so check COPR's build
list after your first branch push.

## State at hand-off (2026-11-08)

Recheck each line before you rely on it. The full hand-off record, with evidence, is the thread
https://ampcode.com/threads/T-01a1b207-0644-705f-888d-0f28d58ff506.

- **Release:** `origin/main` is at `4e34fb9`, and tag `v0.2.0` points at it.
  - The GitHub release has the `.deb` and `SHA256SUMS`.
  - COPR build 11284359 succeeded for Fedora 43, 44, 45 and rawhide.
  - The AUR has no package: `~/.ssh/aur` is not registered with any AUR account, and the owner has to register one.
  - Check with `gh release view v0.2.0` and the COPR API `api_3/build/list/?ownername=5omeotherguy&projectname=amp-linux-desktop`.
- **The owner's install:**
  - "Amp" is the Rust binary `~/.local/bin/amp-linux-desktop`, installed by `scripts/install` from `main` at `4e34fb9` (0.2.0) on 2026-11-08. It is the only launcher.
  - The owner retired the Python app on 2026-11-08: its launcher "Amp Python", its LiveKit folder `~/.local/share/amp-native/python` and `amp_native/` in the main checkout are gone.
- **Open product gaps, in the README parity table and Known limitations:**
  - **Runner status:** the runner reads "Running" when the CLI's OAuth token is invalid. The CLI does not report that, so the fix belongs in the CLI.
  - **Local project import:** Grok is missing.
  - **Add Another Account:** never run end to end.
  - **Page handlers with no Linux code:** `ampNativeSudoAuthenticated`, `ampNativeTranscriptSelectionV2` and `ampNativePuckVoice`.
  - **Confirmation sheets:** they answer protocol v2, while the Mac app answers v5.
  - **Huddles:** no screen share and no transcription.
  - **Notifications:** no action buttons.
  - **File drag:** after a drag, the page's first `pointerdown` is lost.
  - **No Linux equivalent:** the Share extension and Shortcuts actions.
- **Verification gaps:**
  - The packages were never tested signed in, nor on a real Debian, Ubuntu or Fedora desktop.
  - x86_64 only.
  - Microphone, held keys, and the runner signed in cannot be covered in the headless session (see the verify skill).
- **Docs debt:** the README parity intro cites disk image build 1125, while the package uses 1180. When you cite a newer build, recheck every row, as AGENTS.md says.

"""Runs `amp --no-tui` so threads from ampcode.com can run on this computer, like the Mac app's
"Use This Mac as a Runner" (MacRunnerController).

The runner reports its lifecycle as one JSON object per line (`--output-format json`): hello
(CLI version and capabilities), starting, directories, registered, waiting_for_directories,
connection_interrupted, reconnected, registration_lost, update_relaunching, shutting_down, and
fatal { code, message }. This class turns those into the status the settings show, keeps the
served directories in step with the settings, and installs the CLI when it is missing.

Settings it reads (missing keys use the defaults below):
- runner_home: serve the home directory; otherwise only the directories below (--no-serve-cwd).
- runner_dirs: directories added with `amp runner dirs add`, which the CLI also remembers.
- runner_discover_dirs: serve every Git checkout found beneath these (--discover-dirs).
- runner_new_dirs: where New Directory and New Project create folders (--create-dirs-in).
- runner_share_network: let orbs browse through this computer's connection (--share-network).
- runner_desktop, runner_terminal, runner_keep_awake: as before.
"""

import json
import logging
import os
import shutil
import signal
import subprocess
import time
from pathlib import Path

from gi.repository import GLib

from . import config

log = logging.getLogger(__name__)

CLI_CANDIDATES = [Path.home() / ".amp/bin/amp", Path.home() / ".local/bin/amp", Path("/usr/local/bin/amp"), Path("/usr/bin/amp")]
INSTALL_COMMAND = "curl --fail --silent --show-error --location https://ampcode.com/install.sh | bash"
DEFAULTS = {
    "runner_discover_dirs": [],
    "runner_new_dirs": "",
    "runner_share_network": False,
    "runner_dirs_removed": [],  # removed while the runner was off; the CLI still remembers them
}
# Flags an older CLI may not know. When it rejects one, the runner starts again without it.
OPTIONAL_FLAGS = ("--share-network", "--runner-account-email", "--create-dirs-in", "--discover-dirs", "--no-serve-cwd", "--desktop",
                  "--remote-control-terminal", "--no-remote-control-terminal")
RESTARTS = 3
RESTART_WINDOW_S = 120
FATAL_MESSAGES = {
    "not_logged_in": "The Amp CLI is not signed in on this computer. Run amp login, then turn the runner on again.",
    "credentials_rejected": "Amp rejected the CLI’s sign-in. Run amp login again.",
    "runner_id_in_use": "Another live runner uses this Runner ID. Rename one so this app’s runner can start.",
    "feature_not_enabled": "Running threads on your own computer is not enabled for your account.",
}
STATUS_MESSAGES = {
    "installing": "Installing the Amp CLI…",
    "starting": "Starting…",
    "registering": "Registering with Amp…",
    "waiting_for_directories": "Add directories to get started.",
    "connection_interrupted": "Connection to Amp interrupted. Reconnecting…",
    "registration_lost": "Registering with the server again.",
    "update_relaunching": "Updating the Amp CLI…",
}


def find_cli():
    found = shutil.which("amp")
    if found:
        return found
    for candidate in CLI_CANDIDATES:
        if os.access(candidate, os.X_OK):
            return str(candidate)
    return None


class Runner:
    def __init__(self, app):
        self.app = app
        self.settings = app.settings
        self.process = None
        self.last_error = None
        self.state = "off"  # off | installing | starting | registering | registered | the CLI's states
        self.account = None
        self.cli_version = None
        self.capabilities = set()
        self.directories = []  # [{ path, repositoryURL, canCreateWorktree }] the runner serves
        self.unsupported = set()  # optional flags this CLI rejected
        self.account_email = None  # the account signed in to the app, which the runner must use
        self._installer = None
        self._exits = []
        self._fatal = None
        self._rejected_flag = None
        self._synced = False
        self._removing = set()
        self._buffers = {}

    def get(self, key):
        return self.settings.get(key, DEFAULTS.get(key))

    @property
    def runner_id(self):
        return self.settings["runner_name"]

    @property
    def running(self):
        return self.process is not None and self.process.poll() is None

    def status(self):
        if self.running:
            return "running"
        if self.state == "installing":
            return "installing"
        return "error" if self.last_error else "off"

    def summary(self):
        """One line for the settings: what the runner is doing or why it stopped."""
        if self.last_error:
            return self.last_error
        if self.state == "registered" and self.running:
            served = len(self.directories)
            where = f"{served} director{'y' if served == 1 else 'ies'}" if served else "no directories yet"
            account = f" as {self.account}" if self.account else ""
            return f"Running{account} with Runner ID {self.runner_id}, serving {where}."
        if self.state in STATUS_MESSAGES and (self.running or self.state in ("installing", "starting")):
            return STATUS_MESSAGES[self.state]
        return "Off"

    def missing_features(self):
        """Settings this CLI cannot apply, so the settings can say it needs a newer CLI."""
        wanted = {"--share-network": self.get("runner_share_network"), "--create-dirs-in": bool(self.get("runner_new_dirs")),
                  "--discover-dirs": bool(self.get("runner_discover_dirs")), "--desktop": self.settings["runner_desktop"]}
        return sorted(flag for flag, on in wanted.items() if on and flag in self.unsupported)

    # ---- command -----------------------------------------------------------------------------

    def command(self):
        cli = find_cli()
        if not cli:
            return None
        args = [cli, "--no-tui", "--runner-id", self.runner_id, "--output-format", "json"]
        optional = []
        if self.account_email:
            optional.append(["--runner-account-email", self.account_email])
        if not self.settings["runner_home"]:
            optional.append(["--no-serve-cwd"])
        for directory in self.get("runner_discover_dirs") or []:
            optional.append([f"--discover-dirs={directory}"])
        if self.get("runner_new_dirs"):
            optional.append([f"--create-dirs-in={self.get('runner_new_dirs')}"])
        if self.get("runner_share_network"):
            optional.append(["--share-network"])
        if self.settings["runner_desktop"]:
            optional.append(["--desktop"])
        optional.append(["--remote-control-terminal" if self.settings["runner_terminal"] else "--no-remote-control-terminal"])
        for flag in optional:
            if flag[0].split("=")[0] not in self.unsupported:
                args += flag
        if self.settings["runner_keep_awake"] and shutil.which("systemd-inhibit"):
            args = ["systemd-inhibit", "--what=sleep:idle", "--who=Amp", "--why=Amp runner is serving threads", *args]
        return args

    def account_changed(self, email):
        """The app's signed-in account (app state signedInEmail). The runner serves threads for
        that account, as the Mac app's does, and restarts when it changes."""
        email = email if isinstance(email, str) and email else None
        if email == self.account_email:
            return
        self.account_email = email
        if email and self.settings["runner_enabled"] and (self.running or self.last_error):
            self.start()

    def working_directory(self):
        return Path.home() if self.settings["runner_home"] else config.DATA_DIR / "runner"

    # ---- lifecycle ---------------------------------------------------------------------------

    def start(self):
        self.stop()
        self.last_error = None
        self._fatal = None
        self._rejected_flag = None
        self._synced = False
        self._removing = set()  # being removed now; not to be adopted back
        self.directories = []
        args = self.command()
        if args is None:
            self.install_cli()
            return
        cwd = self.working_directory()
        cwd.mkdir(parents=True, exist_ok=True)
        self._log(f"starting {' '.join(args)} in {cwd}")
        try:
            self.process = subprocess.Popen(args, cwd=str(cwd), stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                            stderr=subprocess.PIPE, start_new_session=True)
        except OSError as error:
            self.last_error = str(error)
            self.process = None
            self.state = "off"
        else:
            self.state = "starting"
            process = self.process
            for stream, handler in ((process.stdout, self._stdout_line), (process.stderr, self._stderr_line)):
                os.set_blocking(stream.fileno(), False)
                self._buffers[stream.fileno()] = b""
                GLib.io_add_watch(stream.fileno(), GLib.PRIORITY_DEFAULT, GLib.IO_IN | GLib.IO_HUP | GLib.IO_ERR,
                                  self._readable, stream, handler)
            _watch_exit(process, self._exited)
        self.app.runner_changed()

    def stop(self):
        if self.running:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        self.process = None
        if self.state != "installing":
            self.state = "off"

    def apply(self):
        """Starts, restarts, or stops the runner to match the settings."""
        if self.settings["runner_enabled"]:
            self.start()
        else:
            self.stop()
            self.last_error = None
            self.app.runner_changed()

    def _exited(self, process, code):
        if process is not self.process:
            return  # stopped, or replaced by a restart
        self.process = None
        self._log(f"runner exited with {code}")
        if self._rejected_flag:
            self.unsupported.add(self._rejected_flag)
            self._log(f"the CLI does not know {self._rejected_flag}; starting without it")
            GLib.idle_add(lambda: self.start() and False)
            return
        if self._fatal:
            code_name, message = self._fatal
            self.last_error = message or FATAL_MESSAGES.get(code_name) or "The runner stopped."
            self.state = "off"
            self.app.runner_changed()
            return
        now = time.monotonic()
        self._exits = [t for t in self._exits if now - t < RESTART_WINDOW_S] + [now]
        if self.settings["runner_enabled"] and len(self._exits) <= RESTARTS:
            self.state = "starting"
            GLib.timeout_add_seconds(2 * len(self._exits), lambda: self.settings["runner_enabled"] and self.start() and False)
        else:
            self.last_error = f"The runner exited repeatedly (exit code {code}). See {config.RUNNER_LOG}."
            self.state = "off"
        self.app.runner_changed()

    # ---- output ------------------------------------------------------------------------------

    def _readable(self, fd, condition, stream, handler):
        try:
            chunk = os.read(fd, 65536)
        except BlockingIOError:
            return True
        except OSError:
            chunk = b""
        data = self._buffers.get(fd, b"") + chunk
        *lines, rest = data.split(b"\n")
        self._buffers[fd] = rest
        for line in lines:
            handler(line.decode("utf-8", "replace"))
        if not chunk:
            if rest:
                handler(rest.decode("utf-8", "replace"))
            self._buffers.pop(fd, None)
            stream.close()
            return False
        return True

    def _stderr_line(self, line):
        self._log(line, raw=True)
        marker = "unknown option '"
        if marker in line:
            flag = line.split(marker, 1)[1].split("'", 1)[0].split("=")[0]
            if flag in OPTIONAL_FLAGS:
                self._rejected_flag = flag

    def _stdout_line(self, line):
        self._log(line, raw=True)
        try:
            event = json.loads(line)
        except ValueError:
            return
        if not isinstance(event, dict):
            return
        kind = event.get("type")
        if kind == "hello":
            self.cli_version = event.get("cliVersion")
            self.capabilities = set(event.get("capabilities") or [])
        elif kind == "starting":
            self.state = "registering"
        elif kind == "directories":
            self.directories = [d for d in event.get("directories") or [] if isinstance(d, dict)]
            self._sync_dirs(first=not self._synced)
            self._synced = True
        elif kind in ("registered", "reconnected"):
            self.state = "registered"
            self.account = event.get("accountEmail") or self.account
        elif kind == "waiting_for_directories":
            self.state = kind
        elif kind in ("connection_interrupted", "registration_lost", "update_relaunching"):
            self.state = kind
        elif kind == "fatal":
            self._fatal = (event.get("code"), event.get("message"))
        else:
            return
        self.app.runner_changed()

    def _log(self, line, raw=False):
        try:
            config.CACHE_DIR.mkdir(parents=True, exist_ok=True)
            with open(config.RUNNER_LOG, "a", encoding="utf-8") as f:
                f.write(f"{line}\n" if raw else f"[amp-native] {line}\n")
        except OSError:
            pass

    # ---- directories -------------------------------------------------------------------------

    def _sync_dirs(self, first):
        """Brings the CLI's remembered directories and runner_dirs together: adds settings
        directories the CLI lacks, removes ones taken out of the settings while the runner was off,
        and adopts ones added from a terminal with `amp runner dirs add`. Adds and removes only on
        the first report after starting; adopts on every report."""
        wanted = list(dict.fromkeys(self.settings["runner_dirs"]))
        removed = set(self.get("runner_dirs_removed") or []) | self._removing
        served = [d.get("path") for d in self.directories if d.get("path")]
        roots = [str(Path(d)) for d in self.get("runner_discover_dirs") or []] + [str(self.working_directory())]
        found = lambda p: any(p == r or p.startswith(r.rstrip("/") + "/") for r in roots)  # noqa: E731
        missing = [d for d in wanted if d not in served]
        extra = [p for p in served if p not in wanted and not found(p)]
        if first:
            if missing:
                self.dirs_command("add", *missing)
            if extra:
                self._removing = {p for p in extra if p in removed}
                self.dirs_command("remove", *self._removing)
            self.settings.set("runner_dirs_removed", [])
        adopted = [p for p in extra if p not in removed]
        if adopted:
            self.settings.set("runner_dirs", wanted + adopted)
            if self.app.settings_window:
                self.app.settings_window.refresh_runner()

    def dirs_command(self, action, *directories):
        if not directories:
            return
        if not self.running:
            # The CLI remembers added directories; forget removed ones the next time it runs.
            removed = set(self.get("runner_dirs_removed") or [])
            removed = removed | set(directories) if action == "remove" else removed - set(directories)
            self.settings.set("runner_dirs_removed", sorted(removed))
            return
        cli = find_cli()
        if not cli:
            return
        batches = [list(directories)] if f"dirs-{action}-many" in self.capabilities else [[d] for d in directories]
        for batch in batches:
            subprocess.Popen([cli, "runner", "dirs", action, "--runner-id", self.runner_id, *batch],
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    # ---- CLI install -------------------------------------------------------------------------

    def install_cli(self):
        """Installs the Amp CLI the way the Mac app does, then starts the runner."""
        if self._installer is not None:
            return
        if not shutil.which("curl") or not shutil.which("bash"):
            self.last_error = "The Amp CLI is not installed, and installing it needs curl. Install it from ampcode.com/install."
            self.app.runner_changed()
            return
        self.state = "installing"
        self.last_error = None
        self._log("running the Amp CLI installer")
        log_file = open(config.RUNNER_LOG, "ab")
        try:
            self._installer = subprocess.Popen(["bash", "-c", INSTALL_COMMAND], stdin=subprocess.DEVNULL,
                                               stdout=log_file, stderr=log_file, start_new_session=True)
        except OSError as error:
            self.state = "off"
            self.last_error = f"The Amp CLI installer could not be started: {error}"
        else:
            _watch_exit(self._installer, self._installed)
        finally:
            log_file.close()
        self.app.runner_changed()

    def _installed(self, _process, code):
        self._installer = None
        self._log(f"Amp CLI installer exited with {code}")
        self.state = "off"
        if code != 0 or find_cli() is None:
            self.last_error = (f"The Amp CLI installer exited with code {code}. Check {config.RUNNER_LOG}, "
                               "or install the CLI from ampcode.com/install, then retry.")
            self.app.runner_changed()
        elif self.settings["runner_enabled"]:
            self.start()
        else:
            self.app.runner_changed()


def _watch_exit(process, callback):
    """Calls callback(process, exit code) once the process exits. Polls, because Popen.poll()
    elsewhere would reap the child before a GLib child watch saw it."""

    def check():
        code = process.poll()
        if code is None:
            return True
        callback(process, code)
        return False

    GLib.timeout_add(250, check)

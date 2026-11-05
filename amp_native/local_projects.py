"""Finds Git checkouts on this computer for the page's local project import
(ampNativeImportLocalProjects, version 1).

Actions, as the Mac app's MacImportLocalProjectsMessageHandler answers them:
- discover: checkouts other coding agents worked in; browse: checkouts in folders the user picks.
  Both reply { status: ok | cancelled | busy, repositories: [{ path, name, remoteURL, sources }],
  deniedPaths }.
- recordProjectFolders { projectID, paths }: serve the folders from this computer's runner,
  which the runner settings and `amp runner dirs add` already do.
- openFilesAndFoldersSettings: macOS privacy settings; Linux has none, so it is unavailable.
"""

import json
import logging
import sqlite3
import subprocess
import threading
from pathlib import Path
from urllib.parse import unquote, urlsplit

from gi.repository import GLib, Gtk

log = logging.getLogger(__name__)

HOME = Path.home()
BROWSE_DEPTH = 2  # like the Mac runner's "All Projects Under Directories"
CLAUDE_LINES = 50

_browsing = False


def handle(window, body, reply):
    action = body.get("action") if isinstance(body, dict) and body.get("version") == 1 else None
    if action == "discover":
        _in_background(discover, reply)
    elif action == "browse":
        browse(window, reply)
    elif action == "recordProjectFolders":
        reply(record_project_folders(window.app, body.get("paths")))
    else:
        reply({"status": "unavailable"})


def _in_background(work, reply, *args):
    def run():
        try:
            result = work(*args)
        except Exception:
            log.exception("local project scan failed")
            result = {"status": "unavailable"}
        GLib.idle_add(lambda: reply(result) and False)

    threading.Thread(target=run, daemon=True).start()


def find_projects(folders, done):
    """For the runner settings' project picker, like the Mac app's Choose Projects: the Git
    checkouts other coding agents worked in plus those up to two levels inside folders. Calls
    done({ status, repositories, deniedPaths }) on the main loop."""

    def work():
        agents, scanned = discover(), scan_folders(folders)
        merged = {r["path"]: r for r in scanned["repositories"]}
        for repo in agents["repositories"]:
            merged[repo["path"]] = repo
        return {
            "status": "ok",
            "repositories": sorted(merged.values(), key=lambda r: r["path"]),
            "deniedPaths": sorted(set(agents["deniedPaths"]) | set(scanned["deniedPaths"])),
        }

    _in_background(work, done)


# ---- discover ------------------------------------------------------------------------------


def discover():
    found, denied = {}, set()
    for source, paths in (
        ("claude-code", _claude_code),
        ("codex", _codex),
        ("cursor", _cursor),
        ("opencode", _opencode),
    ):
        try:
            for path in paths():
                found.setdefault(path, set()).add(source)
        except PermissionError as error:
            denied.add(error.filename)
    return _result(found, denied)


def _jsonl_lines(path, limit):
    with open(path, encoding="utf-8", errors="replace") as f:
        for _, line in zip(range(limit), f):
            try:
                yield json.loads(line)
            except ValueError:
                continue


def _claude_code():
    root = HOME / ".claude" / "projects"
    if not root.is_dir():
        return
    for project in root.iterdir():
        for log_file in sorted(project.glob("*.jsonl"), key=lambda p: p.stat().st_mtime, reverse=True)[:3]:
            cwd = next((e["cwd"] for e in _jsonl_lines(log_file, CLAUDE_LINES)
                        if isinstance(e, dict) and isinstance(e.get("cwd"), str)), None)
            if cwd:
                yield cwd
                break


def _codex():
    root = HOME / ".codex"
    for folder in (root / "sessions", root / "archived_sessions"):
        if not folder.is_dir():
            continue
        for session in folder.rglob("rollout-*.jsonl"):
            for entry in _jsonl_lines(session, 1):
                cwd = (entry.get("payload") or {}).get("cwd") if isinstance(entry, dict) else None
                if isinstance(cwd, str):
                    yield cwd


def _cursor():
    root = Path(GLib.get_user_config_dir()) / "Cursor" / "User" / "workspaceStorage"
    if not root.is_dir():
        return
    for workspace in root.glob("*/workspace.json"):
        try:
            folder = json.loads(workspace.read_text()).get("folder")
        except (OSError, ValueError):
            continue
        if isinstance(folder, str) and folder.startswith("file://"):
            yield unquote(urlsplit(folder).path)


def _opencode():
    root = Path(GLib.get_user_data_dir()) / "opencode"
    for database in (root / "opencode.db", *root.glob("*.db")):
        if not database.is_file():
            continue
        try:
            connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
        except sqlite3.Error:
            continue
        with connection:
            for query in (
                "SELECT worktree FROM project WHERE worktree IS NOT NULL",
                "SELECT directory FROM project_directory WHERE directory IS NOT NULL",
            ):
                try:
                    yield from (row[0] for row in connection.execute(query) if isinstance(row[0], str))
                except sqlite3.Error:
                    continue
        connection.close()
        return


# ---- browse --------------------------------------------------------------------------------


def browse(window, reply):
    global _browsing
    if _browsing:
        reply({"status": "busy"})
        return
    _browsing = True
    chooser = Gtk.FileChooserNative(
        title="Choose Git repositories, or a folder that contains them",
        transient_for=window,
        modal=True,
        action=Gtk.FileChooserAction.SELECT_FOLDER,
        select_multiple=True,
        accept_label="Choose",
    )

    def responded(dialog, response):
        global _browsing
        _browsing = False
        folders = []
        if response == Gtk.ResponseType.ACCEPT:
            files = dialog.get_files()
            folders = [files.get_item(i).get_path() for i in range(files.get_n_items())]
        dialog.destroy()
        folders = [f for f in folders if f]
        if not folders:
            reply({"status": "cancelled"})
        else:
            _in_background(scan_folders, reply, folders)

    chooser.connect("response", responded)
    chooser.show()
    window._import_chooser = chooser  # keep it alive until it answers


def scan_folders(folders):
    found, denied = {}, set()

    def walk(folder, depth):
        if (folder / ".git").exists():
            found.setdefault(str(folder), set())
            return
        if depth == 0:
            return
        try:
            children = sorted(p for p in folder.iterdir() if p.is_dir() and not p.name.startswith("."))
        except PermissionError:
            denied.add(str(folder))
            return
        for child in children:
            walk(child, depth - 1)

    for folder in folders:
        walk(Path(folder), BROWSE_DEPTH)
    return _result(found, denied)


# ---- results -------------------------------------------------------------------------------


def _git(path, *args):
    try:
        out = subprocess.run(["git", "-C", path, *args], capture_output=True, text=True, timeout=5,
                             stdin=subprocess.DEVNULL)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return out.stdout.strip() if out.returncode == 0 else None


def _repository(path):
    top = _git(path, "rev-parse", "--show-toplevel")
    if not top:
        return None
    remote = _git(top, "config", "--get", "remote.origin.url")
    if not remote:
        first = (_git(top, "remote") or "").split("\n")[0]
        remote = _git(top, "remote", "get-url", first) if first else None
    return top, remote or None


def _result(found, denied):
    repositories = {}
    for path, sources in found.items():
        if not Path(path).is_dir():
            continue
        try:
            repo = _repository(path)
        except PermissionError:
            denied.add(path)
            continue
        if repo is None:
            continue
        top, remote = repo
        entry = repositories.setdefault(top, {"path": top, "name": Path(top).name, "remoteURL": remote, "sources": set()})
        entry["sources"] |= sources
    return {
        "status": "ok",
        "repositories": [{**r, "sources": sorted(r["sources"])} for r in sorted(repositories.values(), key=lambda r: r["path"])],
        "deniedPaths": sorted(d for d in denied if d),
    }


# ---- record --------------------------------------------------------------------------------


def record_project_folders(app, paths):
    if not isinstance(paths, list) or not all(isinstance(p, str) and p.startswith("/") for p in paths):
        return {"status": "invalid"}
    dirs = list(app.settings["runner_dirs"])
    for path in paths:
        if path not in dirs:
            dirs.append(path)
            app.runner.dirs_command("add", path)
    app.settings.set("runner_dirs", dirs)
    if app.settings_window:
        app.settings_window.refresh_runner()
    return {"status": "ok"}

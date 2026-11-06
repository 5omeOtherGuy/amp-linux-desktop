//! Finds Git checkouts on this computer for the page's local project import
//! (ampNativeImportLocalProjects, version 1).
//!
//! Actions, as the Mac app's MacImportLocalProjectsMessageHandler answers them:
//! - discover: checkouts other coding agents worked in; browse: checkouts in folders the user
//!   picks. Both reply { status: ok | cancelled | busy, repositories: [{ path, name, remoteURL,
//!   sources }], deniedPaths }.
//! - recordProjectFolders { projectID, paths }: serve the folders from this computer's runner,
//!   which the runner settings and `amp runner dirs add` already do.
//! - openFilesAndFoldersSettings: macOS privacy settings; Linux has none, so it is unavailable.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use gtk::glib;
use serde_json::{Value, json};

/// Like the Mac runner's "All Projects Under Directories".
const BROWSE_DEPTH: u32 = 2;
const CLAUDE_LINES: usize = 50;

/// Paths a scan found, with the agents that worked in each, and the folders it could not read.
#[derive(Default)]
struct Found {
    paths: BTreeMap<String, BTreeSet<&'static str>>,
    denied: BTreeSet<String>,
}

fn is_denied(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied
}

/// Runs `work` off the main thread and returns its result.
pub async fn in_background(work: impl FnOnce() -> Value + Send + 'static) -> Value {
    gtk::gio::spawn_blocking(work).await.unwrap_or_else(|_| {
        log::error!("local project scan failed");
        json!({"status": "unavailable"})
    })
}

// ---- discover ----------------------------------------------------------------------------------

pub fn discover() -> Value {
    result(discover_paths())
}

fn discover_paths() -> Found {
    let mut found = Found::default();
    let sources: [(&'static str, Source); 4] =
        [("claude-code", claude_code), ("codex", codex), ("cursor", cursor), ("opencode", opencode)];
    for (source, paths) in sources {
        let mut list = Vec::new();
        let outcome = paths(&mut list);
        for path in list {
            found.paths.entry(path).or_default().insert(source);
        }
        if let Err((error, path)) = outcome
            && is_denied(&error)
        {
            found.denied.insert(path.to_string_lossy().to_string());
        }
    }
    found
}

fn jsonl_lines(path: &Path, limit: usize) -> impl Iterator<Item = Value> {
    let file = std::fs::File::open(path).ok();
    file.into_iter().flat_map(move |f| {
        BufReader::new(f)
            .split(b'\n')
            .take(limit)
            .map_while(Result::ok)
            .filter_map(|line| serde_json::from_slice::<Value>(&line).ok())
    })
}

/// A scan's outcome: the folder it could not read, with why.
type Scan = Result<(), (std::io::Error, PathBuf)>;
/// Adds the folders one agent worked in.
type Source = fn(&mut Vec<String>) -> Scan;

fn read_dir(path: &Path) -> Result<std::fs::ReadDir, (std::io::Error, PathBuf)> {
    std::fs::read_dir(path).map_err(|e| (e, path.to_path_buf()))
}

fn home() -> PathBuf {
    glib::home_dir()
}

fn claude_code(out: &mut Vec<String>) -> Scan {
    let root = home().join(".claude/projects");
    if !root.is_dir() {
        return Ok(());
    }
    for project in read_dir(&root)?.flatten() {
        let Ok(entries) = std::fs::read_dir(project.path()) else { continue };
        let mut logs: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
            .collect();
        logs.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
        for (_, log_file) in logs.into_iter().take(3) {
            let cwd = jsonl_lines(&log_file, CLAUDE_LINES).find_map(|e| e.get("cwd")?.as_str().map(String::from));
            if let Some(cwd) = cwd {
                out.push(cwd);
                break;
            }
        }
    }
    Ok(())
}

fn rollouts(folder: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            rollouts(&path, out);
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && name.starts_with("rollout-")
            && name.ends_with(".jsonl")
        {
            out.push(path);
        }
    }
}

fn codex(out: &mut Vec<String>) -> Scan {
    let root = home().join(".codex");
    for folder in [root.join("sessions"), root.join("archived_sessions")] {
        let mut sessions = Vec::new();
        rollouts(&folder, &mut sessions);
        for session in sessions {
            for entry in jsonl_lines(&session, 1) {
                if let Some(cwd) = entry.get("payload").and_then(|p| p.get("cwd")).and_then(Value::as_str) {
                    out.push(cwd.to_string());
                }
            }
        }
    }
    Ok(())
}

fn cursor(out: &mut Vec<String>) -> Scan {
    let root = glib::user_config_dir().join("Cursor/User/workspaceStorage");
    if !root.is_dir() {
        return Ok(());
    }
    for workspace in read_dir(&root)?.flatten() {
        let Ok(text) = std::fs::read(workspace.path().join("workspace.json")) else { continue };
        let Ok(json) = serde_json::from_slice::<Value>(&text) else { continue };
        if let Some(folder) = json.get("folder").and_then(Value::as_str)
            && folder.starts_with("file://")
            && let Some(path) = url::Url::parse(folder).ok().and_then(|u| u.to_file_path().ok())
        {
            out.push(path.to_string_lossy().to_string());
        }
    }
    Ok(())
}

fn opencode(out: &mut Vec<String>) -> Scan {
    let root = glib::user_data_dir().join("opencode");
    let mut databases = vec![root.join("opencode.db")];
    if let Ok(entries) = std::fs::read_dir(&root) {
        let mut others: Vec<PathBuf> =
            entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "db")).collect();
        others.sort();
        databases.extend(others);
    }
    for database in databases {
        if !database.is_file() {
            continue;
        }
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI;
        let Ok(connection) = rusqlite::Connection::open_with_flags(&database, flags) else { continue };
        for query in [
            "SELECT worktree FROM project WHERE worktree IS NOT NULL",
            "SELECT directory FROM project_directory WHERE directory IS NOT NULL",
        ] {
            let Ok(mut statement) = connection.prepare(query) else { continue };
            if let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(0)) {
                out.extend(rows.flatten());
            }
        }
        return Ok(());
    }
    Ok(())
}

// ---- browse ------------------------------------------------------------------------------------

/// Git checkouts in `folders` and up to two levels below them.
pub fn scan_folders(folders: &[String]) -> Value {
    result(scan_paths(folders))
}

fn scan_paths(folders: &[String]) -> Found {
    fn walk(folder: &Path, depth: u32, found: &mut Found) {
        if folder.join(".git").exists() {
            found.paths.entry(folder.to_string_lossy().to_string()).or_default();
            return;
        }
        if depth == 0 {
            return;
        }
        let entries = match std::fs::read_dir(folder) {
            Ok(entries) => entries,
            Err(error) => {
                if is_denied(&error) {
                    found.denied.insert(folder.to_string_lossy().to_string());
                }
                return;
            }
        };
        let mut children: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
            .collect();
        children.sort();
        for child in children {
            walk(&child, depth - 1, found);
        }
    }
    let mut found = Found::default();
    for folder in folders {
        walk(Path::new(folder), BROWSE_DEPTH, &mut found);
    }
    found
}

// ---- results -----------------------------------------------------------------------------------

fn git(path: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                std::io::Read::read_to_string(child.stdout.as_mut()?, &mut out).ok()?;
                return status.success().then(|| out.trim().to_string());
            }
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn repository(path: &str) -> Option<(String, Option<String>)> {
    let top = git(path, &["rev-parse", "--show-toplevel"]).filter(|t| !t.is_empty())?;
    let mut remote = git(&top, &["config", "--get", "remote.origin.url"]).filter(|r| !r.is_empty());
    if remote.is_none() {
        let first = git(&top, &["remote"]).unwrap_or_default().lines().next().unwrap_or("").to_string();
        if !first.is_empty() {
            remote = git(&top, &["remote", "get-url", &first]).filter(|r| !r.is_empty());
        }
    }
    Some((top, remote))
}

fn result(found: Found) -> Value {
    let Found { paths, mut denied } = found;
    let mut repositories: BTreeMap<String, (Option<String>, BTreeSet<&'static str>)> = BTreeMap::new();
    for (path, sources) in paths {
        match std::fs::metadata(&path) {
            Ok(m) if m.is_dir() => {}
            Err(error) if is_denied(&error) => {
                denied.insert(path);
                continue;
            }
            _ => continue,
        }
        let Some((top, remote)) = repository(&path) else { continue };
        repositories.entry(top).or_insert_with(|| (remote, BTreeSet::new())).1.extend(sources);
    }
    json!({
        "status": "ok",
        "repositories": repositories.into_iter().map(|(path, (remote, sources))| json!({
            "name": Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            "path": path.clone(),
            "remoteURL": remote,
            "sources": sources,
        })).collect::<Vec<_>>(),
        "deniedPaths": denied.into_iter().filter(|d| !d.is_empty()).collect::<Vec<_>>(),
    })
}

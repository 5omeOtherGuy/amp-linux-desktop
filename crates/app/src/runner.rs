//! Runs `amp --no-tui` so threads from ampcode.com can run on this computer, like the Mac app's
//! "Use This Mac as a Runner" (MacRunnerController).
//!
//! The runner reports its lifecycle as one JSON object per line (`--output-format json`): hello
//! (CLI version and capabilities), starting, directories, registered, waiting_for_directories,
//! connection_interrupted, reconnected, registration_lost, update_relaunching, shutting_down, and
//! fatal { code, message }. This turns those into the status the settings show, keeps the served
//! directories in step with the settings, and installs the CLI when it is missing.
//!
//! Settings it reads (missing keys use the defaults in `Runner::get`):
//! - runner_home: serve the home directory; otherwise only the directories below (--no-serve-cwd).
//! - runner_dirs: directories added with `amp runner dirs add`, which the CLI also remembers.
//! - runner_discover_dirs: serve every Git checkout found beneath these (--discover-dirs).
//! - runner_new_dirs: where New Directory and New Project create folders (--create-dirs-in).
//! - runner_share_network: let orbs browse through this computer's connection (--share-network).
//! - runner_desktop, runner_terminal, runner_keep_awake.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use amp_common::config::{PATHS, Settings, strings, truthy};
use gtk::{gio, glib};
use serde_json::{Value, json};

use crate::app::App;

const INSTALL_COMMAND: &str = "curl --fail --silent --show-error --location https://ampcode.com/install.sh | bash";
/// Flags an older CLI may not know. When it rejects one, the runner starts again without it.
const OPTIONAL_FLAGS: [&str; 8] = [
    "--share-network",
    "--runner-account-email",
    "--create-dirs-in",
    "--discover-dirs",
    "--no-serve-cwd",
    "--desktop",
    "--remote-control-terminal",
    "--no-remote-control-terminal",
];
const RESTARTS: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(120);

fn fatal_message(code: &str) -> Option<&'static str> {
    Some(match code {
        // The settings offer Sign In… next to these two.
        "not_logged_in" => "The Amp CLI is not signed in on this computer.",
        "credentials_rejected" => "Amp rejected the Amp CLI’s sign-in. Sign in again.",
        "runner_id_in_use" => "Another live runner uses this Runner ID. Rename one so this app’s runner can start.",
        "feature_not_enabled" => "Running threads on your own computer is not enabled for your account.",
        _ => return None,
    })
}

fn status_message(state: &str) -> Option<&'static str> {
    Some(match state {
        "installing" => "Installing the Amp CLI…",
        "starting" => "Starting…",
        "registering" => "Registering with Amp…",
        "waiting_for_directories" => "Add directories to get started.",
        "connection_interrupted" => "Connection to Amp interrupted. Reconnecting…",
        "registration_lost" => "Registering with the server again.",
        "update_relaunching" => "Updating the Amp CLI…",
        _ => return None,
    })
}

pub fn find_cli() -> Option<PathBuf> {
    if let Some(found) = glib::find_program_in_path("amp") {
        return Some(found);
    }
    let home = glib::home_dir();
    [home.join(".amp/bin/amp"), home.join(".local/bin/amp"), "/usr/local/bin/amp".into(), "/usr/bin/amp".into()]
        .into_iter()
        .find(|c| std::fs::metadata(c).is_ok_and(|m| m.is_file()) && is_executable(c))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: access only reads the NUL-terminated path.
    unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
}

/// Puts the child in its own session, away from Amp's terminal and signals.
fn new_session(command: &mut Command) -> &mut Command {
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        })
    }
}

fn append_log(line: &str) {
    let _ = std::fs::create_dir_all(&PATHS.cache_dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&PATHS.runner_log) {
        let _ = writeln!(f, "{line}");
    }
}

fn log(line: &str) {
    append_log(&format!("[amp-native] {line}"));
}

/// The running `amp --no-tui` process. `generation` tells it apart from the ones before it.
struct Process {
    child: Child,
    generation: u64,
    /// Set once it exited; handled when both output streams are read to the end, so the exit
    /// sees a fatal event that came just before it.
    exit_code: Option<i32>,
    open_streams: u8,
}

enum Output {
    Line { generation: u64, stderr: bool, line: String },
    Closed { generation: u64 },
}

#[derive(Default)]
pub struct Runner {
    process: RefCell<Option<Process>>,
    generation: Cell<u64>,
    pub last_error: RefCell<Option<String>>,
    /// off | installing | starting | registering | registered | the CLI's states
    state: RefCell<String>,
    account: RefCell<Option<String>>,
    cli_version: RefCell<Option<String>>,
    capabilities: RefCell<BTreeSet<String>>,
    /// [{ path, repositoryURL, canCreateWorktree }] the runner serves
    directories: RefCell<Vec<Value>>,
    /// Optional flags this CLI rejected.
    unsupported: RefCell<BTreeSet<String>>,
    /// The account signed in to the app, which the runner must use.
    account_email: RefCell<Option<String>>,
    installing: Cell<bool>,
    exits: RefCell<Vec<Instant>>,
    fatal: RefCell<Option<(Option<String>, Option<String>)>>,
    rejected_flag: RefCell<Option<String>>,
    synced: Cell<bool>,
    /// The CLI stopped because it is not signed in, or Amp rejected its sign-in.
    needs_login: Cell<bool>,
    /// `amp login` is running; the code it asks to confirm in the browser, once it printed it.
    signing_in: RefCell<Option<Option<String>>>,
    /// The `amp login` process while it runs, to end it when Amp quits.
    login_pid: Cell<Option<u32>>,
    /// Being removed now; not to be adopted back.
    removing: RefCell<BTreeSet<String>>,
    output: RefCell<Option<async_channel::Sender<Output>>>,
}

impl Runner {
    pub fn get(settings: &Settings, key: &str) -> Value {
        let value = settings.get(key);
        if !value.is_null() {
            return value;
        }
        match key {
            "runner_discover_dirs" | "runner_dirs_removed" => json!([]),
            "runner_new_dirs" => json!(""),
            "runner_share_network" => json!(false),
            _ => Value::Null,
        }
    }

    pub fn runner_id(settings: &Settings) -> String {
        settings.str("runner_name")
    }

    pub fn running(&self) -> bool {
        let mut process = self.process.borrow_mut();
        process.as_mut().is_some_and(|p| p.exit_code.is_none() && matches!(p.child.try_wait(), Ok(None)))
    }

    pub fn status(&self) -> &'static str {
        if self.running() {
            "running"
        } else if *self.state.borrow() == "installing" {
            "installing"
        } else if self.last_error.borrow().is_some() {
            "error"
        } else {
            "off"
        }
    }

    /// Whether the settings offer Sign In…: the CLI needs a login and none is running.
    pub fn needs_login(&self) -> bool {
        self.needs_login.get() && self.signing_in.borrow().is_none()
    }

    /// One line for the settings: what the runner is doing or why it stopped.
    pub fn summary(&self, settings: &Settings) -> String {
        if let Some(code) = self.signing_in.borrow().as_ref() {
            return match code {
                Some(code) => format!("Signing in… Confirm the code {code} in your browser."),
                None => "Signing in…".into(),
            };
        }
        if let Some(error) = self.last_error.borrow().as_ref() {
            return error.clone();
        }
        let state = self.state.borrow().clone();
        let running = self.running();
        if state == "registered" && running {
            let served = self.directories.borrow().len();
            let place = match served {
                0 => "no directories yet".to_string(),
                1 => "1 directory".to_string(),
                n => format!("{n} directories"),
            };
            let account = self.account.borrow().as_ref().map(|a| format!(" as {a}")).unwrap_or_default();
            return format!("Running{account} with Runner ID {}, serving {place}.", Self::runner_id(settings));
        }
        if let Some(message) = status_message(&state)
            && (running || state == "installing" || state == "starting")
        {
            return message.to_string();
        }
        "Off".to_string()
    }

    /// Settings this CLI cannot apply, so the settings can say it needs a newer CLI.
    pub fn missing_features(&self, settings: &Settings) -> Vec<&'static str> {
        let wanted = [
            ("--create-dirs-in", truthy(&Self::get(settings, "runner_new_dirs"))),
            ("--desktop", settings.bool("runner_desktop")),
            ("--discover-dirs", truthy(&Self::get(settings, "runner_discover_dirs"))),
            ("--share-network", truthy(&Self::get(settings, "runner_share_network"))),
        ];
        let unsupported = self.unsupported.borrow();
        wanted.into_iter().filter(|(flag, on)| *on && unsupported.contains(*flag)).map(|(f, _)| f).collect()
    }

    // ---- command ---------------------------------------------------------------------------

    fn command(&self, settings: &Settings) -> Option<Vec<String>> {
        let cli = find_cli()?;
        let mut args = vec![
            cli.to_string_lossy().to_string(),
            "--no-tui".into(),
            "--runner-id".into(),
            Self::runner_id(settings),
            "--output-format".into(),
            "json".into(),
        ];
        let mut optional: Vec<Vec<String>> = Vec::new();
        if let Some(email) = self.account_email.borrow().as_ref() {
            optional.push(vec!["--runner-account-email".into(), email.clone()]);
        }
        if !settings.bool("runner_home") {
            optional.push(vec!["--no-serve-cwd".into()]);
        }
        for directory in strings(&Self::get(settings, "runner_discover_dirs")) {
            optional.push(vec![format!("--discover-dirs={directory}")]);
        }
        let new_dirs = Self::get(settings, "runner_new_dirs");
        if let Some(new_dirs) = new_dirs.as_str().filter(|d| !d.is_empty()) {
            optional.push(vec![format!("--create-dirs-in={new_dirs}")]);
        }
        if truthy(&Self::get(settings, "runner_share_network")) {
            optional.push(vec!["--share-network".into()]);
        }
        if settings.bool("runner_desktop") {
            optional.push(vec!["--desktop".into()]);
        }
        optional.push(vec![
            if settings.bool("runner_terminal") { "--remote-control-terminal" } else { "--no-remote-control-terminal" }
                .into(),
        ]);
        let unsupported = self.unsupported.borrow();
        for flag in optional {
            if !unsupported.contains(flag[0].split('=').next().unwrap_or_default()) {
                args.extend(flag);
            }
        }
        if settings.bool("runner_keep_awake") && glib::find_program_in_path("systemd-inhibit").is_some() {
            let inhibit = ["systemd-inhibit", "--what=sleep:idle", "--who=Amp", "--why=Amp runner is serving threads"];
            args.splice(0..0, inhibit.iter().map(|s| s.to_string()));
        }
        Some(args)
    }

    /// The app's signed-in account, as the page last reported it; kept after its windows close.
    pub fn account_email(&self) -> Option<String> {
        self.account_email.borrow().clone()
    }

    /// The app's signed-in account (app state signedInEmail). The runner serves threads for that
    /// account, as the Mac app's does, and restarts when it changes.
    pub fn account_changed(&self, app: &Rc<App>, email: Option<&str>) {
        let email = email.filter(|e| !e.is_empty()).map(String::from);
        if *self.account_email.borrow() == email {
            return;
        }
        let signed_in = email.is_some();
        *self.account_email.borrow_mut() = email;
        if signed_in && app.settings.bool("runner_enabled") && (self.running() || self.last_error.borrow().is_some()) {
            self.start(app);
        }
    }

    fn working_directory(settings: &Settings) -> PathBuf {
        if settings.bool("runner_home") { glib::home_dir() } else { PATHS.data_dir.join("runner") }
    }

    // ---- lifecycle -------------------------------------------------------------------------

    pub fn start(&self, app: &Rc<App>) {
        self.stop();
        *self.last_error.borrow_mut() = None;
        *self.fatal.borrow_mut() = None;
        *self.rejected_flag.borrow_mut() = None;
        self.synced.set(false);
        self.needs_login.set(false);
        self.removing.borrow_mut().clear();
        self.directories.borrow_mut().clear();
        let Some(args) = self.command(&app.settings) else {
            self.install_cli(app);
            return;
        };
        let cwd = Self::working_directory(&app.settings);
        let _ = std::fs::create_dir_all(&cwd);
        log(&format!("starting {} in {}", args.join(" "), cwd.display()));
        let spawned = new_session(Command::new(&args[0]).args(&args[1..]).current_dir(&cwd))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        match spawned {
            Err(error) => {
                *self.last_error.borrow_mut() = Some(error.to_string());
                *self.state.borrow_mut() = "off".into();
            }
            Ok(mut child) => {
                *self.state.borrow_mut() = "starting".into();
                let generation = self.generation.get() + 1;
                self.generation.set(generation);
                let sender = self.output_channel(app);
                read_lines(child.stdout.take(), generation, false, sender.clone());
                read_lines(child.stderr.take(), generation, true, sender);
                *self.process.borrow_mut() = Some(Process { child, generation, exit_code: None, open_streams: 2 });
                let weak = Rc::downgrade(app);
                glib::timeout_add_local(Duration::from_millis(250), move || {
                    let Some(app) = weak.upgrade() else { return glib::ControlFlow::Break };
                    app.runner.poll_exit(&app, generation)
                });
            }
        }
        app.runner_changed();
    }

    /// The channel the output readers send to, and its handler on the main loop.
    fn output_channel(&self, app: &Rc<App>) -> async_channel::Sender<Output> {
        if let Some(sender) = self.output.borrow().as_ref() {
            return sender.clone();
        }
        let (sender, receiver) = async_channel::unbounded();
        let weak = Rc::downgrade(app);
        glib::spawn_future_local(async move {
            while let Ok(output) = receiver.recv().await {
                let Some(app) = weak.upgrade() else { break };
                app.runner.output(&app, output);
            }
        });
        *self.output.borrow_mut() = Some(sender.clone());
        sender
    }

    pub fn stop(&self) {
        let process = self.process.borrow_mut().take();
        if let Some(mut process) = process
            && process.exit_code.is_none()
            && matches!(process.child.try_wait(), Ok(None))
        {
            // SAFETY: kill only signals the child's pid.
            unsafe { libc::kill(process.child.id() as i32, libc::SIGTERM) };
            let deadline = Instant::now() + Duration::from_secs(5);
            while matches!(process.child.try_wait(), Ok(None)) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if matches!(process.child.try_wait(), Ok(None)) {
                let _ = process.child.kill();
                let _ = process.child.wait();
            }
        }
        if *self.state.borrow() != "installing" {
            *self.state.borrow_mut() = "off".into();
        }
    }

    /// Starts, restarts, or stops the runner to match the settings.
    pub fn apply(&self, app: &Rc<App>) {
        if app.settings.bool("runner_enabled") {
            self.start(app);
        } else {
            self.stop();
            *self.last_error.borrow_mut() = None;
            app.runner_changed();
        }
    }

    fn poll_exit(&self, app: &Rc<App>, generation: u64) -> glib::ControlFlow {
        {
            let mut process = self.process.borrow_mut();
            let Some(process) = process.as_mut().filter(|p| p.generation == generation) else {
                return glib::ControlFlow::Break; // stopped, or replaced by a restart
            };
            if process.exit_code.is_none() {
                match process.child.try_wait() {
                    Ok(None) => return glib::ControlFlow::Continue,
                    Ok(Some(status)) => {
                        process.exit_code = Some(status.code().unwrap_or_else(|| {
                            use std::os::unix::process::ExitStatusExt;
                            -status.signal().unwrap_or(0)
                        }))
                    }
                    Err(_) => process.exit_code = Some(-1),
                }
            }
        }
        self.exit_if_done(app, generation);
        glib::ControlFlow::Break
    }

    fn exit_if_done(&self, app: &Rc<App>, generation: u64) {
        let code = {
            let process = self.process.borrow();
            match process.as_ref() {
                Some(p) if p.generation == generation && p.open_streams == 0 => p.exit_code,
                _ => None,
            }
        };
        if let Some(code) = code {
            *self.process.borrow_mut() = None;
            self.exited(app, code);
        }
    }

    fn exited(&self, app: &Rc<App>, code: i32) {
        log(&format!("runner exited with {code}"));
        if let Some(flag) = self.rejected_flag.borrow_mut().take() {
            log(&format!("the CLI does not know {flag}; starting without it"));
            self.unsupported.borrow_mut().insert(flag);
            let weak = Rc::downgrade(app);
            glib::idle_add_local_once(move || {
                if let Some(app) = weak.upgrade() {
                    app.runner.start(&app);
                }
            });
            return;
        }
        if let Some((code_name, message)) = self.fatal.borrow().clone() {
            self.needs_login.set(matches!(code_name.as_deref(), Some("not_logged_in" | "credentials_rejected")));
            // The CLI's own text for these asks to run amp login; the settings' Sign In… does that.
            let ours = code_name.as_deref().and_then(fatal_message).map(String::from);
            let message = if self.needs_login.get() { ours.clone() } else { None }
                .or(message.filter(|m| !m.is_empty()))
                .or(ours)
                .unwrap_or_else(|| "The runner stopped.".into());
            *self.last_error.borrow_mut() = Some(message);
            *self.state.borrow_mut() = "off".into();
            app.runner_changed();
            return;
        }
        let now = Instant::now();
        let count = {
            let mut exits = self.exits.borrow_mut();
            exits.retain(|t| now.duration_since(*t) < RESTART_WINDOW);
            exits.push(now);
            exits.len()
        };
        if app.settings.bool("runner_enabled") && count <= RESTARTS {
            *self.state.borrow_mut() = "starting".into();
            let weak = Rc::downgrade(app);
            glib::timeout_add_seconds_local_once(2 * count as u32, move || {
                if let Some(app) = weak.upgrade()
                    && app.settings.bool("runner_enabled")
                {
                    app.runner.start(&app);
                }
            });
        } else {
            *self.last_error.borrow_mut() =
                Some(format!("The runner exited repeatedly (exit code {code}). See {}.", PATHS.runner_log.display()));
            *self.state.borrow_mut() = "off".into();
        }
        app.runner_changed();
    }

    // ---- output ----------------------------------------------------------------------------

    fn output(&self, app: &Rc<App>, output: Output) {
        match output {
            Output::Closed { generation } => {
                if let Some(p) = self.process.borrow_mut().as_mut().filter(|p| p.generation == generation) {
                    p.open_streams = p.open_streams.saturating_sub(1);
                }
                self.exit_if_done(app, generation);
            }
            Output::Line { generation, stderr, line } => {
                append_log(&line);
                if generation != self.generation.get() {
                    return;
                }
                if stderr { self.stderr_line(&line) } else { self.stdout_line(app, &line) }
            }
        }
    }

    fn stderr_line(&self, line: &str) {
        const MARKER: &str = "unknown option '";
        if let Some((_, rest)) = line.split_once(MARKER) {
            let flag = rest.split('\'').next().unwrap_or("").split('=').next().unwrap_or("");
            if OPTIONAL_FLAGS.contains(&flag) {
                *self.rejected_flag.borrow_mut() = Some(flag.to_string());
            }
        }
    }

    fn stdout_line(&self, app: &Rc<App>, line: &str) {
        let Ok(Value::Object(event)) = serde_json::from_str::<Value>(line) else { return };
        let text = |key: &str| event.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
        match event.get("type").and_then(Value::as_str) {
            Some("hello") => {
                *self.cli_version.borrow_mut() = text("cliVersion");
                *self.capabilities.borrow_mut() =
                    strings(event.get("capabilities").unwrap_or(&Value::Null)).into_iter().collect();
            }
            Some("starting") => *self.state.borrow_mut() = "registering".into(),
            Some("directories") => {
                *self.directories.borrow_mut() = event
                    .get("directories")
                    .and_then(Value::as_array)
                    .map(|d| d.iter().filter(|d| d.is_object()).cloned().collect())
                    .unwrap_or_default();
                self.sync_dirs(app, !self.synced.get());
                self.synced.set(true);
            }
            Some("registered" | "reconnected") => {
                *self.state.borrow_mut() = "registered".into();
                if let Some(account) = text("accountEmail") {
                    *self.account.borrow_mut() = Some(account);
                }
            }
            Some(
                kind @ ("waiting_for_directories"
                | "connection_interrupted"
                | "registration_lost"
                | "update_relaunching"),
            ) => *self.state.borrow_mut() = kind.into(),
            Some("fatal") => *self.fatal.borrow_mut() = Some((text("code"), text("message"))),
            _ => return,
        }
        app.runner_changed();
    }

    // ---- directories -----------------------------------------------------------------------

    /// Brings the CLI's remembered directories and runner_dirs together: adds settings directories
    /// the CLI lacks, removes ones taken out of the settings while the runner was off, and adopts
    /// ones added from a terminal with `amp runner dirs add`. Adds and removes only on the first
    /// report after starting; adopts on every report.
    fn sync_dirs(&self, app: &Rc<App>, first: bool) {
        let settings = &app.settings;
        let mut wanted: Vec<String> = Vec::new();
        for d in settings.strings("runner_dirs") {
            if !wanted.contains(&d) {
                wanted.push(d);
            }
        }
        let mut removed: BTreeSet<String> = strings(&Self::get(settings, "runner_dirs_removed")).into_iter().collect();
        removed.extend(self.removing.borrow().iter().cloned());
        let served: Vec<String> = self
            .directories
            .borrow()
            .iter()
            .filter_map(|d| d.get("path").and_then(Value::as_str).filter(|p| !p.is_empty()).map(String::from))
            .collect();
        let mut roots: Vec<String> = strings(&Self::get(settings, "runner_discover_dirs"))
            .iter()
            .map(|d| Path::new(d).components().collect::<PathBuf>().to_string_lossy().to_string())
            .collect();
        roots.push(Self::working_directory(settings).to_string_lossy().to_string());
        let found = |p: &str| roots.iter().any(|r| p == r || p.starts_with(&format!("{}/", r.trim_end_matches('/'))));
        let missing: Vec<String> = wanted.iter().filter(|d| !served.contains(d)).cloned().collect();
        let extra: Vec<String> = served.iter().filter(|p| !wanted.contains(p) && !found(p)).cloned().collect();
        if first {
            if !missing.is_empty() {
                self.dirs_command(app, "add", &missing);
            }
            if !extra.is_empty() {
                let removing: Vec<String> = extra.iter().filter(|p| removed.contains(*p)).cloned().collect();
                *self.removing.borrow_mut() = removing.iter().cloned().collect();
                self.dirs_command(app, "remove", &removing);
            }
            settings.set("runner_dirs_removed", json!([]));
        }
        let adopted: Vec<String> = extra.into_iter().filter(|p| !removed.contains(p)).collect();
        if !adopted.is_empty() {
            wanted.extend(adopted);
            settings.set("runner_dirs", json!(wanted));
            app.refresh_settings_runner();
        }
    }

    pub fn dirs_command(&self, app: &Rc<App>, action: &str, directories: &[String]) {
        if directories.is_empty() {
            return;
        }
        let settings = &app.settings;
        if !self.running() {
            // The CLI remembers added directories; forget removed ones the next time it runs.
            let mut removed: BTreeSet<String> =
                strings(&Self::get(settings, "runner_dirs_removed")).into_iter().collect();
            for d in directories {
                if action == "remove" {
                    removed.insert(d.clone());
                } else {
                    removed.remove(d);
                }
            }
            settings.set("runner_dirs_removed", json!(removed));
            return;
        }
        let Some(cli) = find_cli() else { return };
        let batches: Vec<Vec<String>> = if self.capabilities.borrow().contains(&format!("dirs-{action}-many")) {
            vec![directories.to_vec()]
        } else {
            directories.iter().map(|d| vec![d.clone()]).collect()
        };
        let runner_id = Self::runner_id(settings);
        for batch in batches {
            let child = Command::new(&cli)
                .args(["runner", "dirs", action, "--runner-id", &runner_id])
                .args(&batch)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(mut child) = child {
                std::thread::spawn(move || child.wait());
            }
        }
    }

    // ---- CLI sign-in -----------------------------------------------------------------------

    /// Signs the CLI in, as the Mac's Sign In… does: runs `amp login`, which opens its page in the
    /// browser itself, shows the code that page asks to confirm, and starts the runner again once
    /// the login succeeded.
    pub fn sign_in(&self, app: &Rc<App>) {
        if self.signing_in.borrow().is_some() {
            return;
        }
        let Some(cli) = find_cli() else {
            self.install_cli(app);
            return;
        };
        log("running amp login");
        let spawned = new_session(Command::new(cli).arg("login"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                *self.last_error.borrow_mut() = Some(format!("amp login could not be started: {error}"));
                app.runner_changed();
                return;
            }
        };
        *self.signing_in.borrow_mut() = Some(None);
        self.login_pid.set(Some(child.id()));
        app.runner_changed();
        let (sender, receiver) = async_channel::unbounded::<String>();
        for stream in [
            child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
            child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        ] {
            let sender = sender.clone();
            std::thread::spawn(move || {
                for line in stream.map(BufReader::new).into_iter().flat_map(|r| r.lines().map_while(Result::ok)) {
                    append_log(&line);
                    let _ = sender.send_blocking(line);
                }
            });
        }
        drop(sender);
        let weak = Rc::downgrade(app);
        glib::spawn_future_local(async move {
            while let Ok(line) = receiver.recv().await {
                let Some(app) = weak.upgrade() else { return };
                let code = line
                    .split_whitespace()
                    .filter_map(|word| url::Url::parse(word).ok())
                    .find_map(|u| u.query_pairs().find(|(k, _)| k == "user_code").map(|(_, v)| v.into_owned()));
                if code.is_some() && app.runner.signing_in.borrow().as_ref().is_some_and(Option::is_none) {
                    *app.runner.signing_in.borrow_mut() = Some(code);
                    app.runner_changed();
                }
            }
            let code =
                gio::spawn_blocking(move || child.wait().ok().and_then(|s| s.code()).unwrap_or(-1)).await.unwrap_or(-1);
            let Some(app) = weak.upgrade() else { return };
            app.runner.signed_in(&app, code);
        });
    }

    /// Ends a sign-in still waiting for the browser, as Amp quits.
    pub fn cancel_sign_in(&self) {
        if let Some(pid) = self.login_pid.take() {
            // SAFETY: kill only signals the login's pid, which has not been reaped yet.
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        }
    }

    fn signed_in(&self, app: &Rc<App>, code: i32) {
        log(&format!("amp login exited with {code}"));
        self.signing_in.borrow_mut().take();
        self.login_pid.set(None);
        if code == 0 {
            self.needs_login.set(false);
            if app.settings.bool("runner_enabled") {
                self.start(app);
                return;
            }
            *self.last_error.borrow_mut() = None;
        } else {
            *self.last_error.borrow_mut() = Some("The Amp CLI did not finish signing in. Try again.".into());
        }
        app.runner_changed();
    }

    // ---- CLI install -----------------------------------------------------------------------

    /// Installs the Amp CLI the way the Mac app does, then starts the runner.
    pub fn install_cli(&self, app: &Rc<App>) {
        if self.installing.get() {
            return;
        }
        if glib::find_program_in_path("curl").is_none() || glib::find_program_in_path("bash").is_none() {
            *self.last_error.borrow_mut() = Some(
                "The Amp CLI is not installed, and installing it needs curl. Install it from ampcode.com/install."
                    .into(),
            );
            app.runner_changed();
            return;
        }
        *self.state.borrow_mut() = "installing".into();
        *self.last_error.borrow_mut() = None;
        log("running the Amp CLI installer");
        let _ = std::fs::create_dir_all(&PATHS.cache_dir);
        let log_file = std::fs::OpenOptions::new().create(true).append(true).open(&PATHS.runner_log);
        let spawned = log_file.and_then(|out| {
            let err = out.try_clone()?;
            new_session(Command::new("bash").args(["-c", INSTALL_COMMAND]))
                .stdin(Stdio::null())
                .stdout(out)
                .stderr(err)
                .spawn()
        });
        match spawned {
            Err(error) => {
                *self.state.borrow_mut() = "off".into();
                *self.last_error.borrow_mut() = Some(format!("The Amp CLI installer could not be started: {error}"));
            }
            Ok(mut child) => {
                self.installing.set(true);
                let weak = Rc::downgrade(app);
                glib::timeout_add_local(Duration::from_millis(250), move || match child.try_wait() {
                    Ok(None) => glib::ControlFlow::Continue,
                    result => {
                        let code = result.ok().flatten().and_then(|s| s.code()).unwrap_or(-1);
                        if let Some(app) = weak.upgrade() {
                            app.runner.installed(&app, code);
                        }
                        glib::ControlFlow::Break
                    }
                });
            }
        }
        app.runner_changed();
    }

    fn installed(&self, app: &Rc<App>, code: i32) {
        self.installing.set(false);
        log(&format!("Amp CLI installer exited with {code}"));
        *self.state.borrow_mut() = "off".into();
        if code != 0 || find_cli().is_none() {
            *self.last_error.borrow_mut() = Some(format!(
                "The Amp CLI installer exited with code {code}. Check {}, or install the CLI from ampcode.com/install, then retry.",
                PATHS.runner_log.display()
            ));
            app.runner_changed();
        } else if app.settings.bool("runner_enabled") {
            self.start(app);
        } else {
            app.runner_changed();
        }
    }
}

/// Reads `stream` line by line on its own thread and sends the lines to the main loop.
fn read_lines(
    stream: Option<impl Read + Send + 'static>,
    generation: u64,
    stderr: bool,
    sender: async_channel::Sender<Output>,
) {
    let Some(stream) = stream else {
        let _ = sender.send_blocking(Output::Closed { generation });
        return;
    };
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if buffer.last() == Some(&b'\n') {
                        buffer.pop();
                    }
                    let line = String::from_utf8_lossy(&buffer).into_owned();
                    let _ = sender.send_blocking(Output::Line { generation, stderr, line });
                }
            }
        }
        let _ = sender.send_blocking(Output::Closed { generation });
    });
}

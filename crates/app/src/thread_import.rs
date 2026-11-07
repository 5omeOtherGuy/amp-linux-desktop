//! Settings → General → Other Coding Agents → Import Threads…: brings conversations from other
//! coding agents on this computer into Amp as read-only threads, as the Mac app's
//! MacThreadImportController does, through the Amp CLI's `amp threads import`.
//!
//! `amp threads import detect --json` lists the agents and where their conversations are;
//! `amp threads import <agent> --all --json` imports and reports one JSON object per line:
//! listed { total }, started, uploading, imported, and finished { imported, skipped, failed }.
//! Imports use the CLI's login; `--expect-user-email` makes the CLI refuse when it is signed in
//! to another account than the app. They keep running when the dialog closes.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gio, glib};
use serde_json::{Value, json};

use crate::app::App;
use crate::runner::find_cli;

/// "Imports bring over conversations from the last 30 days."
const MAX_AGE_DAYS: &str = "30";

#[derive(Clone)]
struct Agent {
    /// The provider ID, such as claude-code.
    id: String,
    name: String,
    subcommand: String,
    location: String,
    available: bool,
}

/// An import in progress: how many conversations it found and how many it brought over so far.
#[derive(Default, Clone)]
struct Progress {
    total: Option<u64>,
    imported: u64,
}

#[derive(Default)]
pub struct ThreadImports {
    agents: RefCell<Vec<Agent>>,
    /// Why the agents could not be listed.
    detect_error: RefCell<Option<String>>,
    detecting: Cell<bool>,
    running: RefCell<HashMap<String, Progress>>,
    errors: RefCell<HashMap<String, String>>,
    /// Imports waiting for the one before them (Import All), with whether to import again.
    queue: RefCell<VecDeque<(String, bool)>>,
    dialog: RefCell<Option<Rc<Dialog>>>,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// "5 sec ago", "3 min ago", as the Mac's relative dates.
fn ago(at: u64) -> String {
    let seconds = now().saturating_sub(at);
    match seconds {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86400 => format!("{} hr ago", seconds / 3600),
        _ => match seconds / 86400 {
            1 => "1 day ago".into(),
            days => format!("{days} days ago"),
        },
    }
}

impl ThreadImports {
    /// The CLI that imports: the one chosen with Locate…, else the one the runner uses.
    fn cli(app: &App) -> Option<PathBuf> {
        let chosen = app.settings.str("thread_import_cli");
        if !chosen.is_empty() {
            return Some(PathBuf::from(chosen));
        }
        find_cli()
    }

    fn records(app: &App) -> serde_json::Map<String, Value> {
        app.settings.get("thread_imports").as_object().cloned().unwrap_or_default()
    }

    /// The General tab's line: when the last import finished.
    pub fn summary(app: &App) -> String {
        let last = Self::records(app).values().filter_map(|r| r.get("at").and_then(Value::as_u64)).max();
        match last {
            Some(at) => format!("Last import {}", ago(at)),
            None => "Nothing imported yet.".into(),
        }
    }

    /// Lists the agents again, off the main thread.
    fn detect(&self, app: &Rc<App>) {
        if self.detecting.replace(true) {
            return;
        }
        let cli = Self::cli(app);
        let weak = Rc::downgrade(app);
        glib::spawn_future_local(async move {
            let result =
                gio::spawn_blocking(move || detect(cli)).await.unwrap_or_else(|_| Err("Detection failed.".into()));
            let Some(app) = weak.upgrade() else { return };
            let imports = &app.thread_imports;
            imports.detecting.set(false);
            match result {
                Ok(agents) => {
                    *imports.agents.borrow_mut() = agents;
                    *imports.detect_error.borrow_mut() = None;
                }
                Err(error) => {
                    imports.agents.borrow_mut().clear();
                    *imports.detect_error.borrow_mut() = Some(error);
                }
            }
            imports.changed(&app);
        });
    }

    fn changed(&self, app: &Rc<App>) {
        let dialog = self.dialog.borrow().clone();
        if let Some(dialog) = dialog {
            dialog.refresh();
        }
        app.refresh_settings_imports();
    }

    /// Starts importing `id`'s conversations, or queues it behind a running import.
    fn import(&self, app: &Rc<App>, id: &str, again: bool) {
        if self.running.borrow().contains_key(id) || self.queue.borrow().iter().any(|(q, _)| q == id) {
            return;
        }
        if !self.running.borrow().is_empty() {
            self.queue.borrow_mut().push_back((id.to_string(), again));
            self.changed(app);
            return;
        }
        let agent = self.agents.borrow().iter().find(|a| a.id == id).cloned();
        let Some(agent) = agent else { return };
        let Some(cli) = Self::cli(app) else {
            self.errors.borrow_mut().insert(agent.id.clone(), "The Amp CLI was not found.".into());
            self.changed(app);
            return;
        };
        let mut args: Vec<String> =
            ["threads", "import", &agent.subcommand, "--all", "--max-age", MAX_AGE_DAYS, "--json"]
                .map(String::from)
                .into();
        // Imports go only to the account signed in to the app; the CLI refuses another one.
        let Some(email) = app.runner.account_email() else {
            self.errors.borrow_mut().insert(agent.id.clone(), "Open Amp and sign in before importing.".into());
            self.changed(app);
            return;
        };
        args.extend(["--expect-user-email".into(), email]);
        if again {
            args.push("--reimport".into());
        }
        let child =
            Command::new(&cli).args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                self.errors.borrow_mut().insert(agent.id.clone(), format!("The Amp CLI could not be started: {error}"));
                self.changed(app);
                return;
            }
        };
        log::info!("started import for {}", agent.id);
        self.errors.borrow_mut().remove(&agent.id);
        self.running.borrow_mut().insert(agent.id.clone(), Progress::default());
        self.changed(app);
        let (sender, receiver) = async_channel::unbounded::<Value>();
        let stdout = child.stdout.take();
        std::thread::spawn(move || {
            for line in stdout.map(BufReader::new).into_iter().flat_map(|r| r.lines().map_while(Result::ok)) {
                if let Ok(event) = serde_json::from_str::<Value>(&line) {
                    let _ = sender.send_blocking(event);
                }
            }
        });
        let stderr = child.stderr.take();
        let errors = std::thread::spawn(move || {
            let lines: Vec<String> =
                stderr.map(BufReader::new).into_iter().flat_map(|r| r.lines().map_while(Result::ok)).collect();
            let status = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
            (status, lines.into_iter().rev().find(|l| !l.trim().is_empty()))
        });
        let weak = Rc::downgrade(app);
        let id = agent.id.clone();
        glib::spawn_future_local(async move {
            let mut finished: Option<Value> = None;
            while let Ok(event) = receiver.recv().await {
                let Some(app) = weak.upgrade() else { return };
                let imports = &app.thread_imports;
                match event.get("type").and_then(Value::as_str) {
                    Some("listed") => {
                        if let Some(p) = imports.running.borrow_mut().get_mut(&id) {
                            p.total = event.get("total").and_then(Value::as_u64);
                        }
                    }
                    Some("imported") => {
                        if let Some(p) = imports.running.borrow_mut().get_mut(&id) {
                            p.imported += 1;
                        }
                    }
                    Some("finished") => finished = Some(event),
                    _ => continue,
                }
                imports.changed(&app);
            }
            let (status, last_error) =
                gio::spawn_blocking(move || errors.join().unwrap_or((-1, None))).await.unwrap_or((-1, None));
            let Some(app) = weak.upgrade() else { return };
            app.thread_imports.ended(&app, &id, finished, status, last_error);
        });
    }

    fn ended(&self, app: &Rc<App>, id: &str, finished: Option<Value>, status: i32, last_error: Option<String>) {
        self.running.borrow_mut().remove(id);
        let count = |key: &str| finished.as_ref().and_then(|f| f.get(key)).and_then(Value::as_u64).unwrap_or(0);
        log::info!(
            "import for {id} ended: imported {} skipped {} failed {} status {status}",
            count("imported"),
            count("skipped"),
            count("failed")
        );
        if finished.is_some() && status == 0 {
            let mut records = Self::records(app);
            records
                .insert(id.to_string(), json!({"at": now(), "imported": count("imported"), "failed": count("failed")}));
            app.settings.set("thread_imports", Value::Object(records));
            if count("failed") > 0 {
                self.errors
                    .borrow_mut()
                    .insert(id.to_string(), format!("{} conversations could not be imported.", count("failed")));
            }
        } else {
            let error = last_error
                .map(|e| e.trim_start_matches("Error: ").to_string())
                .unwrap_or_else(|| format!("The import stopped (exit code {status})."));
            self.errors.borrow_mut().insert(id.to_string(), error);
        }
        // An import that cannot start (no account, no CLI) records its error and returns, so move on
        // until one runs or the queue is empty.
        while self.running.borrow().is_empty() {
            let Some((next, again)) = self.queue.borrow_mut().pop_front() else { break };
            self.import(app, &next, again);
        }
        self.changed(app);
    }

    /// One agent's line in the dialog.
    fn status(&self, app: &App, agent: &Agent) -> (String, bool) {
        if let Some(progress) = self.running.borrow().get(&agent.id) {
            return match progress.total {
                Some(total) => (format!("Importing… {} of {total}", progress.imported), false),
                None => ("Importing…".into(), false),
            };
        }
        if self.queue.borrow().iter().any(|(q, _)| *q == agent.id) {
            return ("Waiting to import…".into(), false);
        }
        if let Some(error) = self.errors.borrow().get(&agent.id) {
            return (error.clone(), true);
        }
        if !agent.available {
            return (format!("Not found at {}", agent.location), false);
        }
        let record = Self::records(app).get(&agent.id).cloned();
        match record {
            Some(r) => {
                let imported = r.get("imported").and_then(Value::as_u64).unwrap_or(0);
                let at = r.get("at").and_then(Value::as_u64).unwrap_or(0);
                (format!("{imported} imported · {}", ago(at)), false)
            }
            None => (agent.location.clone(), false),
        }
    }
}

fn detect(cli: Option<PathBuf>) -> Result<Vec<Agent>, String> {
    let cli = cli.ok_or("The Amp CLI is not installed. Install it from ampcode.com/install, or locate it.")?;
    let output = Command::new(&cli)
        .args(["threads", "import", "detect", "--json"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{} could not be started: {e}", cli.display()))?;
    let parsed: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "This Amp CLI cannot import threads. Update it in Settings → Updates.".to_string())?;
    let text = |p: &Value, key: &str| p.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    Ok(parsed
        .get("providers")
        .and_then(Value::as_array)
        .map(|providers| {
            providers
                .iter()
                .map(|p| Agent {
                    id: text(p, "provider"),
                    name: text(p, "agentName"),
                    subcommand: text(p, "subcommand"),
                    location: text(p, "location"),
                    available: p.get("available").and_then(Value::as_bool).unwrap_or(false),
                })
                .filter(|a| !a.id.is_empty() && !a.subcommand.is_empty())
                .collect()
        })
        .unwrap_or_default())
}

/// A small dimmed note, such as the ones under the Mac's settings groups.
pub fn caption(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).wrap(true).xalign(0.0).margin_top(8).build();
    label.add_css_class("dim-label");
    label.add_css_class("caption");
    label
}

// ---- dialog ------------------------------------------------------------------------------------

/// An agent's row, or the row that says why none could be listed (without buttons).
struct AgentRow {
    /// What the row shows: the agent and whether it was found, or the error.
    key: String,
    row: adw::ActionRow,
    buttons: Option<gtk::Box>,
}

struct Dialog {
    app: Weak<App>,
    window: adw::Window,
    cli_row: adw::ActionRow,
    agents_group: adw::PreferencesGroup,
    agent_rows: RefCell<Vec<AgentRow>>,
    import_all: gtk::Button,
    #[allow(deprecated)]
    chooser: RefCell<Option<gtk::FileChooserNative>>,
}

/// Opens the Import Threads dialog over `parent`.
pub fn show(app: &Rc<App>, parent: &impl IsA<gtk::Window>) {
    let existing = app.thread_imports.dialog.borrow().clone();
    if let Some(dialog) = existing {
        dialog.window.present();
        return;
    }
    let window = adw::Window::builder()
        .application(&app.adw)
        .title("Import Threads")
        .transient_for(parent)
        .default_width(640)
        .default_height(720)
        .build();
    let dialog = Rc::new(Dialog {
        app: Rc::downgrade(app),
        window,
        cli_row: adw::ActionRow::builder().title("Amp CLI").build(),
        agents_group: adw::PreferencesGroup::builder().title("Coding Agents").build(),
        agent_rows: RefCell::default(),
        import_all: gtk::Button::builder().label("Import All").valign(gtk::Align::Center).build(),
        chooser: RefCell::default(),
    });
    dialog.build();
    *app.thread_imports.dialog.borrow_mut() = Some(dialog.clone());
    let weak = Rc::downgrade(app);
    dialog.window.connect_close_request(move |_| {
        if let Some(app) = weak.upgrade() {
            app.thread_imports.dialog.borrow_mut().take();
        }
        glib::Propagation::Proceed
    });
    // The list follows changes made while the dialog was in the background, as the Mac's does.
    let weak = Rc::downgrade(app);
    dialog.window.connect_is_active_notify(move |window| {
        if window.is_active()
            && let Some(app) = weak.upgrade()
        {
            app.thread_imports.detect(&app);
        }
    });
    dialog.refresh();
    dialog.window.present();
    app.thread_imports.detect(app);
}

impl Dialog {
    fn build(self: &Rc<Self>) {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&adw::HeaderBar::new());
        let page = adw::PreferencesPage::new();
        page.set_vexpand(true);

        let cli = adw::PreferencesGroup::new();
        let locate = gtk::Button::builder().label("Locate…").valign(gtk::Align::Center).build();
        let weak = Rc::downgrade(self);
        locate.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.locate();
            }
        });
        let again = gtk::Button::builder().label("Check Again").valign(gtk::Align::Center).build();
        let weak = Rc::downgrade(self);
        again.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade().and_then(|t| t.app.upgrade()) {
                app.thread_imports.detect(&app);
            }
        });
        self.cli_row.add_suffix(&locate);
        self.cli_row.add_suffix(&again);
        cli.add(&self.cli_row);
        cli.add(&caption(
            "Imports use the Amp CLI’s login (amp login). Amp checks it matches the account you’re signed in with here.",
        ));
        page.add(&cli);
        page.add(&self.agents_group);

        let footer = adw::PreferencesGroup::new();
        let line = gtk::Box::builder().spacing(12).build();
        let note = caption(
            "Imports bring over conversations from the last 30 days, run in the background, and skip \
             conversations already in Amp, so you can run them again any time. Imported threads are read only; \
             use Continue in Amp to keep going.",
        );
        note.set_hexpand(true);
        line.append(&note);
        let weak = Rc::downgrade(self);
        self.import_all.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade().and_then(|t| t.app.upgrade()) {
                let ids: Vec<String> =
                    app.thread_imports.agents.borrow().iter().filter(|a| a.available).map(|a| a.id.clone()).collect();
                for id in ids {
                    app.thread_imports.import(&app, &id, false);
                }
            }
        });
        line.append(&self.import_all);
        footer.add(&line);
        page.add(&footer);
        content.append(&page);

        let buttons =
            gtk::Box::builder().halign(gtk::Align::End).margin_top(10).margin_bottom(12).margin_end(12).build();
        let done = gtk::Button::with_label("Done");
        done.add_css_class("suggested-action");
        let window = self.window.downgrade();
        done.connect_clicked(move |_| {
            if let Some(window) = window.upgrade() {
                window.close();
            }
        });
        buttons.append(&done);
        content.append(&buttons);
        self.window.set_content(Some(&content));
    }

    /// Updates the rows in place, so an open Re-import menu stays open; rebuilds them only when
    /// the agents found change.
    fn refresh(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        let imports = &app.thread_imports;
        let cli = ThreadImports::cli(&app).map(|p| p.display().to_string()).unwrap_or_else(|| "Not found".into());
        self.cli_row.set_subtitle(&glib::markup_escape_text(&cli));
        let agents = imports.agents.borrow().clone();
        let error = imports.detect_error.borrow().clone();
        let mut keys: Vec<String> = agents.iter().map(|a| format!("{}:{}", a.id, a.available)).collect();
        keys.extend(error.iter().map(|e| format!("error:{e}")));
        if keys != self.agent_rows.borrow().iter().map(|r| r.key.clone()).collect::<Vec<_>>() {
            self.rebuild(&agents, error.as_deref(), keys);
        }
        for (row, agent) in self.agent_rows.borrow().iter().filter(|r| r.buttons.is_some()).zip(&agents) {
            let (status, failed) = imports.status(&app, agent);
            row.row.set_subtitle(&glib::markup_escape_text(&status));
            if failed {
                row.row.add_css_class("amp-import-error");
            } else {
                row.row.remove_css_class("amp-import-error");
            }
            let busy = imports.running.borrow().contains_key(&agent.id);
            if let Some(buttons) = &row.buttons {
                buttons.set_sensitive(agent.available && !busy);
            }
        }
        self.import_all.set_sensitive(agents.iter().any(|a| a.available) && imports.running.borrow().is_empty());
    }

    fn rebuild(self: &Rc<Self>, agents: &[Agent], error: Option<&str>, keys: Vec<String>) {
        for row in self.agent_rows.borrow_mut().drain(..) {
            self.agents_group.remove(&row.row);
        }
        let mut rows = Vec::new();
        for agent in agents {
            let row = adw::ActionRow::builder().title(glib::markup_escape_text(&agent.name)).build();
            let buttons = gtk::Box::builder().valign(gtk::Align::Center).build();
            buttons.add_css_class("linked");
            let import = gtk::Button::with_label("Import");
            let id = agent.id.clone();
            let weak = self.app.clone();
            import.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.thread_imports.import(&app, &id, false);
                }
            });
            let actions = gio::SimpleActionGroup::new();
            let reimport = gio::SimpleAction::new("reimport", None);
            let id = agent.id.clone();
            let weak = self.app.clone();
            reimport.connect_activate(move |_, _| {
                if let Some(app) = weak.upgrade() {
                    app.thread_imports.import(&app, &id, true);
                }
            });
            actions.add_action(&reimport);
            let menu = gio::Menu::new();
            menu.append(Some("Re-import All Conversations"), Some("agent.reimport"));
            let more = gtk::MenuButton::builder().menu_model(&menu).build();
            more.insert_action_group("agent", Some(&actions));
            buttons.append(&import);
            buttons.append(&more);
            row.add_suffix(&buttons);
            rows.push(AgentRow { key: String::new(), row, buttons: Some(buttons) });
        }
        if let Some(error) = error {
            let row = adw::ActionRow::builder()
                .title("No Coding Agents Listed")
                .subtitle(glib::markup_escape_text(error))
                .build();
            row.add_css_class("amp-import-error");
            rows.push(AgentRow { key: String::new(), row, buttons: None });
        }
        for (row, key) in rows.iter_mut().zip(keys) {
            row.key = key;
            self.agents_group.add(&row.row);
        }
        *self.agent_rows.borrow_mut() = rows;
    }

    #[allow(deprecated)] // gtk::FileDialog needs GTK 4.10 features the rest of the app avoids.
    fn locate(self: &Rc<Self>) {
        let chooser = gtk::FileChooserNative::builder()
            .title("Locate the Amp CLI")
            .transient_for(&self.window)
            .modal(true)
            .action(gtk::FileChooserAction::Open)
            .accept_label("Choose")
            .build();
        let weak = Rc::downgrade(self);
        chooser.connect_response(move |dialog, response| {
            let path = (response == gtk::ResponseType::Accept).then(|| dialog.file().and_then(|f| f.path())).flatten();
            dialog.destroy();
            let Some(this) = weak.upgrade() else { return };
            this.chooser.borrow_mut().take();
            if let (Some(path), Some(app)) = (path, this.app.upgrade()) {
                app.settings.set("thread_import_cli", json!(path.to_string_lossy()));
                app.thread_imports.detect(&app);
            }
        });
        chooser.show();
        *self.chooser.borrow_mut() = Some(chooser);
    }
}

//! The page's native bridge: the `window.webkit.messageHandlers.<name>` the macOS app answers,
//! and the window's menu actions.
//!
//! Payload and reply shapes follow the page's bridge module on ampcode.com (the `ampNative*`
//! callers in /_app/immutable/chunks).

use std::rc::Rc;

use adw::prelude::*;
use amp_common::config::{self, PATHS};
use amp_common::scripts;
use gtk::{gdk, glib};
use serde_json::{Value, json};
use webkit::javascriptcore;
use webkit::prelude::*;

use crate::app::App;
use crate::confirmation::ConfirmationSheet;
use crate::files;
use crate::local_projects;
use crate::menus::Menus;
use crate::toasts::ToastCenter;
use crate::window::AmpWindow;

const PUCK_PATH: &str = "/puck-window";

/// Handlers the page calls without awaiting an answer.
const HANDLERS: &[&str] = &[
    "ampNativeWindowDrag",
    "ampNativeOpenExternalURL",
    "ampNativeOpenWindow",
    "ampNativeOpenThreadInMainWindow",
    "ampNativeAppSettings",
    "ampNativeEnableNotifications",
    "ampNativeClipboard",
    "ampNativeAppState",
    "ampNavigationState",
    "ampNativeConfirmationSheet",
    "ampNativeMenuBar",
    "ampNativeSaveFile",
    "ampNativeToast",
    "ampNativeOpenPuckWindow",
    "ampNativeTogglePuckWindow",
];

/// Handlers whose answer the page awaits.
const REPLY_HANDLERS: &[&str] = &[
    "ampNativeClipboardWrite",
    "ampNativeLocalNotification",
    "ampNativeGlobalShortcuts",
    "ampNativeClearCache",
    "ampNativeDiagnostics",
    "ampNativeUnifiedLog",
    "ampNativeOpenInEditor",
    "ampNativeShowInFinder",
    "ampNativeSaveFileStream",
    "ampNativePasteboardImages",
    "ampNativeImportLocalProjects",
];

/// The answer to a message the page awaits. Dropping it unanswered rejects it, so the page
/// never waits forever.
pub struct Reply(Option<(webkit::ScriptMessageReply, Option<javascriptcore::Context>)>);

impl Reply {
    pub fn send(mut self, result: Result<Value, String>) {
        let Some((reply, context)) = self.0.take() else { return };
        match (result, context) {
            (Ok(value), Some(context)) => {
                reply.return_value(&javascriptcore::Value::from_json(&context, &value.to_string()))
            }
            (Ok(_), None) => reply.return_error_message("no script context"),
            (Err(message), _) => reply.return_error_message(&message),
        }
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if let Some((reply, _)) = self.0.take() {
            reply.return_error_message("no reply");
        }
    }
}

fn body_of(value: &javascriptcore::Value) -> Value {
    value.to_json(0).and_then(|j| serde_json::from_str(&j).ok()).unwrap_or(Value::Null)
}

/// The body as the Python app logged it: strings as they are, the rest as JSON, cut to 300
/// characters.
fn loggable(body: &Value) -> String {
    let text = match body {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    text.chars().take(300).collect()
}

pub fn register_handlers(app: &Rc<App>, window: &Rc<AmpWindow>, manager: &webkit::UserContentManager) {
    let mut names: Vec<&'static str> = HANDLERS.to_vec();
    if app.dictation_available() {
        names.push("ampNativeDictation");
    }
    if cfg!(feature = "huddles") {
        names.push("ampNativeHuddle");
    }
    for name in names {
        manager.register_script_message_handler(name, None);
        let weak = Rc::downgrade(window);
        manager.connect_script_message_received(Some(name), move |_, value| {
            let Some(window) = weak.upgrade() else { return };
            let body = body_of(value);
            // A Huddle message carries a call token; keep it out of the log.
            log::debug!("message {name} {}", if name == "ampNativeHuddle" { "{…}".into() } else { loggable(&body) });
            on_message(&window, name, &body);
        });
    }
    for &name in REPLY_HANDLERS {
        manager.register_script_message_handler_with_reply(name, None);
        let weak = Rc::downgrade(window);
        manager.connect_script_message_with_reply_received(Some(name), move |_, value, reply| {
            let reply = Reply(Some((reply.clone(), value.context())));
            let Some(window) = weak.upgrade() else { return true };
            let body = body_of(value);
            log::debug!("message {name} {}", loggable(&body));
            on_reply(&window, name, &body, reply);
            true
        });
    }
}

fn on_message(window: &Rc<AmpWindow>, name: &str, body: &Value) {
    let Some(app) = window.app.upgrade() else { return };
    match name {
        "ampNativeWindowDrag" => {
            if body == "doubleClick" {
                window.toggle_maximized();
            } else if body.get("action").and_then(Value::as_str) == Some("drag") {
                window.begin_move();
            }
        }
        "ampNativeOpenExternalURL" => {
            if let Some(url) = body.as_str() {
                app.open_in_browser(url);
            }
        }
        "ampNativeOpenWindow" => {
            if let Some(path) = body.as_str() {
                app.open_window(path, None);
            }
        }
        "ampNativeOpenThreadInMainWindow" => {
            let path = body.as_str().or_else(|| {
                ["url", "path"].iter().find_map(|k| body.get(*k).and_then(Value::as_str).filter(|p| !p.is_empty()))
            });
            if let Some(path) = path.filter(|p| !p.is_empty())
                && let Some(main) = app.main_window()
            {
                main.navigate(path);
                main.window.present();
            }
        }
        "ampNativeAppSettings" => app.show_app_settings(body.get("tab").and_then(Value::as_str)),
        "ampNativeEnableNotifications" => app.enable_notifications(),
        "ampNativeClipboard" => write_clipboard(window, body),
        "ampNativeAppState" => {
            if body.is_object() && body.get("version") == Some(&json!(1)) {
                *window.app_state.borrow_mut() = body.clone();
                app.app_state_changed(window);
            }
        }
        "ampNavigationState" => {
            if let Some(url) = body.as_str() {
                *window.current_url.borrow_mut() = Some(url.to_string());
                app.update_actions();
            }
        }
        "ampNativeConfirmationSheet" => confirmation_sheet(window, body),
        "ampNativeMenuBar" => {
            if body.is_object() {
                app.menu_bar_changed(body);
            }
        }
        "ampNativeSaveFile" => {
            if let Err(error) = files::save_file(&app.adw, body) {
                log::warn!("could not save file: {error}");
                let name = body.get("fileName").and_then(Value::as_str).filter(|n| !n.is_empty());
                window.show_toast(&format!("Could not save {}", name.unwrap_or("the file")));
            }
        }
        "ampNativeToast" => {
            let center = window.toast_center.get_or_init(|| {
                let weak = Rc::downgrade(window);
                ToastCenter::new(window.toasts.clone(), move |js| {
                    if let Some(window) = weak.upgrade() {
                        window.run_js(&js);
                    }
                })
            });
            center.handle(body);
        }
        "ampNativeDictation" => {
            let dictation = window.dictation.get_or_init(|| {
                let weak = Rc::downgrade(window);
                let dictation = amp_dictation::Dictation::new(move |js| {
                    if let Some(window) = weak.upgrade() {
                        window.run_js(&js);
                    }
                });
                let closing = dictation.clone();
                window.window.connect_close_request(move |_| {
                    closing.close();
                    glib::Propagation::Proceed
                });
                dictation
            });
            dictation.handle(body);
        }
        "ampNativeHuddle" =>
        {
            #[cfg(feature = "huddles")]
            if body.get("command").and_then(Value::as_str) == Some("join") {
                app.join_huddle(body);
            }
        }
        "ampNativeOpenPuckWindow" => {
            let path = puck_path(body);
            match puck_window(&app) {
                None => {
                    app.open_window(&path, Some("puck"));
                }
                Some(puck) => {
                    if path != PUCK_PATH {
                        puck.navigate(&path);
                    }
                    puck.window.present();
                }
            }
        }
        "ampNativeTogglePuckWindow" => match puck_window(&app) {
            None => {
                app.open_window(&puck_path(body), Some("puck"));
            }
            Some(puck) if Rc::ptr_eq(&puck, window) || puck.window.is_active() => puck.window.close(),
            Some(puck) => puck.window.present(),
        },
        _ => {}
    }
}

fn on_reply(window: &Rc<AmpWindow>, name: &str, body: &Value, reply: Reply) {
    let Some(app) = window.app.upgrade() else { return };
    match name {
        "ampNativeClipboardWrite" => {
            write_clipboard(window, body);
            reply.send(Ok(Value::Null));
        }
        "ampNativeLocalNotification" => {
            let delivered = app.post_notification(body.get("notification"));
            reply.send(Ok(json!({"delivered": delivered})));
        }
        "ampNativeGlobalShortcuts" => {
            if body.get("action").and_then(Value::as_str) == Some("openSettings") {
                app.show_app_settings(Some("general"));
                reply.send(Ok(Value::Null));
            } else {
                reply.send(Ok(json!({"version": 1, "shortcuts": Menus::shortcut_list()})));
            }
        }
        "ampNativeClearCache" => {
            if let Some(manager) = app.network_session().website_data_manager() {
                manager.clear(
                    webkit::WebsiteDataTypes::MEMORY_CACHE | webkit::WebsiteDataTypes::DISK_CACHE,
                    glib::TimeSpan(0),
                    gtk::gio::Cancellable::NONE,
                    |_| {},
                );
            }
            reply.send(Ok(json!(true)));
        }
        "ampNativeDiagnostics" => reply.send(Ok(json!({
            "app": "amp-native",
            "version": config::VERSION,
            "platform": "linux",
            "webkit": webkit_version(),
            "gtk": format!("{}.{}.{}", gtk::major_version(), gtk::minor_version(), gtk::micro_version()),
            "runner": app.runner.status(),
        }))),
        "ampNativeUnifiedLog" => {
            let mut parts = Vec::new();
            for path in [&PATHS.log_file, &PATHS.stderr_log, &PATHS.runner_log] {
                if let Ok(bytes) = std::fs::read(path) {
                    let text = String::from_utf8_lossy(&bytes);
                    let count = text.chars().count();
                    let tail: String = text.chars().skip(count.saturating_sub(200_000)).collect();
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    parts.push(format!("==> {name} <==\n{tail}"));
                }
            }
            reply.send(Ok(json!({"log": parts.join("\n")})));
        }
        "ampNativeOpenInEditor" => reply.send(files::open_in_editor(body).map(|_| Value::Null)),
        "ampNativeShowInFinder" => files::show_in_file_manager(body, move |result| reply.send(result)),
        "ampNativeSaveFileStream" => {
            let result = files::save_stream(&app.adw, &mut app.save_transfer.borrow_mut(), body);
            reply.send(result);
        }
        "ampNativePasteboardImages" => {
            let clipboard = window.window.clipboard();
            glib::spawn_future_local(async move {
                reply.send(files::read_pasteboard_images(clipboard).await);
            });
        }
        "ampNativeImportLocalProjects" => import_local_projects(&app, window, body, reply),
        _ => reply.send(Err(format!("unknown handler {name}"))),
    }
}

pub fn webkit_version() -> String {
    format!(
        "{}.{}.{}",
        webkit::functions::major_version(),
        webkit::functions::minor_version(),
        webkit::functions::micro_version()
    )
}

fn write_clipboard(window: &AmpWindow, body: &Value) {
    let Some(text) = body.get("text").and_then(Value::as_str) else { return };
    let mut providers =
        vec![gdk::ContentProvider::for_bytes("text/plain;charset=utf-8", &glib::Bytes::from(text.as_bytes()))];
    if let Some(html) = body.get("html").and_then(Value::as_str) {
        providers.insert(0, gdk::ContentProvider::for_bytes("text/html", &glib::Bytes::from(html.as_bytes())));
    }
    if let Err(error) = window.window.clipboard().set_content(Some(&gdk::ContentProvider::new_union(&providers))) {
        log::warn!("could not set the clipboard: {error}");
    }
}

fn confirmation_sheet(window: &Rc<AmpWindow>, body: &Value) {
    if !body.is_object() || body.get("version") != Some(&json!(1)) {
        return;
    }
    let previous = window.confirmation.borrow_mut().take();
    if let Some(sheet) = previous {
        sheet.close();
    }
    if body.get("open").is_some_and(config::truthy)
        && let Some(state) = body.get("state").filter(|s| s.is_object())
    {
        let weak = Rc::downgrade(window);
        let sheet = ConfirmationSheet::new(&window.window, state, move |detail| {
            if let Some(window) = weak.upgrade() {
                window.confirmation.borrow_mut().take();
                window.run_js(&scripts::custom_event("amp:native-confirmation-sheet", &detail));
            }
        });
        *window.confirmation.borrow_mut() = Some(sheet);
    }
}

fn puck_path(body: &Value) -> String {
    match body.as_str() {
        Some(path) if path == PUCK_PATH || path.starts_with(&format!("{PUCK_PATH}/")) => path.to_string(),
        _ => PUCK_PATH.to_string(),
    }
}

fn puck_window(app: &App) -> Option<Rc<AmpWindow>> {
    // Closed windows stay in the list without an application until they are destroyed.
    app.windows().into_iter().find(|w| w.auxiliary == Some("puck") && w.window.application().is_some())
}

// ---- local projects ----------------------------------------------------------------------------

fn import_local_projects(app: &Rc<App>, window: &Rc<AmpWindow>, body: &Value, reply: Reply) {
    let action = body.get("action").and_then(Value::as_str).filter(|_| body.get("version") == Some(&json!(1)));
    match action {
        Some("discover") => {
            glib::spawn_future_local(async move {
                reply.send(Ok(local_projects::in_background(local_projects::discover).await));
            });
        }
        Some("browse") => browse(app, window, reply),
        Some("recordProjectFolders") => reply.send(Ok(record_project_folders(app, body.get("paths")))),
        _ => reply.send(Ok(json!({"status": "unavailable"}))),
    }
}

fn browse(app: &Rc<App>, window: &Rc<AmpWindow>, reply: Reply) {
    if app.browsing.get() {
        reply.send(Ok(json!({"status": "busy"})));
        return;
    }
    app.browsing.set(true);
    #[allow(deprecated)] // gtk::FileDialog picks several folders only from GTK 4.14.
    let chooser = gtk::FileChooserNative::builder()
        .title("Choose Git repositories, or a folder that contains them")
        .transient_for(&window.window)
        .modal(true)
        .action(gtk::FileChooserAction::SelectFolder)
        .select_multiple(true)
        .accept_label("Choose")
        .build();
    let weak_app = Rc::downgrade(app);
    let weak_window = Rc::downgrade(window);
    let reply = std::cell::RefCell::new(Some(reply));
    #[allow(deprecated)]
    chooser.connect_response(move |dialog, response| {
        if let Some(app) = weak_app.upgrade() {
            app.browsing.set(false);
        }
        let mut folders: Vec<String> = Vec::new();
        if response == gtk::ResponseType::Accept {
            let files = dialog.files();
            for i in 0..files.n_items() {
                if let Some(path) =
                    files.item(i).and_then(|f| f.downcast::<gtk::gio::File>().ok()).and_then(|f| f.path())
                {
                    folders.push(path.to_string_lossy().to_string());
                }
            }
        }
        dialog.destroy();
        if let Some(window) = weak_window.upgrade() {
            window.import_chooser.borrow_mut().take();
        }
        let Some(reply) = reply.borrow_mut().take() else { return };
        if folders.is_empty() {
            reply.send(Ok(json!({"status": "cancelled"})));
        } else {
            glib::spawn_future_local(async move {
                reply.send(Ok(local_projects::in_background(move || local_projects::scan_folders(&folders)).await));
            });
        }
    });
    #[allow(deprecated)]
    chooser.show();
    *window.import_chooser.borrow_mut() = Some(chooser);
}

fn record_project_folders(app: &Rc<App>, paths: Option<&Value>) -> Value {
    let Some(paths) = paths.and_then(Value::as_array) else { return json!({"status": "invalid"}) };
    let paths: Option<Vec<String>> =
        paths.iter().map(|p| p.as_str().filter(|p| p.starts_with('/')).map(String::from)).collect();
    let Some(paths) = paths else { return json!({"status": "invalid"}) };
    let mut dirs = app.settings.strings("runner_dirs");
    for path in paths {
        if !dirs.contains(&path) {
            dirs.push(path.clone());
            app.runner.dirs_command(app, "add", &[path]);
        }
    }
    app.settings.set("runner_dirs", json!(dirs));
    app.refresh_settings_runner();
    json!({"status": "ok"})
}

// ---- window actions ----------------------------------------------------------------------------

type WindowAction = Box<dyn Fn(&Rc<AmpWindow>)>;

/// The window half of the menu bar: File, Edit and View commands that act on one window.
pub fn add_window_actions(app: &Rc<App>, window: &Rc<AmpWindow>) {
    let add = |name: &str, f: WindowAction| {
        let action = gtk::gio::SimpleAction::new(name, None);
        let weak = Rc::downgrade(window);
        action.connect_activate(move |_, _| {
            if let Some(window) = weak.upgrade() {
                f(&window);
            }
        });
        window.window.add_action(&action);
    };
    let command = |name: &'static str| -> WindowAction {
        Box::new(move |w: &Rc<AmpWindow>| w.app_command(name, json!({}), None))
    };
    let edit = |name: &'static glib::GStr| -> WindowAction {
        Box::new(move |w: &Rc<AmpWindow>| w.webview.execute_editing_command(name))
    };
    fn print_dialog(w: &AmpWindow) {
        webkit::PrintOperation::new(&w.webview).run_dialog(Some(&w.window));
    }
    fn flag(w: &AmpWindow, key: &str) -> bool {
        w.app_state.borrow().get(key).is_some_and(config::truthy)
    }
    add("personal-settings", command("openSettings"));
    add("workspace-settings", command("openWorkspaceSettings"));
    add("new-thread", command("createThread"));
    add(
        "close",
        Box::new(|w| {
            let tab = w.app_state.borrow().get("paneTabClose").cloned().unwrap_or(Value::Null);
            let closable = tab.get("hasClosableTab").is_some_and(config::truthy)
                && !tab.get("blocked").is_some_and(config::truthy);
            if closable {
                let window = w.window.clone();
                w.app_command("closeActiveTab", json!({}), Some(Box::new(move || window.close())));
            } else {
                w.window.close();
            }
        }),
    );
    add(
        "print",
        Box::new(|w| {
            if flag(w, "printAvailable") {
                let weak = Rc::downgrade(w);
                w.app_command(
                    "print",
                    json!({}),
                    Some(Box::new(move || {
                        if let Some(w) = weak.upgrade() {
                            print_dialog(&w);
                        }
                    })),
                );
            } else {
                print_dialog(w);
            }
        }),
    );
    add("undo", edit(webkit::EDITING_COMMAND_UNDO));
    add("redo", edit(webkit::EDITING_COMMAND_REDO));
    add("cut", edit(webkit::EDITING_COMMAND_CUT));
    add("copy", edit(webkit::EDITING_COMMAND_COPY));
    add("paste", edit(webkit::EDITING_COMMAND_PASTE));
    add("select-all", edit(webkit::EDITING_COMMAND_SELECT_ALL));
    add("toggle-sidebar", command("toggleSidebar"));
    add("toggle-puck", command("togglePuck"));
    add("back", Box::new(|w| w.webview.go_back()));
    add("forward", Box::new(|w| w.webview.go_forward()));
    add(
        "reload-portal",
        Box::new(|w| {
            if flag(w, "portalReloadAvailable") {
                let view = w.webview.clone();
                w.app_command("reloadPortal", json!({}), Some(Box::new(move || view.reload())));
            } else {
                w.webview.reload();
            }
        }),
    );
    add("reload", Box::new(|w| w.webview.reload()));
    add("fullscreen", Box::new(|w| w.toggle_full_screen()));
    add(
        "inspector",
        Box::new(|w| {
            if let Some(inspector) = w.webview.inspector() {
                inspector.show();
            }
        }),
    );
    add("minimize", Box::new(|w| w.window.minimize()));
    add("zoom", Box::new(|w| w.toggle_maximized()));
    add(
        "keyboard-shortcuts",
        Box::new(|w| w.run_js("document.dispatchEvent(new KeyboardEvent('keydown', {key: '?', bubbles: true}))")),
    );
    add("report-bug", command("reportBug"));
    let weak = Rc::downgrade(app);
    window.window.connect_is_active_notify(move |_| {
        if let Some(app) = weak.upgrade() {
            app.update_actions();
        }
    });
    let weak = Rc::downgrade(app);
    window.window.connect_fullscreened_notify(move |_| {
        if let Some(app) = weak.upgrade() {
            app.update_actions();
        }
    });
}

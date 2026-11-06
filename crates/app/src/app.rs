//! The application: windows, the Mac menu bar's commands and shortcuts, notifications, and badges.

use std::cell::{Cell, OnceCell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use amp_common::config::{self, APP_ID, APP_NAME, APP_URL, BUILD_NUMBER, BUNDLE_ID, PATHS, Settings};
use amp_common::scripts;
use amp_common::theme::{self, Palette};
use gtk::{gdk, gio, glib};
use serde_json::{Value, json};
use webkit::prelude::*;

use crate::files;
use crate::menus::{GLOBAL_COMMANDS, Menus};
use crate::runner::Runner;
use crate::settings_window::SettingsWindow;
use crate::signin::SignIn;
use crate::tray::Tray;
use crate::window::{AmpWindow, is_app_url};

const QUIT_HOLD: Duration = Duration::from_millis(900);
const CSS: &str = include_str!("style.css");

pub struct App {
    pub adw: adw::Application,
    pub settings: Settings,
    windows: RefCell<Vec<Rc<AmpWindow>>>,
    settings_window: RefCell<Option<Rc<SettingsWindow>>>,
    unread: Cell<i64>,
    pub sign_in: SignIn,
    pub runner: Runner,
    pub tray: Tray,
    menus: OnceCell<Menus>,
    /// A global command for a window that is still loading; it runs once the page is ready.
    pending_command: Cell<Option<&'static str>>,
    #[cfg(feature = "huddles")]
    huddles: RefCell<std::collections::HashMap<String, amp_huddle::HuddleWindow>>,
    pub user_agent_suffix: String,
    network_session: OnceCell<webkit::NetworkSession>,
    host_palette: RefCell<Option<Palette>>,
    theme_monitor: RefCell<Option<gio::FileMonitor>>,
    theme_timer: RefCell<Option<glib::SourceId>>,
    quit_hold: RefCell<Option<(glib::SourceId, Weak<AmpWindow>, u64)>>,
    /// The streamed save in progress (ampNativeSaveFileStream), one at a time for all windows.
    pub save_transfer: RefCell<Option<files::Transfer>>,
    /// Whether a local project import folder chooser is open.
    pub browsing: Cell<bool>,
    dictation_available: OnceCell<bool>,
    /// Keeps Amp running with no window open, as the Mac app stays in the Dock; the tray, the
    /// launcher and `--action show` bring the window back, and Quit ends it.
    keep_running: OnceCell<gio::ApplicationHoldGuard>,
}

pub fn run() -> std::process::ExitCode {
    let adw = adw::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::HANDLES_OPEN).build();
    glib::set_application_name(APP_NAME);
    let app = Rc::new(App {
        adw: adw.clone(),
        settings: Settings::load(),
        windows: RefCell::default(),
        settings_window: RefCell::default(),
        unread: Cell::new(0),
        sign_in: SignIn::default(),
        runner: Runner::default(),
        tray: Tray::default(),
        menus: OnceCell::new(),
        pending_command: Cell::new(None),
        #[cfg(feature = "huddles")]
        huddles: RefCell::default(),
        user_agent_suffix: format!(
            // WebKit only accepts RFC 9110 product tokens, so the two-slash AmpApp token goes in a
            // comment, where the page's /\bAmpApp\/(...)\/(\d+)\b/ still finds it.
            "AmpNativeApp/1 AmpPlatform/Linux AmpAppVersion/0.1.{BUILD_NUMBER} (AmpApp/{BUNDLE_ID}/{BUILD_NUMBER}) \
             AmpMCPOAuth/1 AmpConnectOAuth/1 AmpConnectOAuth/2 AmpConnectOAuth/3"
        ),
        network_session: OnceCell::new(),
        host_palette: RefCell::default(),
        theme_monitor: RefCell::default(),
        theme_timer: RefCell::default(),
        quit_hold: RefCell::default(),
        save_transfer: RefCell::default(),
        browsing: Cell::new(false),
        dictation_available: OnceCell::new(),
        keep_running: OnceCell::new(),
    });
    let names: Vec<&str> = GLOBAL_COMMANDS.iter().map(|(n, _)| *n).collect();
    adw.add_main_option(
        "action",
        glib::Char::from(0),
        glib::OptionFlags::NONE,
        glib::OptionArg::String,
        &format!("Run an action in the running Amp: {}", names.join(", ")),
        Some("NAME"),
    );
    let a = app.clone();
    adw.connect_handle_local_options(move |_, options| a.handle_local_options(options));
    let a = app.clone();
    adw.connect_startup(move |_| a.startup());
    let a = app.clone();
    adw.connect_activate(move |_| a.present_main());
    let a = app.clone();
    adw.connect_open(move |_, files, _| a.open(files));
    let a = app.clone();
    adw.connect_shutdown(move |_| a.shutdown());
    let a = app.clone();
    adw.connect_window_removed(move |_, window| a.window_removed(window));
    adw.run().into()
}

impl App {
    // ---- lifecycle -------------------------------------------------------------------------

    fn startup(self: &Rc<Self>) {
        for dir in [&PATHS.data_dir, &PATHS.cache_dir, &PATHS.config_dir] {
            let _ = std::fs::create_dir_all(dir);
        }
        let web_data = PATHS.data_dir.join("web");
        let session = webkit::NetworkSession::new(
            Some(&web_data.to_string_lossy()),
            Some(&PATHS.cache_dir.join("web").to_string_lossy()),
        );
        if let Some(cookies) = session.cookie_manager() {
            cookies.set_persistent_storage(
                &web_data.join("cookies.sqlite").to_string_lossy(),
                webkit::CookiePersistentStorage::Sqlite,
            );
        }
        let weak = Rc::downgrade(self);
        session.connect_download_started(move |_, download| {
            if let Some(app) = weak.upgrade() {
                app.download_started(download);
            }
        });
        let _ = self.network_session.set(session);

        let provider = gtk::CssProvider::new();
        #[allow(deprecated)] // load_from_string needs GTK 4.12.
        provider.load_from_data(CSS);
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        gtk::Window::set_default_icon_name(APP_ID);
        self.apply_appearance();
        self.watch_theme();

        let _ = self.menus.set(Menus::new());
        self.add_actions();
        self.update_actions();
        self.tray.start(self);
        let _ = self.keep_running.set(self.adw.hold());
        if self.settings.bool("runner_enabled") {
            self.runner.apply(self);
        }
        if config::icon_file("app").is_none() {
            self.fetch_icons();
        }
    }

    fn open(self: &Rc<Self>, files: &[gio::File]) {
        if self.main_window().is_none() {
            self.open_window("/", None);
        }
        for file in files {
            let uri = file.uri().to_string();
            if uri.starts_with(&format!("{}:", config::SIGN_IN_SCHEME)) {
                let handled = self.sign_in.handle_callback(self, &uri);
                log::info!("sign-in callback {}", if handled { "accepted" } else { "ignored (no matching request)" });
            } else if is_app_url(&uri) {
                let path = uri.strip_prefix(APP_URL.as_str()).filter(|p| !p.is_empty()).unwrap_or("/");
                self.open_window(path, None);
            }
        }
        if let Some(main) = self.main_window() {
            main.window.present();
        }
    }

    fn shutdown(self: &Rc<Self>) {
        #[cfg(feature = "huddles")]
        {
            let huddles: Vec<_> = self.huddles.borrow().values().cloned().collect();
            for huddle in huddles {
                huddle.close();
                huddle.wait_left(Duration::from_secs(3));
            }
        }
        self.runner.stop();
    }

    fn handle_local_options(self: &Rc<Self>, options: &glib::VariantDict) -> std::ops::ControlFlow<glib::ExitCode> {
        let Ok(Some(name)) = options.lookup::<String>("action") else {
            return std::ops::ControlFlow::Continue(());
        };
        let Some((name, command)) = GLOBAL_COMMANDS.iter().find(|(n, _)| *n == name) else {
            let names: Vec<&str> = GLOBAL_COMMANDS.iter().map(|(n, _)| *n).collect();
            eprintln!("amp-linux-desktop: unknown action '{name}'; one of: {}", names.join(", "));
            return std::ops::ControlFlow::Break(glib::ExitCode::from(2));
        };
        if self.adw.register(gio::Cancellable::NONE).is_ok() && self.adw.is_remote() {
            self.adw.activate_action(name, None);
            return std::ops::ControlFlow::Break(glib::ExitCode::SUCCESS);
        }
        self.pending_command.set(*command);
        std::ops::ControlFlow::Continue(())
    }

    /// The menu bar's model and shortcuts, built at startup.
    pub fn menus(&self) -> &Menus {
        self.menus.get().expect("built at startup")
    }

    pub fn network_session(&self) -> &webkit::NetworkSession {
        self.network_session.get().expect("created at startup")
    }

    pub fn dictation_available(&self) -> bool {
        *self.dictation_available.get_or_init(amp_dictation::available)
    }

    // ---- windows ---------------------------------------------------------------------------

    pub fn windows(&self) -> Vec<Rc<AmpWindow>> {
        self.windows.borrow().clone()
    }

    pub fn main_window(&self) -> Option<Rc<AmpWindow>> {
        self.windows.borrow().iter().find(|w| w.window.application().is_some()).cloned()
    }

    pub fn present_main(self: &Rc<Self>) {
        let window = self.main_window().unwrap_or_else(|| self.open_window("/", None));
        window.window.present();
    }

    /// Opens a window at `path`; `auxiliary` "puck" makes it a Puck window.
    pub fn open_window(self: &Rc<Self>, path: &str, auxiliary: Option<&'static str>) -> Rc<AmpWindow> {
        let window = AmpWindow::new(self, path, auxiliary);
        self.windows.borrow_mut().push(window.clone());
        crate::bridge::add_window_actions(self, &window);
        window.window.present();
        self.refresh_window_list();
        window
    }

    /// A closed window leaves the application before it is disposed, and it is only disposed once
    /// nothing holds it: drop the app's references here, not on `destroy`, which would never come.
    fn window_removed(&self, window: &gtk::Window) {
        let removed = self.find_window(window);
        if let Some(removed) = &removed {
            self.windows.borrow_mut().retain(|w| !Rc::ptr_eq(w, removed));
            self.refresh_window_list();
        }
        let settings =
            self.settings_window.borrow().as_ref().is_some_and(|s| s.window.upcast_ref::<gtk::Window>() == window);
        if settings {
            self.settings_window.borrow_mut().take();
        }
    }

    fn find_window(&self, window: &gtk::Window) -> Option<Rc<AmpWindow>> {
        self.windows.borrow().iter().find(|w| w.window.upcast_ref::<gtk::Window>() == window).cloned()
    }

    pub fn show_app_settings(self: &Rc<Self>, tab: Option<&str>) {
        let existing = self.settings_window.borrow().clone();
        let window = existing.unwrap_or_else(|| {
            let window = SettingsWindow::new(self);
            *self.settings_window.borrow_mut() = Some(window.clone());
            window
        });
        window.show_tab(tab.unwrap_or("general"));
    }

    pub fn refresh_settings_runner(self: &Rc<Self>) {
        let window = self.settings_window.borrow().clone();
        if let Some(window) = window {
            window.refresh_runner();
        }
    }

    /// Opens a link in the default browser. Uses the XDG handler directly, since the portal
    /// gtk::show_uri goes through is missing on many window-manager-only desktops. $BROWSER, the
    /// usual override, comes first.
    pub fn launch_uri(&self, uri: &str) {
        let spawn = |argv: Vec<std::ffi::OsString>| {
            use std::os::unix::process::CommandExt;
            std::process::Command::new(&argv[0])
                .args(&argv[1..])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .process_group(0)
                .spawn()
                .map(|mut child| {
                    std::thread::spawn(move || child.wait());
                })
        };
        let redacted = uri.split('?').next().unwrap_or("");
        if let Ok(browser) = std::env::var("BROWSER")
            && !browser.is_empty()
        {
            match glib::shell_parse_argv(&browser) {
                Ok(mut argv) if !argv.is_empty() => {
                    argv.push(uri.into());
                    match spawn(argv) {
                        Ok(()) => return,
                        Err(error) => log::error!("$BROWSER failed: {error}"),
                    }
                }
                _ => log::error!("$BROWSER failed: cannot parse it"),
            }
        }
        match gio::AppInfo::launch_default_for_uri(uri, gio::AppLaunchContext::NONE) {
            Ok(()) => return,
            Err(error) => log::info!("no default handler for {redacted}: {}", error.message()),
        }
        if let Err(error) = spawn(vec!["xdg-open".into(), uri.into()]) {
            log::error!("could not open {redacted}: {error}");
        }
    }

    pub fn open_in_browser(&self, uri: &str) {
        if !(uri.starts_with("http://") || uri.starts_with("https://") || uri.starts_with("mailto:")) {
            return;
        }
        self.sign_in.remember_relay(uri);
        self.launch_uri(uri);
    }

    // ---- page state ------------------------------------------------------------------------

    pub fn app_state_changed(self: &Rc<Self>, window: &Rc<AmpWindow>) {
        if self.main_window().is_some_and(|m| Rc::ptr_eq(&m, window)) {
            let state = window.app_state.borrow().clone();
            self.runner.account_changed(self, state.get("signedInEmail").and_then(Value::as_str));
            if let Some(preference) = state.get("colorSchemePreference").and_then(Value::as_str)
                && ["system", "light", "dark"].contains(&preference)
                && preference != self.settings.str("appearance")
            {
                self.settings.set("appearance", json!(preference));
                self.apply_appearance();
            }
            let settings_window = self.settings_window.borrow().clone();
            if let Some(settings_window) = settings_window {
                settings_window.refresh_account();
            }
            if state.get("websiteReady").is_some_and(config::truthy)
                && let Some(command) = self.pending_command.take()
            {
                window.app_command(command, json!({}), None);
            }
        }
        self.update_actions();
    }

    pub fn run_global_command(self: &Rc<Self>, command: Option<&'static str>) {
        let Some(window) = self.main_window() else {
            // A new window takes the command once the page reports it is ready.
            self.pending_command.set(command);
            self.present_main();
            return;
        };
        window.window.present();
        if let Some(command) = command {
            window.app_command(command, json!({}), None);
        }
    }

    /// Joins a Huddle in its own window, or shows the window when this call is already open.
    #[cfg(feature = "huddles")]
    pub fn join_huddle(self: &Rc<Self>, body: &Value) {
        let text = |key: &str| body.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        let thread_id = text("threadID");
        if let Some(current) = self.huddles.borrow().get(&thread_id) {
            current.present();
            return;
        }
        let (Some(server_url), Some(token)) =
            (body.get("serverURL").and_then(Value::as_str), body.get("participantToken").and_then(Value::as_str))
        else {
            return;
        };
        let call_name = text("callName");
        let request = amp_huddle::JoinRequest {
            thread_id: thread_id.clone(),
            title: text("title"),
            call_name: if call_name == "Space" { call_name } else { "Huddle".into() },
            server_url: server_url.into(),
            participant_token: token.into(),
        };
        let window_slot: Rc<RefCell<Option<amp_huddle::WeakHuddleWindow>>> = Rc::default();
        let (weak, id, slot) = (Rc::downgrade(self), thread_id.clone(), window_slot.clone());
        let reconcile_weak = weak.clone();
        let reconcile_id = id.clone();
        let hooks = amp_huddle::AppHooks {
            reconcile: Box::new(move || {
                if let Some(app) = reconcile_weak.upgrade() {
                    app.huddle_reconcile(&reconcile_id);
                }
            }),
            ended: Box::new(move |message| {
                let Some(app) = weak.upgrade() else { return };
                let current = app.huddles.borrow().get(&id).cloned();
                let this = slot.borrow().as_ref().and_then(|w| w.upgrade());
                if let (Some(current), Some(this)) = (current, this)
                    && current.is(&this)
                {
                    app.huddles.borrow_mut().remove(&id);
                }
                if let (Some(message), Some(main)) = (message, app.main_window()) {
                    main.show_toast(&message);
                }
            }),
        };
        let window = amp_huddle::HuddleWindow::new(&self.adw, request, hooks);
        *window_slot.borrow_mut() = Some(window.downgrade());
        self.huddles.borrow_mut().insert(thread_id, window.clone());
        window.present();
    }

    /// Asks the server to update the thread's live call state, as the page does after it joins or
    /// leaves.
    #[cfg(feature = "huddles")]
    fn huddle_reconcile(&self, thread_id: &str) {
        if let Some(main) = self.main_window() {
            main.run_js(&format!(
                "fetch('/api/threads/' + encodeURIComponent({}) + '/space-token', \
                 {{method: 'POST', headers: {{'Content-Type': 'application/json'}}, \
                 body: JSON.stringify({{action: 'reconcile'}}), keepalive: true}}).catch(() => {{}}); true",
                json!(thread_id)
            ));
        }
    }

    pub fn open_thread(self: &Rc<Self>, thread_id: &str) {
        self.open_notification(&format!("/threads/{thread_id}"));
    }

    /// Tells the page its menu opened, so it sends the threads now instead of within 10 s.
    pub fn request_menu_refresh(&self) {
        if let Some(main) = self.main_window() {
            main.run_js(
                "window.dispatchEvent(new CustomEvent('amp:macos-menu-bar-menu', {detail: {open: true}}));\
                 setTimeout(() => window.dispatchEvent(new CustomEvent('amp:macos-menu-bar-menu', \
                 {detail: {open: false}})), 2000)",
            );
        }
    }

    pub fn menu_bar_changed(self: &Rc<Self>, body: &Value) {
        let unread = match body.get("unreadThreadCount") {
            Some(Value::Number(n)) if n.is_i64() => n.as_i64().unwrap_or(0),
            _ => self.unread.get(),
        };
        self.set_unread_count(unread);
        let enabled = body.get("enabled").is_none_or(config::truthy);
        let threads = body.get("threads").and_then(Value::as_array).filter(|_| enabled).cloned().unwrap_or_default();
        self.tray.update(&threads, self.unread.get());
    }

    pub fn update_actions(&self) {
        let active = self.adw.active_window().and_then(|w| self.find_window(&w));
        let state = active.as_ref().map(|w| w.app_state.borrow().clone()).unwrap_or(json!({}));
        let flag = |key: &str| state.get(key).is_some_and(config::truthy);
        let menus = self.menus();
        menus.set_label("win.toggle-sidebar", if flag("sidebarVisible") { "Hide Sidebar" } else { "Show Sidebar" });
        menus.set_label("win.toggle-puck", if flag("puckOpen") { "Hide Puck" } else { "Show Puck" });
        // As the Mac: plain Close where the page has no panes, Close Window where it has panes but
        // no closable tab, else Close Tab, which only works while a tab has focus (the page sends
        // its label then, and none while the composer has focus).
        let tab = state.get("paneTabClose").filter(|t| t.is_object());
        let closable = tab.is_some_and(|t| t.get("hasClosableTab").is_some_and(config::truthy));
        let tab_focused = tab.is_some_and(|t| {
            t.get("tabLabel").is_some_and(Value::is_string) && !t.get("blocked").is_some_and(config::truthy)
        });
        let close_label = match (tab, closable) {
            (None, _) => "Close",
            (Some(_), false) => "Close Window",
            (Some(_), true) => "Close Tab",
        };
        menus.set_label("win.close", close_label);
        let report =
            state.get("reportBugLabel").and_then(Value::as_str).filter(|l| !l.is_empty()).unwrap_or("Report Bug…");
        menus.set_label("win.report-bug", report);
        let set_enabled = |action: Option<gio::Action>, enabled: bool| {
            if let Some(action) = action.and_then(|a| a.downcast::<gio::SimpleAction>().ok()) {
                action.set_enabled(enabled);
            }
        };
        set_enabled(self.adw.lookup_action("zoom-reset"), self.zoom() != 1.0);
        if let Some(window) = active {
            for (name, enabled) in [
                ("close", !closable || tab_focused),
                ("close-window", closable),
                ("reload-portal", flag("portalReloadAvailable")),
                ("print", flag("printAvailable")),
                ("back", window.webview.can_go_back()),
                ("forward", window.webview.can_go_forward()),
            ] {
                set_enabled(window.window.lookup_action(name), enabled);
            }
        }
    }

    /// Lists the windows in the Window menu.
    pub fn refresh_window_list(&self) {
        let titles: Vec<String> = self
            .open_windows()
            .iter()
            .map(|w| w.window.title().map(|t| t.to_string()).filter(|t| !t.is_empty()).unwrap_or(APP_NAME.into()))
            .collect();
        self.menus().set_windows(&titles);
    }

    /// The windows still open, in the order the Window menu lists them.
    fn open_windows(&self) -> Vec<Rc<AmpWindow>> {
        self.windows.borrow().iter().filter(|w| w.window.application().is_some()).cloned().collect()
    }

    // ---- actions ---------------------------------------------------------------------------

    fn add_action(self: &Rc<Self>, name: &str, f: impl Fn(&Rc<App>) + 'static) {
        let action = gio::SimpleAction::new(name, None);
        let weak = Rc::downgrade(self);
        action.connect_activate(move |_, _| {
            if let Some(app) = weak.upgrade() {
                f(&app);
            }
        });
        self.adw.add_action(&action);
    }

    fn add_actions(self: &Rc<Self>) {
        self.add_action("about", |app| app.show_app_settings(Some("about")));
        self.add_action("app-settings", |app| app.show_app_settings(None));
        self.add_action("quit", |app| app.quit_all());
        self.add_action("new-window", |app| {
            app.open_window("/", None);
        });
        self.add_action("zoom-in", |app| app.set_zoom(config::step_zoom(app.zoom(), 1)));
        self.add_action("zoom-out", |app| app.set_zoom(config::step_zoom(app.zoom(), -1)));
        self.add_action("zoom-reset", |app| app.set_zoom(1.0));
        self.add_action("bring-all", |app| {
            for window in app.open_windows() {
                window.window.present();
            }
        });
        let present = gio::SimpleAction::new("present-window", Some(glib::VariantTy::UINT32));
        let weak = Rc::downgrade(self);
        present.connect_activate(move |_, parameter| {
            let (Some(app), Some(index)) = (weak.upgrade(), parameter.and_then(|p| p.get::<u32>())) else { return };
            if let Some(window) = app.open_windows().get(index as usize) {
                window.window.present();
            }
        });
        self.adw.add_action(&present);
        self.add_action("docs", |app| app.launch_uri("https://ampcode.com/manual"));

        let warn = gio::SimpleAction::new_stateful(
            "warn-before-quitting",
            None,
            &self.settings.bool("warn_before_quitting").to_variant(),
        );
        let weak = Rc::downgrade(self);
        warn.connect_activate(move |action, _| {
            let on = !action.state().and_then(|s| s.get::<bool>()).unwrap_or(false);
            action.set_state(&on.to_variant());
            if let Some(app) = weak.upgrade() {
                app.settings.set("warn_before_quitting", json!(on));
            }
        });
        let state = warn.clone();
        self.settings.connect(move |key, value| {
            if key == "warn_before_quitting" {
                state.set_state(&config::truthy(value).to_variant());
            }
        });
        self.adw.add_action(&warn);
        // Desktop-wide entry points, as the Mac's global shortcuts: the tray menu, the launcher's
        // desktop actions and `amp-linux-desktop --action NAME` (bind that in the compositor) run
        // these.
        for (name, command) in GLOBAL_COMMANDS {
            let command = *command;
            self.add_action(name, move |app| app.run_global_command(command));
        }
        let open_url = gio::SimpleAction::new("open-url", Some(glib::VariantTy::STRING));
        let weak = Rc::downgrade(self);
        open_url.connect_activate(move |_, parameter| {
            if let (Some(app), Some(url)) = (weak.upgrade(), parameter.and_then(|p| p.get::<String>())) {
                app.open_notification(&url);
            }
        });
        self.adw.add_action(&open_url);
    }

    /// Runs a menu shortcut before the page sees the key, as AppKit's key equivalents do.
    pub fn handle_shortcut(
        self: &Rc<Self>,
        window: &Rc<AmpWindow>,
        keyval: gdk::Key,
        keycode: u32,
        state: gdk::ModifierType,
    ) -> bool {
        let display = gtk::prelude::WidgetExt::display(&window.window);
        let Some(action) = self.menus().shortcut_for(&display, keyval, keycode, state) else { return false };
        if action == "app.quit" && self.settings.bool("warn_before_quitting") {
            self.hold_to_quit(window);
            return true;
        }
        let (scope, name) = action.split_once('.').expect("scoped action name");
        let target = if scope == "app" { self.adw.lookup_action(name) } else { window.window.lookup_action(name) };
        match target {
            Some(target) if target.is_enabled() => {
                target.activate(None);
                true
            }
            _ => false,
        }
    }

    fn hold_to_quit(self: &Rc<Self>, window: &Rc<AmpWindow>) {
        if self.quit_hold.borrow().is_some() {
            return;
        }
        window.show_toast("Hold Ctrl+Q to Quit");
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(QUIT_HOLD, move || {
            if let Some(app) = weak.upgrade() {
                app.quit_hold.borrow_mut().take();
                app.quit_all();
            }
        });
        let weak = Rc::downgrade(self);
        let handler = window.on_key_release(gdk::Key::q, move || {
            let Some(app) = weak.upgrade() else { return };
            let hold = app.quit_hold.borrow_mut().take();
            if let Some((timer, window, handler)) = hold {
                timer.remove();
                if let Some(window) = window.upgrade() {
                    window.disconnect_key_release(handler);
                }
            }
        });
        *self.quit_hold.borrow_mut() = Some((timer, Rc::downgrade(window), handler));
    }

    pub fn quit_all(&self) {
        for window in self.windows() {
            window.window.close();
        }
        let settings_window = self.settings_window.borrow().clone();
        if let Some(settings_window) = settings_window {
            settings_window.window.close();
        }
        self.adw.quit();
    }

    // ---- settings --------------------------------------------------------------------------

    pub fn zoom(&self) -> f64 {
        config::nearest_zoom(&self.settings.get("zoom"))
    }

    pub fn set_zoom(&self, factor: f64) {
        let factor = config::nearest_zoom_f64(factor);
        self.settings.set("zoom", json!(factor));
        for window in self.windows() {
            window.set_zoom(factor);
        }
        self.update_actions();
    }

    pub fn set_appearance(&self, value: &str) {
        self.settings.set("appearance", json!(value));
        self.apply_appearance();
        for window in self.windows() {
            window.app_command("setColorScheme", json!({"colorScheme": value}), None);
        }
    }

    pub fn host_palette(&self) -> Option<Palette> {
        self.host_palette.borrow().clone()
    }

    pub fn host_css(&self) -> Option<String> {
        self.host_palette.borrow().as_ref().map(theme::palette_css)
    }

    /// Follows the Omarchy theme: recolors the page from ~/.local/state/omarchy/current/theme now
    /// and whenever `omarchy theme set` repoints that link.
    fn watch_theme(self: &Rc<Self>) {
        let Some(directory) = theme::current_directory() else { return };
        // `omarchy theme set` repoints the current/theme symlink, so watch its directory.
        match gio::File::for_path(&directory)
            .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        {
            Ok(monitor) => {
                let weak = Rc::downgrade(self);
                let dir = directory.clone();
                monitor.connect_changed(move |_, _, _, _| {
                    let Some(app) = weak.upgrade() else { return };
                    if let Some(timer) = app.theme_timer.borrow_mut().take() {
                        timer.remove();
                    }
                    let weak = Rc::downgrade(&app);
                    let dir = dir.clone();
                    let timer = glib::timeout_add_local_once(Duration::from_millis(100), move || {
                        if let Some(app) = weak.upgrade() {
                            app.theme_timer.borrow_mut().take();
                            app.palette_changed(theme::read_palette(&dir));
                        }
                    });
                    *app.theme_timer.borrow_mut() = Some(timer);
                });
                *self.theme_monitor.borrow_mut() = Some(monitor);
            }
            Err(error) => log::warn!("cannot watch the Omarchy theme: {error}"),
        }
        self.palette_changed(theme::read_palette(&directory));
    }

    fn palette_changed(&self, palette: Option<Palette>) {
        *self.host_palette.borrow_mut() = palette;
        self.apply_appearance();
        for window in self.windows() {
            window.apply_host_css(true);
        }
    }

    fn apply_appearance(&self) {
        let manager = adw::StyleManager::default();
        let palette = self.host_palette.borrow();
        let appearance = self.settings.str("appearance");
        if let Some(palette) = palette.as_ref()
            && appearance == "system"
        {
            // A desktop theme decides light or dark, as amp-desktop does with nativeTheme.
            manager.set_color_scheme(if palette.dark {
                adw::ColorScheme::ForceDark
            } else {
                adw::ColorScheme::ForceLight
            });
            return;
        }
        manager.set_color_scheme(match appearance.as_str() {
            "light" => adw::ColorScheme::ForceLight,
            "dark" => adw::ColorScheme::ForceDark,
            _ => adw::ColorScheme::Default,
        });
    }

    pub fn set_open_at_login(&self, enabled: bool) {
        self.settings.set("open_at_login", json!(enabled));
        let autostart = glib::user_config_dir().join("autostart").join(format!("{APP_ID}.desktop"));
        if enabled {
            let exec = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("amp-linux-desktop"));
            let entry = format!(
                "[Desktop Entry]\nType=Application\nName=Amp\nExec={}\nIcon={APP_ID}\nX-GNOME-Autostart-enabled=true\n",
                exec.display()
            );
            let result = std::fs::create_dir_all(autostart.parent().expect("has a parent"))
                .and_then(|_| std::fs::write(&autostart, entry));
            if let Err(error) = result {
                log::warn!("could not add Amp to autostart: {error}");
            }
        } else {
            let _ = std::fs::remove_file(&autostart);
        }
    }

    /// The .deb and .rpm cannot carry Amp's icons; take them from Amp's disk image once, off the
    /// main thread, then install the launcher icon. A failure leaves the generic icon and is
    /// retried at the next start.
    fn fetch_icons(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(|| amp_common::icons::fetch(&PATHS.icon_dir, None)).await;
            match result {
                Ok(Ok(names)) => {
                    log::info!("fetched Amp's icons: {}", names.join(", "));
                    if let Some(app) = weak.upgrade() {
                        app.set_app_icon(&app.settings.str("app_icon"));
                    }
                }
                Ok(Err(error)) => log::warn!("could not fetch Amp's icons: {error}"),
                Err(_) => log::warn!("could not fetch Amp's icons: the extraction panicked"),
            }
        });
    }

    pub fn set_app_icon(&self, key: &str) {
        self.settings.set("app_icon", json!(key));
        let Some(source) = config::icon_file(key) else { return };
        let target = glib::user_data_dir().join("icons/hicolor/512x512/apps").join(format!("{APP_ID}.png"));
        let copied = std::fs::create_dir_all(target.parent().expect("has a parent"))
            .and_then(|_| std::fs::copy(&source, &target));
        if let Err(error) = copied {
            log::warn!("could not install the app icon: {error}");
            return;
        }
        if let Some(display) = gdk::Display::default() {
            gtk::IconTheme::for_display(&display).add_search_path(glib::user_data_dir().join("icons/hicolor"));
        }
        for window in self.windows() {
            window.window.set_icon_name(None);
            window.window.set_icon_name(Some(APP_ID));
        }
        self.tray.set_icon(key);
        let settings_window = self.settings_window.borrow().clone();
        if let Some(settings_window) = settings_window {
            settings_window.refresh_app_icon(key);
        }
    }

    // ---- notifications and badge -----------------------------------------------------------

    pub fn enable_notifications(&self) {
        self.settings.set("notifications", json!("authorized"));
        for window in self.windows() {
            // The page reads the permission at every load, so later loads need it too.
            window.reload_user_scripts();
            window.run_js(&scripts::notification_permission("authorized"));
        }
        let settings_window = self.settings_window.borrow().clone();
        if let Some(settings_window) = settings_window {
            settings_window.refresh_notifications();
        }
    }

    pub fn post_notification(&self, notification: Option<&Value>) -> bool {
        let Some(notification) = notification.filter(|n| n.is_object()) else { return false };
        if self.settings.str("notifications") != "authorized" {
            return false;
        }
        let (Some(title), Some(collapse)) =
            (notification.get("title").and_then(Value::as_str), notification.get("collapseID").and_then(Value::as_str))
        else {
            return false;
        };
        let text = |key: &str| match notification.get(key) {
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            Some(Value::Null) | None => None,
            Some(Value::String(_)) => None,
            Some(other) => Some(other.to_string()),
        };
        let mut body = text("body");
        if let Some(subtitle) = text("subtitle") {
            body = Some(format!("{subtitle}\n{}", body.unwrap_or_default()));
        }
        let n = gio::Notification::new(title);
        if let Some(body) = body.filter(|b| !b.is_empty()) {
            n.set_body(Some(&body));
        }
        n.set_icon(&gio::ThemedIcon::new(APP_ID));
        if notification.get("interruptionLevel").and_then(Value::as_str) == Some("passive") {
            n.set_priority(gio::NotificationPriority::Low);
        }
        if let Some(url) = text("url") {
            n.set_default_action_and_target_value("app.open-url", Some(&url.to_variant()));
        }
        self.adw.send_notification(Some(collapse), &n);
        true
    }

    fn open_notification(self: &Rc<Self>, url: &str) {
        let window = self.main_window().unwrap_or_else(|| self.open_window("/", None));
        window.window.present();
        let path = url.strip_prefix(APP_URL.as_str()).unwrap_or(url);
        if path.starts_with('/') {
            window.navigate(path);
        }
    }

    fn set_unread_count(&self, count: i64) {
        self.unread.set(count.max(0));
        self.refresh_badge();
    }

    /// Sets the launcher badge through the Unity LauncherEntry API (KDE, Dash to Dock, Plank).
    pub fn refresh_badge(&self) {
        let Some(connection) = self.adw.dbus_connection() else { return };
        let visible = self.settings.bool("show_unread_count") && self.unread.get() > 0;
        let properties = glib::VariantDict::new(None);
        properties.insert_value("count", &self.unread.get().to_variant());
        properties.insert_value("count-visible", &visible.to_variant());
        // (sa{sv}): a tuple of the variants themselves; to_variant() on a tuple would box the dict.
        let params =
            glib::Variant::tuple_from_iter([format!("application://{APP_ID}.desktop").to_variant(), properties.end()]);
        let path = format!("/{}", APP_ID.replace('.', "/"));
        if let Err(error) =
            connection.emit_signal(None, &path, "com.canonical.Unity.LauncherEntry", "Update", Some(&params))
        {
            log::debug!("badge not updated: {}", error.message());
        }
    }

    pub fn runner_changed(self: &Rc<Self>) {
        let runner_id = self.runner.running().then(|| Runner::runner_id(&self.settings));
        for window in self.windows() {
            window.run_js(&scripts::runner_state(runner_id.as_deref()));
        }
        self.refresh_settings_runner();
    }

    // ---- downloads -------------------------------------------------------------------------

    fn download_started(&self, download: &webkit::Download) {
        download.connect_decide_destination(|download, suggested| {
            let folder = glib::user_special_dir(glib::UserDirectory::Downloads).unwrap_or_else(glib::home_dir);
            let name = std::path::Path::new(suggested).file_name().and_then(|n| n.to_str()).unwrap_or("download");
            match files::unique_target(&folder, name) {
                Ok(target) => download.set_destination(&target.to_string_lossy()),
                Err(_) => download.cancel(),
            }
            true
        });
        let app = self.adw.clone();
        download.connect_finished(move |download| {
            let destination = download.destination().map(|d| d.to_string()).unwrap_or_default();
            let path = if destination.starts_with("file:") {
                glib::filename_from_uri(&destination).map(|(p, _)| p).unwrap_or_else(|_| destination.clone().into())
            } else {
                destination.into()
            };
            let n = gio::Notification::new("Download Finished");
            n.set_body(path.file_name().and_then(|n| n.to_str()));
            app.send_notification(None, &n);
        });
    }
}

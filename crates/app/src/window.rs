//! The Amp window: a WebKitGTK view of ampcode.com in the macOS app's shell mode.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::LazyLock;

use adw::prelude::*;
use amp_common::config::{self, APP_NAME, APP_URL};
use amp_common::scripts;
use gtk::{gdk, glib};
use serde_json::{Value, json};
use webkit::prelude::*;

use crate::app::App;
use crate::confirmation::ConfirmationSheet;
use crate::signin;
use crate::toasts::ToastCenter;
use crate::window_controls::WindowControls;

static APP_HOST: LazyLock<String> =
    LazyLock::new(|| url::Url::parse(&APP_URL).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_default());

/// Where the user scripts and style sheets apply: the app's origin and its subdomains.
static ALLOW_LIST: LazyLock<[String; 2]> = LazyLock::new(|| {
    let scheme = url::Url::parse(&APP_URL).map(|u| u.scheme().to_string()).unwrap_or_else(|_| "https".into());
    [format!("{scheme}://{}/*", *APP_HOST), format!("https://*.{}/*", *APP_HOST)]
});

fn allow_list() -> Vec<&'static str> {
    ALLOW_LIST.iter().map(String::as_str).collect()
}

pub fn is_app_url(uri: &str) -> bool {
    let Ok(url) = url::Url::parse(uri) else { return false };
    let host = url.host_str().unwrap_or("");
    let local = matches!(APP_HOST.as_str(), "localhost" | "127.0.0.1");
    (url.scheme() == "https" || (local && url.scheme() == "http"))
        && (host == *APP_HOST || host.ends_with(&format!(".{}", *APP_HOST)))
}

/// The last pointer press on the page, for a window drag the page starts with ampNativeWindowDrag.
struct Press {
    button: u32,
    device: Option<gdk::Device>,
    x: f64,
    y: f64,
    time: u32,
}

type ReleaseHandler = (gdk::Key, Rc<dyn Fn()>);

pub struct AmpWindow {
    pub window: adw::ApplicationWindow,
    pub webview: webkit::WebView,
    pub app: Weak<App>,
    manager: webkit::UserContentManager,
    /// The page's latest ampNativeAppState.
    pub app_state: RefCell<Value>,
    pub current_url: RefCell<Option<String>>,
    /// "puck" for a Puck window.
    pub auxiliary: Option<&'static str>,
    last_press: RefCell<Option<Press>>,
    visibility_sequence: Cell<u64>,
    pub confirmation: RefCell<Option<ConfirmationSheet>>,
    controls: WindowControls,
    pub toasts: adw::ToastOverlay,
    menu_revealer: gtk::Revealer,
    theme_sheet: RefCell<Option<webkit::UserStyleSheet>>,
    pub toast_center: OnceCell<ToastCenter>,
    pub dictation: OnceCell<amp_dictation::Dictation>,
    alt_alone: Cell<bool>,
    release_handlers: RefCell<HashMap<u64, ReleaseHandler>>,
    next_release_handler: Cell<u64>,
    /// Keeps the local project import's folder chooser alive until it answers.
    #[allow(deprecated)]
    pub import_chooser: RefCell<Option<gtk::FileChooserNative>>,
}

impl AmpWindow {
    pub fn new(app: &Rc<App>, path: &str, auxiliary: Option<&'static str>) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder().application(&app.adw).title(APP_NAME).build();
        let geometry = app.settings.get("window");
        let dimension =
            |key: &str, default: i32| geometry.get(key).and_then(Value::as_i64).map_or(default, |v| v as i32);
        if auxiliary == Some("puck") {
            window.set_default_size(480, 720);
        } else {
            window.set_default_size(dimension("width", 1280), dimension("height", 800));
        }
        window.set_size_request(480, 360);
        if geometry.get("maximized").is_some_and(config::truthy) && auxiliary.is_none() {
            window.maximize();
        }

        let manager = webkit::UserContentManager::new();
        let webview =
            webkit::WebView::builder().network_session(app.network_session()).user_content_manager(&manager).build();
        let menu_bar = gtk::PopoverMenuBar::from_model(Some(&app.menus().model));
        menu_bar.add_css_class("amp-menu-bar");
        // The Mac menu bar, shown with Alt the way Linux apps without a menu bar do it.
        let menu_revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Start)
            .child(&menu_bar)
            .build();

        let this = Rc::new_cyclic(|weak: &Weak<AmpWindow>| {
            let weak = weak.clone();
            let controls = WindowControls::new(move || {
                if let Some(this) = weak.upgrade() {
                    this.push_metrics();
                }
            });
            let overlay = gtk::Overlay::new();
            overlay.set_child(Some(&webview));
            overlay.add_overlay(&controls.start);
            overlay.add_overlay(&controls.end);
            overlay.add_overlay(&menu_revealer);
            let toasts = adw::ToastOverlay::new();
            toasts.set_child(Some(&overlay));
            window.set_content(Some(&toasts));
            AmpWindow {
                window: window.clone(),
                webview: webview.clone(),
                app: Rc::downgrade(app),
                manager,
                app_state: RefCell::new(json!({"version": 1, "websiteReady": false})),
                current_url: RefCell::default(),
                auxiliary,
                last_press: RefCell::default(),
                visibility_sequence: Cell::new(0),
                confirmation: RefCell::default(),
                controls,
                toasts,
                menu_revealer,
                theme_sheet: RefCell::default(),
                toast_center: OnceCell::new(),
                dictation: OnceCell::new(),
                alt_alone: Cell::new(false),
                release_handlers: RefCell::default(),
                next_release_handler: Cell::new(1),
                import_chooser: RefCell::default(),
            }
        });
        this.setup_webview(app);
        this.install_shortcuts();
        let weak = Rc::downgrade(&this);
        this.window.connect_is_active_notify(move |_| {
            if let Some(this) = weak.upgrade() {
                this.publish_host_visibility();
            }
        });
        let weak = Rc::downgrade(&this);
        this.window.connect_fullscreened_notify(move |w| {
            if let Some(this) = weak.upgrade() {
                this.controls.set_visible(!w.is_fullscreen());
                this.push_metrics();
            }
        });
        let weak = Rc::downgrade(&this);
        this.window.connect_close_request(move |_| {
            if let Some(this) = weak.upgrade() {
                this.close_request();
            }
            glib::Propagation::Proceed
        });
        let weak = Rc::downgrade(&this);
        glib::timeout_add_seconds_local(15, move || match weak.upgrade() {
            Some(this) if this.window.is_mapped() || this.webview.parent().is_some() => {
                this.publish_host_visibility();
                glib::ControlFlow::Continue
            }
            _ => glib::ControlFlow::Break,
        });
        this.load_path(path);
        this
    }

    fn app(&self) -> Option<Rc<App>> {
        self.app.upgrade()
    }

    // ---- web view --------------------------------------------------------------------------

    fn setup_webview(self: &Rc<Self>, app: &Rc<App>) {
        self.reload_user_scripts();
        self.manager.add_style_sheet(&webkit::UserStyleSheet::new(
            scripts::LINUX_CSS,
            webkit::UserContentInjectedFrames::TopFrame,
            webkit::UserStyleLevel::Author,
            &allow_list(),
            &[],
        ));
        self.apply_host_css(false);
        crate::bridge::register_handlers(app, self, &self.manager);

        let view = &self.webview;
        if let Some(settings) = webkit::prelude::WebViewExt::settings(view) {
            let agent = settings.user_agent().map(|a| a.to_string()).unwrap_or_default();
            settings.set_user_agent(Some(&format!("{agent} {}", app.user_agent_suffix)));
            settings.set_enable_developer_extras(true);
            settings.set_javascript_can_access_clipboard(true);
            settings.set_enable_back_forward_navigation_gestures(true);
            settings.set_media_playback_requires_user_gesture(false);
            settings.set_enable_media_stream(true);
        }
        view.set_zoom_level(app.zoom());
        view.set_background_color(&self.background_rgba());

        let weak = Rc::downgrade(self);
        view.connect_decide_policy(move |_, decision, kind| {
            weak.upgrade().is_some_and(|this| this.decide_policy(decision, kind))
        });
        let weak = Rc::downgrade(self);
        view.connect_create(move |_, action| {
            if let Some(this) = weak.upgrade() {
                this.create_web_view(action);
            }
            None
        });
        view.connect_permission_request(|_, request| {
            if request.is::<webkit::UserMediaPermissionRequest>()
                || request.is::<webkit::ClipboardPermissionRequest>()
                || request.is::<webkit::NotificationPermissionRequest>()
            {
                request.allow();
                return true;
            }
            false
        });
        let weak = Rc::downgrade(self);
        view.connect_load_changed(move |view, event| {
            let Some(this) = weak.upgrade() else { return };
            match event {
                webkit::LoadEvent::Committed => {
                    if signin::is_sign_in_page(&view.uri().unwrap_or_default()) {
                        this.load_path("/");
                    }
                }
                webkit::LoadEvent::Finished => {
                    this.publish_host_visibility();
                    this.push_metrics();
                }
                _ => {}
            }
            if let Some(app) = this.app() {
                app.update_actions();
            }
        });
        view.connect_load_failed(|_, _, uri, error| {
            // A navigation that replaces another one is cancelled; that is not a failure.
            if !error.matches(webkit::NetworkError::Cancelled) {
                let shown = url::Url::parse(uri)
                    .map(|mut u| {
                        u.set_query(None);
                        u.set_fragment(None);
                        u.to_string()
                    })
                    .unwrap_or_default();
                log::warn!("load failed: {shown}: {}", error.message());
            }
            false
        });
        view.connect_web_process_terminated(|_, reason| log::warn!("web process terminated: {reason:?}"));
        let weak = Rc::downgrade(self);
        view.connect_title_notify(move |view| {
            if let Some(this) = weak.upgrade() {
                let title = view.title().filter(|t| !t.is_empty());
                this.window.set_title(Some(title.as_deref().unwrap_or(APP_NAME)));
            }
        });
        view.connect_context_menu(|_, menu, _| {
            // Navigation entries belong in the View menu, as on the Mac.
            for item in menu.items() {
                if matches!(
                    item.stock_action(),
                    webkit::ContextMenuAction::GoBack
                        | webkit::ContextMenuAction::GoForward
                        | webkit::ContextMenuAction::Stop
                        | webkit::ContextMenuAction::Reload
                        | webkit::ContextMenuAction::OpenLinkInNewWindow
                        | webkit::ContextMenuAction::DownloadLinkToDisk
                ) {
                    menu.remove(&item);
                }
            }
            false
        });

        let press = gtk::GestureClick::builder().button(0).propagation_phase(gtk::PropagationPhase::Capture).build();
        let weak = Rc::downgrade(self);
        press.connect_pressed(move |gesture, _, x, y| {
            if let Some(this) = weak.upgrade() {
                *this.last_press.borrow_mut() = Some(Press {
                    button: gesture.current_button(),
                    device: gesture.device(),
                    x,
                    y,
                    time: gesture.current_event_time(),
                });
            }
        });
        view.add_controller(press);
    }

    fn user_scripts(&self, app: &App) -> Vec<String> {
        let mut sources = vec![
            scripts::SHELL_MARKERS.to_string(),
            scripts::auxiliary_window(self.auxiliary),
            scripts::window_metrics(0, 0, false, app.zoom()),
            scripts::notification_permission(&app.settings.str("notifications")),
            scripts::APP_STATE.to_string(),
            scripts::NAVIGATION_STATE.to_string(),
            scripts::EXTERNAL_WINDOW_OPEN.to_string(),
            scripts::OPEN_IN_NEW_WINDOW.to_string(),
            scripts::CLIPBOARD.to_string(),
            scripts::NO_WEBRTC.to_string(),
        ];
        if cfg!(feature = "huddles") {
            sources.push(scripts::HUDDLE.to_string());
        }
        if app.runner.running() {
            sources.push(scripts::runner_state(Some(&crate::runner::Runner::runner_id(&app.settings))));
        }
        sources
    }

    /// Builds the user scripts from the current settings; they apply from the next page load.
    pub fn reload_user_scripts(&self) {
        let Some(app) = self.app() else { return };
        self.manager.remove_all_scripts();
        for source in self.user_scripts(&app) {
            self.manager.add_script(&webkit::UserScript::new(
                &source,
                webkit::UserContentInjectedFrames::TopFrame,
                webkit::UserScriptInjectionTime::Start,
                &allow_list(),
                &[],
            ));
        }
    }

    fn background_rgba(&self) -> gdk::RGBA {
        let palette = self.app().and_then(|a| a.host_palette());
        let color = match palette {
            Some(p) => p.window_background,
            None if adw::StyleManager::default().is_dark() => "#0b0d0b".into(),
            None => "#fafaf8".into(),
        };
        gdk::RGBA::parse(&color).unwrap_or(gdk::RGBA::BLACK)
    }

    /// Swaps in the desktop theme's page colors, or removes them.
    pub fn apply_host_css(&self, view_ready: bool) {
        if let Some(sheet) = self.theme_sheet.borrow_mut().take() {
            self.manager.remove_style_sheet(&sheet);
        }
        if let Some(css) = self.app().and_then(|a| a.host_css()) {
            let sheet = webkit::UserStyleSheet::new(
                &css,
                webkit::UserContentInjectedFrames::TopFrame,
                webkit::UserStyleLevel::User,
                &allow_list(),
                &[],
            );
            self.manager.add_style_sheet(&sheet);
            *self.theme_sheet.borrow_mut() = Some(sheet);
        }
        if view_ready {
            self.webview.set_background_color(&self.background_rgba());
        }
    }

    pub fn load_path(&self, path: &str) {
        let path = match path {
            "/" => "/__native/start",
            p if p.starts_with('/') => p,
            _ => "/__native/start",
        };
        self.webview.load_uri(&format!("{}{path}", *APP_URL));
    }

    pub fn run_js(&self, source: &str) {
        self.run_js_then(source, |_| {});
    }

    /// Runs a script in the page; `callback` gets its result as JSON, or None when it failed.
    pub fn run_js_then(&self, source: &str, callback: impl FnOnce(Option<Value>) + 'static) {
        self.webview.evaluate_javascript(source, None, None, gtk::gio::Cancellable::NONE, move |result| match result {
            Ok(value) => {
                let json = value.to_json(0).map(|j| j.to_string()).unwrap_or_else(|| "null".into());
                callback(serde_json::from_str(&json).ok())
            }
            Err(error) => {
                // The message names the failure, not the script, which can carry tokens.
                log::warn!("script failed: {}", error.message());
                callback(None)
            }
        });
    }

    /// Dispatches one of the page's app commands; `fallback` runs when the page did not handle it.
    pub fn app_command(&self, command: &str, fields: Value, fallback: Option<Box<dyn FnOnce()>>) {
        self.run_js_then(&scripts::app_command(command, fields), move |result| {
            if result != Some(Value::Bool(true))
                && let Some(fallback) = fallback
            {
                fallback();
            }
        });
    }

    /// Navigates inside the single-page app, or loads the page when the app is not ready.
    pub fn navigate(self: &Rc<Self>, path: &str) {
        let weak = Rc::downgrade(self);
        let target = path.to_string();
        self.app_command(
            "navigate",
            json!({"url": path, "replaceState": false}),
            Some(Box::new(move || {
                if let Some(this) = weak.upgrade() {
                    this.load_path(&target);
                }
            })),
        );
    }

    // ---- policy ----------------------------------------------------------------------------

    fn decide_policy(self: &Rc<Self>, decision: &webkit::PolicyDecision, kind: webkit::PolicyDecisionType) -> bool {
        let Some(app) = self.app() else { return false };
        if kind == webkit::PolicyDecisionType::Response {
            if let Some(response) = decision.downcast_ref::<webkit::ResponsePolicyDecision>()
                && !response.is_mime_type_supported()
            {
                response.download();
                return true;
            }
            return false;
        }
        let Some(navigation) = decision.downcast_ref::<webkit::NavigationPolicyDecision>() else { return false };
        let Some(action) = navigation.navigation_action() else { return false };
        let uri = action.request().and_then(|r| r.uri()).map(|u| u.to_string()).unwrap_or_default();
        if kind == webkit::PolicyDecisionType::NewWindowAction {
            decision.ignore();
            // A link the user opens in a new window stays in Amp. A window the page opens from
            // script, such as "Open in Browser" for passkeys, which WebKitGTK cannot do, goes to
            // the browser.
            if is_app_url(&uri) && action.navigation_type() == webkit::NavigationType::LinkClicked {
                app.open_window(&url_path(&uri), None);
            } else {
                app.open_in_browser(&uri);
            }
            return true;
        }
        if action.frame_name().is_some_and(|f| !f.is_empty()) {
            return false;
        }
        if signin::is_sign_in_page(&uri) {
            decision.ignore();
            app.sign_in.start(&app, self);
            return true;
        }
        if uri.starts_with("about:") || uri.starts_with("data:") || uri.starts_with("blob:") || is_app_url(&uri) {
            return false;
        }
        decision.ignore();
        app.open_in_browser(&uri);
        true
    }

    fn create_web_view(&self, action: &webkit::NavigationAction) {
        let Some(app) = self.app() else { return };
        let action = action.clone();
        let uri = action.request().and_then(|r| r.uri()).map(|u| u.to_string()).unwrap_or_default();
        if is_app_url(&uri) && action.navigation_type() == webkit::NavigationType::LinkClicked {
            app.open_window(&url_path(&uri), None);
        } else {
            app.open_in_browser(&uri);
        }
    }

    // ---- window behaviour ------------------------------------------------------------------

    pub fn begin_move(&self) {
        let press = self.last_press.borrow();
        let Some(press) = press.as_ref() else { return };
        let Some(native) = self.webview.native() else { return };
        #[allow(deprecated)] // compute_point needs GTK 4.12.
        let Some((x, y)) = self.webview.translate_coordinates(&native, press.x, press.y) else { return };
        if let (Some(toplevel), Some(device)) =
            (native.surface().and_then(|s| s.downcast::<gdk::Toplevel>().ok()), press.device.as_ref())
        {
            let button = if press.button == 0 { gdk::BUTTON_PRIMARY as i32 } else { press.button as i32 };
            toplevel.begin_move(device, button, x, y, press.time);
        }
    }

    pub fn toggle_maximized(&self) {
        if self.window.is_maximized() {
            self.window.unmaximize();
        } else {
            self.window.maximize();
        }
    }

    pub fn toggle_full_screen(&self) {
        if self.window.is_fullscreen() {
            self.window.unfullscreen();
        } else {
            self.window.fullscreen();
        }
    }

    fn push_metrics(&self) {
        let (left, right) = self.controls.insets();
        self.run_js(&scripts::window_metrics(left, right, self.window.is_fullscreen(), self.webview.zoom_level()));
        self.menu_revealer.set_margin_start(left);
    }

    pub fn set_zoom(&self, factor: f64) {
        self.webview.set_zoom_level(factor);
        self.push_metrics();
    }

    pub fn publish_host_visibility(&self) {
        self.visibility_sequence.set(self.visibility_sequence.get() + 1);
        self.run_js(&scripts::host_visibility(self.visibility_sequence.get(), self.window.is_active()));
    }

    fn close_request(self: &Rc<Self>) {
        let Some(app) = self.app() else { return };
        if self.auxiliary.is_none() && app.main_window().is_some_and(|m| Rc::ptr_eq(&m, self)) {
            let (default_width, default_height) = self.window.default_size();
            let width = Some(self.window.width()).filter(|w| *w != 0).unwrap_or(default_width);
            let height = Some(self.window.height()).filter(|h| *h != 0).unwrap_or(default_height);
            app.settings
                .set("window", json!({"width": width, "height": height, "maximized": self.window.is_maximized()}));
        }
    }

    // ---- keys ------------------------------------------------------------------------------

    fn install_shortcuts(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, keyval, keycode, state| {
            let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
            this.alt_alone.set(
                matches!(keyval, gdk::Key::Alt_L | gdk::Key::Alt_R)
                    && (state & gtk::accelerator_get_default_mod_mask()).is_empty(),
            );
            if keyval == gdk::Key::Escape && this.menu_revealer.reveals_child() {
                this.menu_revealer.set_reveal_child(false);
                return glib::Propagation::Stop;
            }
            match this.app() {
                Some(app) if app.handle_shortcut(&this, keyval, keycode, state) => glib::Propagation::Stop,
                _ => glib::Propagation::Proceed,
            }
        });
        let weak = Rc::downgrade(self);
        keys.connect_key_released(move |_, keyval, _, _| {
            let Some(this) = weak.upgrade() else { return };
            let lower = keyval.to_lower();
            let handlers: Vec<ReleaseHandler> = this.release_handlers.borrow().values().cloned().collect();
            for (key, callback) in handlers {
                if key == lower || matches!(keyval, gdk::Key::Control_L | gdk::Key::Control_R) {
                    callback();
                }
            }
            if matches!(keyval, gdk::Key::Alt_L | gdk::Key::Alt_R) && this.alt_alone.get() {
                this.menu_revealer.set_reveal_child(!this.menu_revealer.reveals_child());
            }
            this.alt_alone.set(false);
        });
        self.window.add_controller(keys);
    }

    pub fn show_toast(&self, title: &str) {
        self.toasts.add_toast(adw::Toast::builder().title(glib::markup_escape_text(title)).timeout(2).build());
    }

    /// Calls `callback` when `keyval` or Ctrl is released; returns an ID for
    /// [`Self::disconnect_key_release`].
    pub fn on_key_release(&self, keyval: gdk::Key, callback: impl Fn() + 'static) -> u64 {
        let id = self.next_release_handler.get();
        self.next_release_handler.set(id + 1);
        self.release_handlers.borrow_mut().insert(id, (keyval, Rc::new(callback)));
        id
    }

    pub fn disconnect_key_release(&self, id: u64) {
        self.release_handlers.borrow_mut().remove(&id);
    }
}

/// The path of an app URL, for opening it in a new window.
fn url_path(uri: &str) -> String {
    url::Url::parse(uri).map(|u| u.path().to_string()).ok().filter(|p| !p.is_empty()).unwrap_or_else(|| "/".into())
}

//! App Settings: a libadwaita preferences window with the Mac app's six sections.

use std::cell::RefCell;
use std::path::Path;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use amp_common::config::{self, Settings, truthy};
use gtk::{gdk, glib};
use serde_json::{Value, json};

use crate::app::App;
use crate::menus::{GLOBAL_SHORTCUTS, accel_label};
use crate::runner::{Runner, find_cli};

const TABS: [(&str, &str, &str); 6] = [
    ("general", "General", "emblem-system-symbolic"),
    ("account", "Account", "avatar-default-symbolic"),
    ("runner", "Runner", "computer-symbolic"),
    ("notifications", "Notifications", "preferences-system-notifications-symbolic"),
    ("updates", "Updates", "software-update-available-symbolic"),
    ("about", "About", "help-about-symbolic"),
];
const ICON_CHOICES: [(&str, &str); 4] = [("dark", "Dark"), ("oled", "OLED"), ("light", "Light"), ("puck", "Puck")];
const GIT_COMMIT: &str = env!("AMP_GIT_COMMIT");

fn switch_row(title: &str, subtitle: Option<&str>, active: bool, on_toggle: impl Fn(bool) + 'static) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).subtitle(subtitle.unwrap_or("")).build();
    let switch = gtk::Switch::builder().active(active).valign(gtk::Align::Center).build();
    switch.connect_active_notify(move |s| on_toggle(s.is_active()));
    row.add_suffix(&switch);
    row.set_activatable_widget(Some(&switch));
    row
}

fn combo_row<T: Clone + PartialEq + 'static>(
    title: &str,
    options: Vec<(T, String)>,
    selected: &T,
    on_select: impl Fn(T) + 'static,
) -> adw::ComboRow {
    let labels: Vec<&str> = options.iter().map(|(_, l)| l.as_str()).collect();
    let row = adw::ComboRow::builder().title(title).model(&gtk::StringList::new(&labels)).build();
    let values: Vec<T> = options.into_iter().map(|(v, _)| v).collect();
    row.set_selected(values.iter().position(|v| v == selected).unwrap_or(0) as u32);
    row.connect_selected_notify(move |r| {
        if let Some(value) = values.get(r.selected() as usize) {
            on_select(value.clone());
        }
    });
    row
}

fn group(title: Option<&str>, description: Option<&str>) -> adw::PreferencesGroup {
    let g = adw::PreferencesGroup::new();
    if let Some(title) = title {
        g.set_title(title);
    }
    if let Some(description) = description {
        g.set_description(Some(description));
    }
    g
}

/// Plain widgets go in a group of their own, as preferences pages hold only groups.
fn wrap(widget: &impl IsA<gtk::Widget>) -> adw::PreferencesGroup {
    let g = adw::PreferencesGroup::new();
    g.add(widget);
    g
}

fn icon(name: &str) -> gtk::Image {
    gtk::Image::from_icon_name(name)
}

pub struct SettingsWindow {
    pub window: adw::PreferencesWindow,
    app: Weak<App>,
    pages: RefCell<Vec<(String, adw::PreferencesPage)>>,
    account_group: adw::PreferencesGroup,
    account_rows: RefCell<Vec<gtk::Widget>>,
    runner_switch: gtk::Switch,
    runner_status: adw::ActionRow,
    network_group: adw::PreferencesGroup,
    discover_row: adw::ActionRow,
    new_dirs_row: adw::ActionRow,
    dirs_group: adw::PreferencesGroup,
    dir_rows: RefCell<Vec<gtk::Widget>>,
    notification_status: gtk::Label,
    notification_button: gtk::Button,
    /// The About tab's app icon, which follows the App Icon choice.
    about_icon: gtk::Picture,
    #[allow(deprecated)]
    chooser: RefCell<Option<gtk::FileChooserNative>>,
}

impl SettingsWindow {
    pub fn new(app: &Rc<App>) -> Rc<Self> {
        let window =
            adw::PreferencesWindow::builder().application(&app.adw).title("Settings").search_enabled(false).build();
        window.set_default_size(860, 680);
        let this = Rc::new(Self {
            window,
            app: Rc::downgrade(app),
            pages: RefCell::default(),
            account_group: group(None, None),
            account_rows: RefCell::default(),
            runner_switch: gtk::Switch::builder()
                .active(app.settings.bool("runner_enabled"))
                .valign(gtk::Align::Center)
                .build(),
            runner_status: adw::ActionRow::builder().title("Use This Computer as a Runner").build(),
            network_group: group(None, None),
            discover_row: adw::ActionRow::builder().title("All Projects Under Directories").build(),
            new_dirs_row: adw::ActionRow::builder().title("New Projects Folder").build(),
            dirs_group: group(
                Some("Directories"),
                Some("Add a project’s Git checkout. Threads start in the directory you add."),
            ),
            dir_rows: RefCell::default(),
            notification_status: gtk::Label::builder().valign(gtk::Align::Center).build(),
            notification_button: gtk::Button::builder()
                .label("Enable Notifications")
                .valign(gtk::Align::Center)
                .build(),
            about_icon: gtk::Picture::builder().can_shrink(true).build(),
            chooser: RefCell::default(),
        });
        for (key, label, icon_name) in TABS {
            let groups = match key {
                "general" => this.build_general(app),
                "account" => vec![this.account_group.clone()],
                "runner" => this.build_runner(app),
                "notifications" => this.build_notifications(app),
                "updates" => this.build_updates(),
                _ => this.build_about(app),
            };
            let page = adw::PreferencesPage::builder().title(label).icon_name(icon_name).name(key).build();
            for g in groups {
                page.add(&g);
            }
            this.window.add(&page);
            this.pages.borrow_mut().push((key.to_string(), page));
        }
        let weak = Rc::downgrade(&this);
        this.window.connect_visible_page_notify(move |_| {
            if let Some(this) = weak.upgrade() {
                this.page_changed();
            }
        });
        this
    }

    fn settings(&self) -> Option<Rc<App>> {
        self.app.upgrade()
    }

    pub fn show_tab(&self, key: &str) {
        let page = self.pages.borrow().iter().find(|(k, _)| k == key).map(|(_, p)| p.clone());
        if let Some(page) = page {
            self.window.set_visible_page(&page);
        }
        self.page_changed();
        self.window.present();
    }

    fn page_changed(&self) {
        let key = self.window.visible_page().and_then(|p| p.name());
        match key.as_deref() {
            Some("account") => self.refresh_account(),
            Some("runner") => self.refresh_runner(),
            _ => {}
        }
    }

    // ---- General ---------------------------------------------------------------------------

    fn build_general(&self, app: &Rc<App>) -> Vec<adw::PreferencesGroup> {
        let s = &app.settings;
        let g = group(None, None);
        let weak = Rc::downgrade(app);
        let with_app = move |f: fn(&Rc<App>, bool)| {
            let weak = weak.clone();
            move |v: bool| {
                if let Some(app) = weak.upgrade() {
                    f(&app, v);
                }
            }
        };
        g.add(&switch_row(
            "Show Amp in Tray",
            Some("Recent threads, New Thread, and Puck from the tray of any desktop that has one."),
            s.bool("show_in_tray"),
            with_app(|app, v| {
                app.settings.set("show_in_tray", json!(v));
                app.tray.set_visible(v);
            }),
        ));
        g.add(&switch_row(
            "Show Unread Count in Launcher",
            Some("The number of unread threads on Amp’s launcher icon."),
            s.bool("show_unread_count"),
            with_app(|app, v| {
                app.settings.set("show_unread_count", json!(v));
                app.refresh_badge();
            }),
        ));
        g.add(&switch_row(
            "Warn Before Quitting",
            Some("Hold Ctrl+Q to quit, so a stray press keeps Amp open."),
            s.bool("warn_before_quitting"),
            with_app(|app, v| app.settings.set("warn_before_quitting", json!(v))),
        ));
        g.add(&switch_row("Open at Login", None, s.bool("open_at_login"), with_app(|app, v| app.set_open_at_login(v))));
        let weak = Rc::downgrade(app);
        let appearances =
            vec![("system", "System".to_string()), ("light", "Light".to_string()), ("dark", "Dark".to_string())];
        let current = s.str("appearance");
        let current = appearances.iter().map(|(v, _)| *v).find(|v| *v == current).unwrap_or("system");
        g.add(&combo_row("Appearance", appearances, &current, move |v| {
            if let Some(app) = weak.upgrade() {
                app.set_appearance(v);
            }
        }));
        let zooms: Vec<(u32, String)> = config::ZOOM_FACTORS
            .iter()
            .enumerate()
            .map(|(i, f)| (i as u32, format!("{}%", (f * 100.0).round())))
            .collect();
        let zoom_index = config::ZOOM_FACTORS.iter().position(|f| *f == app.zoom()).unwrap_or(4) as u32;
        let weak = Rc::downgrade(app);
        g.add(&combo_row("Zoom", zooms, &zoom_index, move |i| {
            if let Some(app) = weak.upgrade() {
                app.set_zoom(config::ZOOM_FACTORS[i as usize]);
            }
        }));
        let mut groups = vec![g];

        let icons: Vec<(&str, &str, std::path::PathBuf)> =
            ICON_CHOICES.iter().filter_map(|(k, l)| config::icon_file(k).map(|p| (*k, *l, p))).collect();
        if !icons.is_empty() {
            let g2 = group(Some("App Icon"), Some("Shows in the launcher and app switcher while Amp is open."));
            let flow = gtk::FlowBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .max_children_per_line(4)
                .min_children_per_line(4)
                .homogeneous(true)
                .halign(gtk::Align::Center)
                .build();
            flow.add_css_class("amp-icon-picker");
            let mut first: Option<gtk::ToggleButton> = None;
            let chosen = s.str("app_icon");
            for (key, label, path) in icons {
                let button = gtk::ToggleButton::new();
                button.add_css_class("amp-icon-choice");
                let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
                let picture = gtk::Picture::for_filename(&path);
                picture.set_size_request(88, 88);
                picture.set_can_shrink(true);
                column.append(&picture);
                column.append(&gtk::Label::new(Some(label)));
                button.set_child(Some(&column));
                match &first {
                    Some(first) => button.set_group(Some(first)),
                    None => first = Some(button.clone()),
                }
                button.set_active(chosen == key);
                let weak = Rc::downgrade(app);
                button.connect_toggled(move |b| {
                    if b.is_active()
                        && let Some(app) = weak.upgrade()
                    {
                        app.set_app_icon(key);
                    }
                });
                flow.append(&button);
            }
            let card = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            card.add_css_class("card");
            card.append(&flow);
            flow.set_hexpand(true);
            flow.set_margin_top(16);
            flow.set_margin_bottom(16);
            g2.add(&card);
            groups.push(g2);
        }
        groups.push(shortcuts_group());
        groups
    }

    // ---- Account ---------------------------------------------------------------------------

    pub fn refresh_account(&self) {
        let Some(app) = self.settings() else { return };
        for row in self.account_rows.borrow_mut().drain(..) {
            self.account_group.remove(&row);
        }
        let state = app.main_window().map(|w| w.app_state.borrow().clone()).unwrap_or(json!({}));
        let account = state.get("signedInAccount").filter(|a| truthy(a)).cloned();
        let mut rows: Vec<gtk::Widget> = Vec::new();
        let Some(account) = account else {
            let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
            card.add_css_class("card");
            let avatar = icon("avatar-default-symbolic");
            avatar.set_pixel_size(52);
            avatar.set_margin_top(30);
            avatar.add_css_class("dim-label");
            card.append(&avatar);
            let title = gtk::Label::builder().label("Not Signed In").margin_top(10).build();
            title.add_css_class("title-3");
            title.add_css_class("dim-label");
            card.append(&title);
            let hint = gtk::Label::new(Some("Sign in to Amp to see your account here."));
            hint.add_css_class("dim-label");
            card.append(&hint);
            let button = gtk::Button::builder()
                .label("Open Amp to Sign In")
                .halign(gtk::Align::Center)
                .margin_top(14)
                .margin_bottom(30)
                .build();
            button.add_css_class("suggested-action");
            let weak = self.app.clone();
            button.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.present_main();
                }
            });
            card.append(&button);
            self.account_group.add(&card);
            rows.push(card.upcast());
            *self.account_rows.borrow_mut() = rows;
            return;
        };
        let accounts = state
            .get("deviceAccounts")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
            .cloned()
            .unwrap_or_else(|| vec![account.clone()]);
        let text = |entry: &Value, key: &str| {
            entry.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from)
        };
        for entry in accounts {
            let current = entry.get("userID") == account.get("userID");
            let name = text(&entry, "username").or_else(|| text(&entry, "email"));
            let row = adw::ActionRow::builder()
                .title(name.clone().unwrap_or_default())
                .subtitle(text(&entry, "email").unwrap_or_default())
                .build();
            row.add_prefix(&adw::Avatar::new(32, Some(name.as_deref().unwrap_or("?")), true));
            if current {
                let check = icon("object-select-symbolic");
                check.add_css_class("accent");
                row.add_suffix(&check);
            } else {
                let button = gtk::Button::builder().label("Switch").valign(gtk::Align::Center).build();
                let weak = self.app.clone();
                let user_id = entry.get("userID").cloned().unwrap_or(Value::Null);
                button.connect_clicked(move |_| {
                    if let Some(main) = weak.upgrade().and_then(|a| a.main_window()) {
                        main.app_command("switchAccount", json!({"userID": user_id}), None);
                    }
                });
                row.add_suffix(&button);
            }
            self.account_group.add(&row);
            rows.push(row.upcast());
        }
        let add = adw::ActionRow::builder().title("Add Account…").activatable(true).build();
        add.add_prefix(&icon("list-add-symbolic"));
        let weak = self.app.clone();
        add.connect_activated(move |_| {
            if let Some(app) = weak.upgrade() {
                app.present_main();
                if let Some(main) = app.main_window() {
                    main.app_command("addAccount", json!({}), None);
                }
            }
        });
        self.account_group.add(&add);
        rows.push(add.upcast());
        *self.account_rows.borrow_mut() = rows;
    }

    // ---- Runner ----------------------------------------------------------------------------

    fn build_runner(self: &Rc<Self>, app: &Rc<App>) -> Vec<adw::PreferencesGroup> {
        let s = &app.settings;
        let g = group(None, None);
        let computer = icon("computer-symbolic");
        computer.set_pixel_size(32);
        self.runner_status.add_prefix(&computer);
        let weak = Rc::downgrade(self);
        self.runner_switch.connect_active_notify(move |sw| {
            if let Some(this) = weak.upgrade() {
                this.runner_setting("runner_enabled", json!(sw.is_active()));
            }
        });
        self.runner_status.add_suffix(&self.runner_switch);
        self.runner_status.set_activatable_widget(Some(&self.runner_switch));
        g.add(&self.runner_status);

        let name =
            adw::EntryRow::builder().title("Runner Name").text(s.str("runner_name")).show_apply_button(true).build();
        let weak = Rc::downgrade(self);
        name.connect_apply(move |row| {
            let Some(this) = weak.upgrade() else { return };
            let Some(app) = this.settings() else { return };
            let text = row.text().trim().to_string();
            let value = if text.is_empty() { app.settings.str("runner_name") } else { text };
            this.runner_setting("runner_name", json!(value));
        });
        g.add(&name);
        let setting = |key: &'static str| {
            let weak = Rc::downgrade(self);
            move |v: bool| {
                if let Some(this) = weak.upgrade() {
                    this.runner_setting(key, json!(v));
                }
            }
        };
        g.add(&switch_row(
            "Share This Computer’s Display",
            Some("Threads running here can show and control this computer’s screen in their Desktop tab."),
            s.bool("runner_desktop"),
            setting("runner_desktop"),
        ));
        g.add(&switch_row(
            "Allow Terminal Access",
            Some("Threads running here can open a shell on this computer in their Terminal tab."),
            s.bool("runner_terminal"),
            setting("runner_terminal"),
        ));
        g.add(&switch_row(
            "Keep This Computer Awake",
            Some("Prevent sleep while the runner is on."),
            s.bool("runner_keep_awake"),
            setting("runner_keep_awake"),
        ));

        let place = group(Some("Where Threads Can Run"), None);
        place.add(&switch_row_with_prefix(
            "Home Directory",
            "~",
            "user-home-symbolic",
            s.bool("runner_home"),
            setting("runner_home"),
        ));
        self.discover_row.add_prefix(&icon("folder-saved-search-symbolic"));
        let choose = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
        let weak = Rc::downgrade(self);
        choose.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.choose_folder("runner_discover_dirs");
            }
        });
        let clear = gtk::Button::builder().icon_name("edit-clear-symbolic").valign(gtk::Align::Center).build();
        clear.add_css_class("flat");
        clear.set_tooltip_text(Some("Stop Looking"));
        let weak = Rc::downgrade(self);
        clear.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.runner_setting("runner_discover_dirs", json!([]));
                this.refresh_runner_paths();
            }
        });
        self.discover_row.add_suffix(&clear);
        self.discover_row.add_suffix(&choose);
        place.add(&self.discover_row);
        self.new_dirs_row.add_prefix(&icon("folder-new-symbolic"));
        let new_choose = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
        let weak = Rc::downgrade(self);
        new_choose.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.choose_folder("runner_new_dirs");
            }
        });
        self.new_dirs_row.add_suffix(&new_choose);
        place.add(&self.new_dirs_row);

        self.network_group.add(&switch_row(
            "Share This Computer’s Network",
            Some("Let your orbs browse the web through this computer’s connection."),
            truthy(&Runner::get(s, "runner_share_network")),
            setting("runner_share_network"),
        ));
        self.refresh_runner_paths();

        let add = gtk::Button::builder().icon_name("list-add-symbolic").valign(gtk::Align::Center).build();
        add.add_css_class("flat");
        add.set_tooltip_text(Some("Add Directory…"));
        let weak = Rc::downgrade(self);
        add.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.add_directory();
            }
        });
        self.dirs_group.set_header_suffix(Some(&add));
        self.refresh_dirs();
        vec![g, place, self.network_group.clone(), self.dirs_group.clone()]
    }

    fn runner_setting(&self, key: &str, value: Value) {
        let Some(app) = self.settings() else { return };
        if Runner::get(&app.settings, key) == value {
            return;
        }
        app.settings.set(key, value);
        if key == "runner_enabled" || app.settings.bool("runner_enabled") {
            app.runner.apply(&app);
        }
    }

    pub fn refresh_runner(&self) {
        let Some(app) = self.settings() else { return };
        let settings = &app.settings;
        let subtitle = if !settings.bool("runner_enabled") {
            if find_cli().is_none() {
                "Install the Amp CLI to use this computer as a runner.".to_string()
            } else {
                "Run threads from ampcode.com here with your Secrets & Env Vars.".to_string()
            }
        } else {
            let mut subtitle = app.runner.summary(settings);
            let missing = app.runner.missing_features(settings);
            if !missing.is_empty() {
                subtitle += &format!(" This Amp CLI does not support {}; update it in Updates.", missing.join(", "));
            }
            subtitle
        };
        self.runner_status.set_subtitle(&glib::markup_escape_text(&subtitle));
        let net = app.main_window().is_some_and(|m| m.app_state.borrow().get("ampNetEnabled").is_some_and(truthy));
        self.network_group.set_visible(net);
        self.refresh_runner_paths();
        if self.runner_switch.is_active() != settings.bool("runner_enabled") {
            self.runner_switch.set_active(settings.bool("runner_enabled"));
        }
    }

    fn refresh_runner_paths(&self) {
        let Some(app) = self.settings() else { return };
        let discover = config::strings(&Runner::get(&app.settings, "runner_discover_dirs"));
        let discover = if discover.is_empty() { "Off".to_string() } else { discover.join(", ") };
        self.discover_row.set_subtitle(&glib::markup_escape_text(&discover));
        let new_dirs = Runner::get(&app.settings, "runner_new_dirs");
        let new_dirs = new_dirs.as_str().filter(|d| !d.is_empty()).unwrap_or("Not set");
        self.new_dirs_row.set_subtitle(&glib::markup_escape_text(new_dirs));
    }

    #[allow(deprecated)] // gtk::FileDialog needs GTK 4.10 features the rest of the app avoids.
    fn folder_chooser(&self, title: &str, accept: &str) -> gtk::FileChooserNative {
        gtk::FileChooserNative::builder()
            .title(title)
            .transient_for(&self.window)
            .action(gtk::FileChooserAction::SelectFolder)
            .accept_label(accept)
            .build()
    }

    #[allow(deprecated)]
    fn choose_folder(self: &Rc<Self>, key: &'static str) {
        let dialog = self.folder_chooser("Choose Folder", "Choose");
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |d, response| {
            if let (Some(this), true) = (weak.upgrade(), response == gtk::ResponseType::Accept)
                && let Some(path) = d.file().and_then(|f| f.path())
                && let Some(app) = this.settings()
            {
                let path = path.to_string_lossy().to_string();
                if key == "runner_discover_dirs" {
                    let mut current = config::strings(&Runner::get(&app.settings, key));
                    if !current.contains(&path) {
                        current.push(path);
                    }
                    this.runner_setting(key, json!(current));
                } else {
                    this.runner_setting(key, json!(path));
                }
                this.refresh_runner_paths();
            }
            d.destroy();
        });
        dialog.show();
        *self.chooser.borrow_mut() = Some(dialog);
    }

    fn refresh_dirs(self: &Rc<Self>) {
        let Some(app) = self.settings() else { return };
        for row in self.dir_rows.borrow_mut().drain(..) {
            self.dirs_group.remove(&row);
        }
        let mut rows: Vec<gtk::Widget> = Vec::new();
        for directory in app.settings.strings("runner_dirs") {
            let name = Path::new(&directory).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&name))
                .subtitle(glib::markup_escape_text(&directory))
                .build();
            row.add_prefix(&icon("folder-symbolic"));
            let remove = gtk::Button::builder().icon_name("user-trash-symbolic").valign(gtk::Align::Center).build();
            remove.add_css_class("flat");
            let weak = Rc::downgrade(self);
            remove.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.remove_directory(&directory);
                }
            });
            row.add_suffix(&remove);
            self.dirs_group.add(&row);
            rows.push(row.upcast());
        }
        if rows.is_empty() {
            let row = adw::ActionRow::builder()
                .title("No directories yet")
                .subtitle("Add a folder with the + button.")
                .build();
            row.add_css_class("dim-label");
            self.dirs_group.add(&row);
            rows.push(row.upcast());
        }
        *self.dir_rows.borrow_mut() = rows;
    }

    #[allow(deprecated)]
    fn add_directory(self: &Rc<Self>) {
        let dialog = self.folder_chooser("Add Directory", "Add");
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |d, response| {
            if let (Some(this), true) = (weak.upgrade(), response == gtk::ResponseType::Accept)
                && let Some(path) = d.file().and_then(|f| f.path())
                && let Some(app) = this.settings()
            {
                let path = path.to_string_lossy().to_string();
                let mut dirs = app.settings.strings("runner_dirs");
                if !dirs.contains(&path) {
                    dirs.push(path.clone());
                    app.settings.set("runner_dirs", json!(dirs));
                    app.runner.dirs_command(&app, "add", &[path]);
                    this.refresh_dirs();
                }
            }
            d.destroy();
        });
        dialog.show();
        *self.chooser.borrow_mut() = Some(dialog);
    }

    fn remove_directory(self: &Rc<Self>, directory: &str) {
        let Some(app) = self.settings() else { return };
        let dirs: Vec<String> = app.settings.strings("runner_dirs").into_iter().filter(|d| d != directory).collect();
        app.settings.set("runner_dirs", json!(dirs));
        app.runner.dirs_command(&app, "remove", &[directory.to_string()]);
        self.refresh_dirs();
    }

    // ---- Notifications ---------------------------------------------------------------------

    fn build_notifications(&self, app: &Rc<App>) -> Vec<adw::PreferencesGroup> {
        let g = group(None, None);
        let row = adw::ActionRow::builder()
            .title("Notifications")
            .subtitle("Let Amp notify you when a thread needs you.")
            .build();
        let weak = Rc::downgrade(app);
        self.notification_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.enable_notifications();
            }
        });
        row.add_suffix(&self.notification_status);
        row.add_suffix(&self.notification_button);
        g.add(&row);
        let g2 = group(
            None,
            Some(
                "Notifications for the thread you are viewing are never shown. \
                 Your choices apply to every device signed in to your account.",
            ),
        );
        let choose = adw::ActionRow::builder()
            .title("Choose Notifications…")
            .subtitle("Pick which notifications Amp sends, for all your devices.")
            .activatable(true)
            .build();
        choose.add_suffix(&icon("go-next-symbolic"));
        let weak = Rc::downgrade(app);
        choose.connect_activated(move |_| {
            if let Some(app) = weak.upgrade() {
                app.present_main();
                if let Some(main) = app.main_window() {
                    main.app_command("openSettings", json!({}), None);
                }
            }
        });
        g2.add(&choose);
        self.refresh_notifications_with(&app.settings);
        vec![g, g2]
    }

    pub fn refresh_notifications(&self) {
        if let Some(app) = self.settings() {
            self.refresh_notifications_with(&app.settings);
        }
    }

    fn refresh_notifications_with(&self, settings: &Settings) {
        let allowed = settings.str("notifications") == "authorized";
        self.notification_status.set_label(if allowed { "Allowed" } else { "Not Set Up" });
        self.notification_status.remove_css_class("success");
        self.notification_status.remove_css_class("warning");
        self.notification_status.add_css_class(if allowed { "success" } else { "warning" });
        self.notification_button.set_visible(!allowed);
    }

    // ---- Updates ---------------------------------------------------------------------------

    fn build_updates(&self) -> Vec<adw::PreferencesGroup> {
        let g = group(None, None);
        let web = adw::ActionRow::builder()
            .title("Amp Web App")
            .subtitle("Loaded from ampcode.com, so it is always the latest version.")
            .build();
        let current = gtk::Label::builder().label("Up to Date").valign(gtk::Align::Center).build();
        current.add_css_class("success");
        web.add_suffix(&current);
        g.add(&web);
        g.add(
            &adw::ActionRow::builder()
                .title("Linux Shell")
                .subtitle(format!("Version {}. Update it with your package or the install script.", config::VERSION))
                .build(),
        );
        let g2 = group(None, None);
        let cli = adw::ActionRow::builder().title("Amp CLI").subtitle(glib::markup_escape_text(&cli_version())).build();
        let button = gtk::Button::builder().label("Update CLI").valign(gtk::Align::Center).build();
        button.set_sensitive(find_cli().is_some());
        let row = cli.clone();
        button.connect_clicked(move |button| update_cli(button, &row));
        cli.add_suffix(&button);
        g2.add(&cli);
        vec![g, g2]
    }

    // ---- About -----------------------------------------------------------------------------

    pub fn refresh_app_icon(&self, key: &str) {
        let path = config::icon_file(key).or_else(|| config::icon_file("app"));
        self.about_icon.set_filename(path.as_ref());
        self.about_icon.set_visible(path.is_some());
    }

    fn build_about(&self, app: &Rc<App>) -> Vec<adw::PreferencesGroup> {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("card");
        let top =
            gtk::Box::builder().spacing(36).margin_top(36).margin_start(44).margin_end(44).margin_bottom(24).build();
        self.about_icon.set_size_request(104, 104);
        self.refresh_app_icon(&app.settings.str("app_icon"));
        top.append(&self.about_icon);
        let info =
            gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).valign(gtk::Align::Center).build();
        let wordmark = gtk::Label::builder().label("amp").xalign(0.0).build();
        wordmark.add_css_class("amp-wordmark");
        info.append(&wordmark);
        let version = gtk::Label::builder().label(format!("{} (Linux)", config::VERSION)).xalign(0.0).build();
        version.add_css_class("title-3");
        version.add_css_class("dim-label");
        info.append(&version);
        let webkit = crate::bridge::webkit_version();
        let built = gtk::Label::builder().label(format!("WebKitGTK {webkit} · {GIT_COMMIT}")).xalign(0.0).build();
        built.add_css_class("dim-label");
        info.append(&built);
        let copy = gtk::Button::builder().label("Copy Build Info").halign(gtk::Align::Start).margin_top(10).build();
        copy.connect_clicked(move |_| {
            if let Some(display) = gdk::Display::default() {
                display
                    .clipboard()
                    .set_text(&format!("Amp for Linux {} ({GIT_COMMIT}), WebKitGTK {webkit}", config::VERSION));
            }
        });
        info.append(&copy);
        let note = gtk::Label::builder()
            .label("Amp for Linux. The app loads ampcode.com, so features arrive without updates.")
            .xalign(0.0)
            .margin_top(10)
            .build();
        note.add_css_class("dim-label");
        note.add_css_class("caption");
        info.append(&note);
        top.append(&info);
        card.append(&top);
        card.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let buttons =
            gtk::Box::builder().spacing(8).margin_top(16).margin_bottom(16).margin_start(20).margin_end(20).build();
        for (label, url) in
            [("Release Notes", "https://ampcode.com/news"), ("Documentation", "https://ampcode.com/manual")]
        {
            let button = gtk::Button::with_label(label);
            let weak = Rc::downgrade(app);
            button.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.launch_uri(url);
                }
            });
            buttons.append(&button);
        }
        let site = gtk::Button::builder().label("Open ampcode.com").hexpand(true).halign(gtk::Align::End).build();
        let weak = Rc::downgrade(app);
        site.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.launch_uri("https://ampcode.com");
            }
        });
        buttons.append(&site);
        card.append(&buttons);
        vec![wrap(&card)]
    }
}

/// The Mac's Keyboard Shortcuts section. Wayland leaves global shortcuts to the compositor, so each
/// row names the command to bind and the Mac's key with Ctrl for ⌃.
fn shortcuts_group() -> adw::PreferencesGroup {
    let g = group(
        Some("Keyboard Shortcuts"),
        Some(
            "Bind these commands in your desktop’s keyboard settings to use them from any app. \
             In Hyprland: bind = CTRL ALT, N, exec, amp-linux-desktop --action new-thread",
        ),
    );
    for (title, action, accel) in GLOBAL_SHORTCUTS {
        let command = format!("amp-linux-desktop --action {action}");
        let row = adw::ActionRow::builder().title(*title).subtitle(&command).build();
        let key = gtk::Label::builder()
            .label(accel.and_then(accel_label).unwrap_or_else(|| "Off".into()))
            .valign(gtk::Align::Center)
            .build();
        key.add_css_class(if accel.is_some() { "amp-shortcut-key" } else { "dim-label" });
        row.add_suffix(&key);
        let copy = gtk::Button::builder().icon_name("edit-copy-symbolic").valign(gtk::Align::Center).build();
        copy.add_css_class("flat");
        copy.set_tooltip_text(Some("Copy Command"));
        copy.connect_clicked(move |button| {
            button.clipboard().set_text(&command);
        });
        row.add_suffix(&copy);
        g.add(&row);
    }
    g
}

fn switch_row_with_prefix(
    title: &str,
    subtitle: &str,
    icon_name: &str,
    active: bool,
    on_toggle: impl Fn(bool) + 'static,
) -> adw::ActionRow {
    let row = switch_row(title, Some(subtitle), active, on_toggle);
    row.add_prefix(&icon(icon_name));
    row
}

fn cli_version() -> String {
    let Some(cli) = find_cli() else { return "Not installed. Runners and thread imports need it.".into() };
    let output = std::process::Command::new(&cli).arg("--version").stdin(std::process::Stdio::null()).output();
    match output {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let first = text.lines().next().filter(|l| !l.is_empty()).unwrap_or("Installed").to_string();
            format!("{first} · {}", cli.display())
        }
        Err(_) => cli.display().to_string(),
    }
}

fn update_cli(button: &gtk::Button, row: &adw::ActionRow) {
    let Some(cli) = find_cli() else { return };
    button.set_sensitive(false);
    row.set_subtitle("Updating…");
    let child = std::process::Command::new(cli)
        .arg("update")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let (button, row) = (button.clone(), row.clone());
    let done = move || {
        row.set_subtitle(&glib::markup_escape_text(&cli_version()));
        button.set_sensitive(true);
    };
    match child {
        Ok(mut child) => {
            glib::timeout_add_local(std::time::Duration::from_millis(250), move || match child.try_wait() {
                Ok(None) => glib::ControlFlow::Continue,
                _ => {
                    done();
                    glib::ControlFlow::Break
                }
            });
        }
        Err(_) => done(),
    }
}

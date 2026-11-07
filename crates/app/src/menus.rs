//! The macOS app's menu bar (Amp, File, Edit, View, Window, Help) with ⌘ mapped to Ctrl, and its
//! shortcuts, which reach the app before the page as AppKit's key equivalents do.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::{gdk, gio, glib, prelude::*};

/// A menu entry: label, action, accelerator. `None` separates sections.
type Entry = Option<(&'static str, &'static str, Option<&'static str>)>;

/// Where the Window menu lists the open windows, as the Mac's Window menu does.
const WINDOW_LIST: &str = "app.present-window";

const MENUS: &[(&str, &[Entry])] = &[
    (
        // The Mac's application menu. Settings live here, as on the Mac, not under Edit.
        "Amp",
        &[
            Some(("About Amp", "app.about", None)),
            Some(("Check for Updates…", "app.check-updates", None)),
            None,
            Some(("Personal Settings", "win.personal-settings", Some("<Control>comma"))),
            Some(("Workspace Settings", "win.workspace-settings", Some("<Control><Alt>comma"))),
            Some(("App Settings…", "app.app-settings", Some("<Control><Shift>comma"))),
            None,
            Some(("Warn Before Quitting", "app.warn-before-quitting", None)),
            Some(("Quit Amp", "app.quit", Some("<Control>q"))),
        ],
    ),
    (
        "File",
        &[
            Some(("New Thread", "win.new-thread", None)),
            Some(("New Window", "app.new-window", Some("<Control><Shift>n"))),
            None,
            Some(("Print…", "win.print", Some("<Control>p"))),
            None,
            Some(("Close", "win.close", Some("<Control>w"))),
            Some(("Close Window", "win.close-window", Some("<Control><Shift>w"))),
        ],
    ),
    (
        "Edit",
        &[
            Some(("Undo", "win.undo", Some("<Control>z"))),
            Some(("Redo", "win.redo", Some("<Control><Shift>z"))),
            None,
            Some(("Cut", "win.cut", Some("<Control>x"))),
            Some(("Copy", "win.copy", Some("<Control>c"))),
            Some(("Paste", "win.paste", Some("<Control>v"))),
            Some(("Select All", "win.select-all", Some("<Control>a"))),
        ],
    ),
    (
        "View",
        &[
            Some(("Show Sidebar", "win.toggle-sidebar", Some("<Control>b"))),
            Some(("Show Puck", "win.toggle-puck", Some("<Control>slash"))),
            // The Mac's global ⌃⌥P; `--action puck-window` binds it desktop-wide.
            Some(("Puck Window", "app.puck-window", Some("<Control><Alt>p"))),
            None,
            Some(("Back", "win.back", Some("<Alt>Left"))),
            Some(("Forward", "win.forward", Some("<Alt>Right"))),
            Some(("Reload Portal", "win.reload-portal", Some("<Control>r"))),
            Some(("Reload Window", "win.reload", Some("<Control><Shift>r"))),
            None,
            Some(("Zoom In", "app.zoom-in", Some("<Control>equal"))),
            Some(("Zoom Out", "app.zoom-out", Some("<Control>minus"))),
            Some(("Reset Zoom", "app.zoom-reset", Some("<Control>0"))),
            Some(("Full Screen", "win.fullscreen", Some("F11"))),
            None,
            Some(("Developer Tools", "win.inspector", Some("<Control><Shift>i"))),
        ],
    ),
    (
        // The Mac's ⌘M would take the composer's Ctrl+M, so Minimize has no shortcut; tiling
        // (Fill, Center, Tile) is the compositor's on Linux.
        "Window",
        &[
            Some(("Minimize", "win.minimize", None)),
            Some(("Maximize", "win.zoom", None)),
            None,
            Some(("", WINDOW_LIST, None)),
            None,
            Some(("Bring All to Front", "app.bring-all", None)),
        ],
    ),
    (
        "Help",
        &[
            Some(("Keyboard Shortcuts", "win.keyboard-shortcuts", Some("<Control>question"))),
            Some(("Amp Manual", "app.docs", None)),
            Some(("Report Bug…", "win.report-bug", None)),
        ],
    ),
];

/// Items that only show while their action is enabled, as the Mac shows Close Window only next
/// to Close Tab.
const HIDDEN_WHEN_DISABLED: &[&str] = &["win.close-window"];

/// Shortcuts that also work without the menu, matching common Linux habits.
const EXTRA_ACCELS: &[(&str, &[&str])] = &[
    ("win.back", &["<Control>bracketleft"]),
    ("win.forward", &["<Control>bracketright"]),
    ("app.zoom-in", &["<Control>plus", "<Control>KP_Add"]),
    ("app.zoom-out", &["<Control>KP_Subtract"]),
];

/// Left to the web view, which already does them; the menu only names them.
const WEB_HANDLED: &[&str] = &["win.undo", "win.redo", "win.cut", "win.copy", "win.paste", "win.select-all"];

/// What an app action run from outside the window does.
#[derive(Clone, Copy)]
pub enum GlobalCommand {
    /// Shows Amp.
    Show,
    /// Shows Amp and runs this app command in the page.
    Page(&'static str),
    /// Opens Puck in its own window, or closes it when it has focus.
    PuckWindow,
}

/// App actions run from outside the window.
pub const GLOBAL_COMMANDS: &[(&str, GlobalCommand)] = &[
    ("show", GlobalCommand::Show),
    ("new-thread", GlobalCommand::Page("createThread")),
    ("dictate-new-thread", GlobalCommand::Page("dictateNewThread")),
    ("toggle-puck", GlobalCommand::Page("togglePuck")),
    ("puck-voice", GlobalCommand::Page("togglePuckRealtimeVoice")),
    ("puck-window", GlobalCommand::PuckWindow),
    ("dictate-to-puck", GlobalCommand::Page("openPuckWithDictation")),
];

/// The Mac's global hotkeys (Settings, Keyboard Shortcuts) as the desktop-wide actions a compositor
/// binds: title, action, suggested binding. The Mac's Open Amp Menu (⌃⌥A) becomes Show Amp, since
/// a tray menu cannot be opened from outside; the bindings are the Mac's with Ctrl for ⌃.
pub const GLOBAL_SHORTCUTS: &[(&str, &str, Option<&str>)] = &[
    ("Show Amp", "show", Some("<Control><Alt>a")),
    ("New Thread", "new-thread", Some("<Control><Alt>n")),
    ("Open Puck", "puck-window", Some("<Control><Alt>p")),
    ("Call Puck", "puck-voice", Some("<Control><Alt><Shift>p")),
    ("Dictate New Thread", "dictate-new-thread", None),
    ("Dictate to Puck", "dictate-to-puck", None),
];

/// A binding as GTK shows it, such as Ctrl+Alt+N.
pub fn accel_label(accel: &str) -> Option<String> {
    gtk::accelerator_parse(accel).map(|(key, mods)| gtk::accelerator_get_label(key, mods).to_string())
}

struct Label {
    section: gio::Menu,
    index: i32,
    label: String,
    accel: Option<&'static str>,
}

struct Shortcut {
    key: gdk::Key,
    mods: gdk::ModifierType,
    action: &'static str,
}

pub struct Menus {
    pub model: gio::Menu,
    shortcuts: Vec<Shortcut>,
    labels: RefCell<HashMap<&'static str, Label>>,
    /// The Window menu's section that lists the open windows.
    window_list: gio::Menu,
}

fn menu_item(label: &str, action: &str, accel: Option<&str>) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), Some(action));
    if let Some(accel) = accel {
        item.set_attribute_value("accel", Some(&accel.to_variant()));
    }
    if HIDDEN_WHEN_DISABLED.contains(&action) {
        item.set_attribute_value("hidden-when", Some(&"action-disabled".to_variant()));
    }
    item
}

impl Menus {
    pub fn new() -> Self {
        let model = gio::Menu::new();
        let mut shortcuts = Vec::new();
        let mut labels = HashMap::new();
        let window_list = gio::Menu::new();
        let mut add_shortcut = |accel: &str, action: &'static str| {
            if let Some((key, mods)) = gtk::accelerator_parse(accel) {
                shortcuts.push(Shortcut { key, mods, action });
            }
        };
        for (title, entries) in MENUS {
            let menu = gio::Menu::new();
            let mut section = gio::Menu::new();
            for entry in entries.iter() {
                let Some((label, action, accel)) = *entry else {
                    menu.append_section(None, &section);
                    section = gio::Menu::new();
                    continue;
                };
                if action == WINDOW_LIST {
                    section = window_list.clone();
                    continue;
                }
                let item = menu_item(label, action, accel);
                if let Some(accel) = accel
                    && !WEB_HANDLED.contains(&action)
                {
                    add_shortcut(accel, action);
                }
                let index = section.n_items();
                labels.insert(action, Label { section: section.clone(), index, label: label.to_string(), accel });
                section.append_item(&item);
            }
            menu.append_section(None, &section);
            model.append_submenu(Some(title), &menu);
        }
        for (action, accels) in EXTRA_ACCELS {
            for accel in accels.iter() {
                add_shortcut(accel, action);
            }
        }
        Self { model, shortcuts, labels: RefCell::new(labels), window_list }
    }

    /// Lists the open windows by title in the Window menu; choosing one shows it.
    pub fn set_windows(&self, titles: &[String]) {
        self.window_list.remove_all();
        for (index, title) in titles.iter().enumerate() {
            let item = gio::MenuItem::new(Some(title), None);
            item.set_action_and_target_value(Some(WINDOW_LIST), Some(&(index as u32).to_variant()));
            self.window_list.append_item(&item);
        }
    }

    /// Relabels a menu item, as the Mac app does for Show/Hide Sidebar and Close/Close Tab/Close
    /// Window.
    pub fn set_label(&self, action: &str, label: &str) {
        let mut labels = self.labels.borrow_mut();
        let Some(entry) = labels.get_mut(action) else { return };
        if entry.label == label {
            return;
        }
        let item = menu_item(label, action, entry.accel);
        entry.section.remove(entry.index);
        entry.section.insert_item(entry.index, &item);
        entry.label = label.to_string();
    }

    /// The menu action whose shortcut this key press is, if any.
    ///
    /// Matches the physical key as well as the keyval, so Ctrl+Shift+comma still matches when
    /// Shift turns the comma into "<".
    pub fn shortcut_for(
        &self,
        display: &gdk::Display,
        keyval: gdk::Key,
        keycode: u32,
        state: gdk::ModifierType,
        consumed: gdk::ModifierType,
    ) -> Option<&'static str> {
        let mods = state & gtk::accelerator_get_default_mod_mask();
        for shortcut in &self.shortcuts {
            // Ignore Shift for a symbol only when the keymap used it to produce that symbol.
            let symbol = keyval == shortcut.key
                && consumed.contains(gdk::ModifierType::SHIFT_MASK)
                && keyval.into_glib() < 0x100
                && !char::from_u32(keyval.into_glib()).is_some_and(char::is_alphabetic);
            let wanted = if symbol { mods - gdk::ModifierType::SHIFT_MASK } else { mods };
            if shortcut.mods != wanted {
                continue;
            }
            let same_key = keyval.to_lower() == shortcut.key.to_lower()
                || display.map_keyval(shortcut.key).is_some_and(|keys| keys.iter().any(|k| k.keycode() == keycode));
            if same_key {
                return Some(shortcut.action);
            }
        }
        None
    }

    /// The desktop-wide shortcuts, which the page lists under "From any app" in its Keymap, as the
    /// Mac sends its global hotkeys.
    pub fn shortcut_list() -> Vec<serde_json::Value> {
        GLOBAL_SHORTCUTS
            .iter()
            .map(|(title, _, accel)| serde_json::json!({"title": title, "shortcut": accel.and_then(accel_label)}))
            .collect()
    }
}

use glib::translate::IntoGlib;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_shortcuts_preserve_unconsumed_shift() {
        gtk::init().expect("run with a display, e.g. agent-desktop exec SESSION -- cargo test");
        let display = gdk::Display::default().expect("display");
        let menus = Menus::new();
        let ctrl = gdk::ModifierType::CONTROL_MASK;
        let shift = gdk::ModifierType::SHIFT_MASK;
        let alt = gdk::ModifierType::ALT_MASK;
        let none = gdk::ModifierType::empty();
        for (key, mods, consumed, action) in [
            (gdk::Key::comma, ctrl, none, Some("win.personal-settings")),
            (gdk::Key::comma, ctrl | alt, none, Some("win.workspace-settings")),
            (gdk::Key::comma, ctrl | shift, none, Some("app.app-settings")),
            (gdk::Key::comma, ctrl | alt | shift, none, None),
            (gdk::Key::question, ctrl | shift, shift, Some("win.keyboard-shortcuts")),
            (gdk::Key::plus, ctrl | shift, shift, Some("app.zoom-in")),
        ] {
            assert_eq!(menus.shortcut_for(&display, key, 0, mods, consumed), action, "{key:?} {mods:?}");
        }
        // A real US keymap produces '<' on the physical comma key with Shift held.
        let comma = display.map_keyval(gdk::Key::comma).expect("comma in keymap")[0].keycode();
        assert_eq!(menus.shortcut_for(&display, gdk::Key::less, comma, ctrl | shift, shift), Some("app.app-settings"));
    }
}

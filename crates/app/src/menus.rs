//! The macOS app's menu bar (Amp, File, Edit, View, Window, Help) with ⌘ mapped to Ctrl, and its
//! shortcuts, which reach the app before the page as AppKit's key equivalents do.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::{gdk, gio, glib, prelude::*};

/// A menu entry: label, action, accelerator. `None` separates sections.
type Entry = Option<(&'static str, &'static str, Option<&'static str>)>;

pub const MENUS: &[(&str, &[Entry])] = &[
    (
        "File",
        &[
            Some(("New Thread", "win.new-thread", None)),
            Some(("New Window", "app.new-window", Some("<Control><Shift>n"))),
            None,
            Some(("Print…", "win.print", Some("<Control>p"))),
            None,
            Some(("Close", "win.close", Some("<Control>w"))),
            Some(("Quit", "app.quit", Some("<Control>q"))),
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
            None,
            Some(("Personal Settings", "win.personal-settings", Some("<Control>comma"))),
            Some(("Workspace Settings", "win.workspace-settings", Some("<Control><Alt>comma"))),
            Some(("Preferences", "app.app-settings", Some("<Control><Shift>comma"))),
        ],
    ),
    (
        "View",
        &[
            Some(("Show Sidebar", "win.toggle-sidebar", Some("<Control>b"))),
            Some(("Show Puck", "win.toggle-puck", Some("<Control>slash"))),
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
        "Help",
        &[
            Some(("Keyboard Shortcuts", "win.keyboard-shortcuts", Some("<Control>question"))),
            Some(("Amp Manual", "app.docs", None)),
            Some(("Report Bug…", "win.report-bug", None)),
            None,
            Some(("About Amp", "app.about", None)),
        ],
    ),
];

/// Shortcuts that also work without the menu, matching common Linux habits.
const EXTRA_ACCELS: &[(&str, &[&str])] = &[
    ("win.back", &["<Control>bracketleft"]),
    ("win.forward", &["<Control>bracketright"]),
    ("app.zoom-in", &["<Control>plus", "<Control>KP_Add"]),
    ("app.zoom-out", &["<Control>KP_Subtract"]),
];

/// Left to the web view, which already does them; the menu only names them.
const WEB_HANDLED: &[&str] = &["win.undo", "win.redo", "win.cut", "win.copy", "win.paste", "win.select-all"];

/// App actions run from outside the window, mapped to the page's app commands (None only shows
/// Amp).
pub const GLOBAL_COMMANDS: &[(&str, Option<&str>)] = &[
    ("show", None),
    ("new-thread", Some("createThread")),
    ("dictate-new-thread", Some("dictateNewThread")),
    ("toggle-puck", Some("togglePuck")),
    ("puck-voice", Some("togglePuckRealtimeVoice")),
];

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
}

impl Menus {
    pub fn new() -> Self {
        let model = gio::Menu::new();
        let mut shortcuts = Vec::new();
        let mut labels = HashMap::new();
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
                let item = gio::MenuItem::new(Some(label), Some(action));
                if let Some(accel) = accel {
                    item.set_attribute_value("accel", Some(&accel.to_variant()));
                    if !WEB_HANDLED.contains(&action) {
                        add_shortcut(accel, action);
                    }
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
        Self { model, shortcuts, labels: RefCell::new(labels) }
    }

    /// Relabels a menu item, as the Mac app does for Show/Hide Sidebar and Close/Close Tab.
    pub fn set_label(&self, action: &str, label: &str) {
        let mut labels = self.labels.borrow_mut();
        let Some(entry) = labels.get_mut(action) else { return };
        if entry.label == label {
            return;
        }
        let item = gio::MenuItem::new(Some(label), Some(action));
        if let Some(accel) = entry.accel {
            item.set_attribute_value("accel", Some(&accel.to_variant()));
        }
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
    ) -> Option<&'static str> {
        let mods = state & gtk::accelerator_get_default_mod_mask();
        for shortcut in &self.shortcuts {
            // A shifted symbol such as "?" or "+" already includes Shift in its keyval.
            let symbol = keyval == shortcut.key
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

    /// The shortcuts the page lists in its keyboard shortcut settings.
    pub fn shortcut_list() -> Vec<serde_json::Value> {
        let mut shortcuts = Vec::new();
        for (_, entries) in MENUS {
            for (title, _, accel) in entries.iter().flatten() {
                if let Some(accel) = accel {
                    let label =
                        gtk::accelerator_parse(*accel).map(|(k, m)| gtk::accelerator_get_label(k, m).to_string());
                    shortcuts.push(serde_json::json!({"title": title, "shortcut": label}));
                }
            }
        }
        shortcuts
    }
}

use glib::translate::IntoGlib;

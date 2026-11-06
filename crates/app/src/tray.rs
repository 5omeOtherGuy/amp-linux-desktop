//! The status icon: the Mac app's menu bar extra, as a StatusNotifierItem.
//!
//! Waybar, KDE, and GNOME's AppIndicator extension show StatusNotifierItems. The menu lists the
//! recent threads the page sends through ampNativeMenuBar (version 2), with the unread ones marked,
//! plus New Thread, Puck, Preferences, and Quit. Without a StatusNotifierWatcher on the bus the
//! icon stays unregistered and nothing else changes.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use amp_common::config::APP_ID;
use gtk::{gio, glib, prelude::*};
use serde_json::Value;

use crate::app::App;

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const MAX_THREADS: usize = 10;

const SNI_XML: &str = r#"
<node>
  <interface name="org.kde.StatusNotifierItem">
    <property name="Category" type="s" access="read"/>
    <property name="Id" type="s" access="read"/>
    <property name="Title" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="IconName" type="s" access="read"/>
    <property name="IconPixmap" type="a(iiay)" access="read"/>
    <property name="IconThemePath" type="s" access="read"/>
    <property name="AttentionIconName" type="s" access="read"/>
    <property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
    <property name="ItemIsMenu" type="b" access="read"/>
    <property name="Menu" type="o" access="read"/>
    <method name="Activate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
    <method name="SecondaryActivate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
    <method name="ContextMenu"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
    <method name="Scroll"><arg type="i" direction="in"/><arg type="s" direction="in"/></method>
    <signal name="NewTitle"/>
    <signal name="NewIcon"/>
    <signal name="NewToolTip"/>
    <signal name="NewStatus"><arg type="s"/></signal>
  </interface>
</node>
"#;

const MENU_XML: &str = r#"
<node>
  <interface name="com.canonical.dbusmenu">
    <property name="Version" type="u" access="read"/>
    <property name="TextDirection" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="IconThemePath" type="as" access="read"/>
    <method name="GetLayout">
      <arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="as" direction="in"/>
      <arg type="u" direction="out"/><arg type="(ia{sv}av)" direction="out"/>
    </method>
    <method name="GetGroupProperties">
      <arg type="ai" direction="in"/><arg type="as" direction="in"/><arg type="a(ia{sv})" direction="out"/>
    </method>
    <method name="GetProperty">
      <arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="out"/>
    </method>
    <method name="Event">
      <arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="in"/><arg type="u" direction="in"/>
    </method>
    <method name="EventGroup">
      <arg type="a(isvu)" direction="in"/><arg type="ai" direction="out"/>
    </method>
    <method name="AboutToShow"><arg type="i" direction="in"/><arg type="b" direction="out"/></method>
    <method name="AboutToShowGroup">
      <arg type="ai" direction="in"/><arg type="ai" direction="out"/><arg type="ai" direction="out"/>
    </method>
    <signal name="LayoutUpdated"><arg type="u"/><arg type="i"/></signal>
    <signal name="ItemsPropertiesUpdated"><arg type="a(ia{sv})"/><arg type="a(ias)"/></signal>
  </interface>
</node>
"#;

type Props = HashMap<String, glib::Variant>;
type Action = Rc<dyn Fn(&Rc<App>)>;

#[derive(Default)]
pub struct Tray {
    threads: RefCell<Vec<Value>>,
    unread: Cell<i64>,
    revision: Cell<u32>,
    /// Menu items by ID (1-based), with what a click on them does.
    items: RefCell<Vec<(Props, Option<Action>)>>,
    connection: RefCell<Option<gio::DBusConnection>>,
    /// The chosen app icon as ARGB32 images, which hosts draw instead of looking up the app's icon
    /// name in their own icon theme, where a changed icon file would not show until they restart.
    pixmaps: RefCell<Vec<(i32, i32, Vec<u8>)>>,
    /// The bus name while the icon shows; hosts drop the item when it is released.
    owner: RefCell<Option<gio::OwnerId>>,
}

/// The icon at the sizes trays use, as StatusNotifierItem pixmaps: ARGB32 in network byte order.
fn argb_pixmaps(path: &std::path::Path) -> Vec<(i32, i32, Vec<u8>)> {
    [32, 64]
        .into_iter()
        .filter_map(|size| gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(path, size, size, true).ok())
        .map(|pixbuf| {
            let pixbuf = pixbuf.add_alpha(false, 0, 0, 0).unwrap_or(pixbuf);
            let (width, height, stride) = (pixbuf.width(), pixbuf.height(), pixbuf.rowstride() as usize);
            let bytes = pixbuf.read_pixel_bytes();
            let mut argb = Vec::with_capacity((width * height * 4) as usize);
            for row in 0..height as usize {
                for [r, g, b, a] in bytes[row * stride..][..width as usize * 4].as_chunks::<4>().0 {
                    argb.extend_from_slice(&[*a, *r, *g, *b]);
                }
            }
            (width, height, argb)
        })
        .collect()
}

fn service_name() -> String {
    format!("org.kde.StatusNotifierItem-{}-1", std::process::id())
}

fn interface(xml: &str) -> gio::DBusInterfaceInfo {
    let node = gio::DBusNodeInfo::for_xml(xml).expect("valid introspection XML");
    node.interfaces().first().cloned().expect("one interface")
}

impl Tray {
    pub fn start(&self, app: &Rc<App>) {
        self.revision.set(1);
        let Some(connection) = app.adw.dbus_connection() else { return };
        let weak = Rc::downgrade(app);
        let item = connection
            .register_object(ITEM_PATH, &interface(SNI_XML))
            .method_call({
                let weak = weak.clone();
                move |_, _, _, _, method, _, invocation| {
                    if matches!(method, "Activate" | "SecondaryActivate")
                        && let Some(app) = weak.upgrade()
                    {
                        app.present_main();
                    }
                    invocation.return_value(None);
                }
            })
            .property({
                let weak = weak.clone();
                move |_, _, _, _, name| match weak.upgrade() {
                    Some(app) => app.tray.item_property(name),
                    None => "".to_variant(),
                }
            })
            .build();
        let menu = connection
            .register_object(MENU_PATH, &interface(MENU_XML))
            .method_call({
                let weak = weak.clone();
                move |_, _, _, _, method, params, invocation| match weak.upgrade() {
                    Some(app) => app.tray.menu_call(&app, method, &params, invocation),
                    None => invocation.return_value(None),
                }
            })
            .property(|_, _, _, _, name| match name {
                "Version" => 3u32.to_variant(),
                "TextDirection" => "ltr".to_variant(),
                "Status" => "normal".to_variant(),
                _ => Vec::<String>::new().to_variant(),
            })
            .build();
        if let Err(error) = item.and(menu) {
            log::info!("status icon not exported: {error}");
            return;
        }
        let weak_appeared = weak.clone();
        gio::bus_watch_name_on_connection(
            &connection,
            WATCHER,
            gio::BusNameWatcherFlags::NONE,
            move |_, _, _| {
                if let Some(app) = weak_appeared.upgrade() {
                    app.tray.register();
                }
            },
            |_, _| {},
        );
        *self.connection.borrow_mut() = Some(connection);
        self.set_icon(&app.settings.str("app_icon"));
        self.set_visible(app.settings.bool("show_in_tray"));
    }

    /// Shows the app icon variant `key` (dark, oled, light, puck), as the Mac's status item follows
    /// the app icon.
    pub fn set_icon(&self, key: &str) {
        let path = amp_common::config::icon_file(key).or_else(|| amp_common::config::icon_file("app"));
        *self.pixmaps.borrow_mut() = path.map(|path| argb_pixmaps(&path)).unwrap_or_default();
        self.emit(ITEM_PATH, "org.kde.StatusNotifierItem", "NewIcon", None);
    }

    /// Shows or hides the icon (Show Amp in Tray), as the Mac adds or removes its status item.
    pub fn set_visible(&self, visible: bool) {
        let Some(connection) = self.connection.borrow().clone() else { return };
        if visible == self.owner.borrow().is_some() {
            return;
        }
        if visible {
            let owner = gio::bus_own_name_on_connection(
                &connection,
                &service_name(),
                gio::BusNameOwnerFlags::NONE,
                |_, _| {},
                |_, _| {},
            );
            *self.owner.borrow_mut() = Some(owner);
            self.register();
        } else {
            if let Some(owner) = self.owner.borrow_mut().take() {
                gio::bus_unown_name(owner);
            }
        }
    }

    /// Asks the StatusNotifierWatcher, if there is one, to show the icon.
    fn register(&self) {
        let Some(connection) = self.connection.borrow().clone() else { return };
        if self.owner.borrow().is_none() {
            return;
        }
        connection.call(
            Some(WATCHER),
            "/StatusNotifierWatcher",
            WATCHER,
            "RegisterStatusNotifierItem",
            Some(&(service_name(),).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            -1,
            gio::Cancellable::NONE,
            |result| {
                if let Err(error) = result {
                    log::info!("status icon not registered: {}", error.message());
                }
            },
        );
    }

    // ---- state from the page ---------------------------------------------------------------

    pub fn update(&self, threads: &[Value], unread: i64) {
        *self.threads.borrow_mut() =
            threads.iter().filter(|t| t.get("id").is_some_and(Value::is_string)).take(MAX_THREADS).cloned().collect();
        self.unread.set(unread.max(0));
        self.revision.set(self.revision.get() + 1);
        if self.connection.borrow().is_some() {
            self.emit(
                MENU_PATH,
                "com.canonical.dbusmenu",
                "LayoutUpdated",
                Some((self.revision.get(), 0i32).to_variant()),
            );
            self.emit(ITEM_PATH, "org.kde.StatusNotifierItem", "NewToolTip", None);
            self.emit(ITEM_PATH, "org.kde.StatusNotifierItem", "NewTitle", None);
        }
    }

    fn emit(&self, path: &str, interface: &str, name: &str, params: Option<glib::Variant>) {
        if let Some(connection) = self.connection.borrow().as_ref()
            && let Err(error) = connection.emit_signal(None, path, interface, name, params.as_ref())
        {
            log::debug!("signal {name} failed: {}", error.message());
        }
    }

    fn title(&self) -> String {
        match self.unread.get() {
            0 => "Amp".into(),
            1 => "Amp — 1 unread thread".into(),
            n => format!("Amp — {n} unread threads"),
        }
    }

    // ---- StatusNotifierItem ----------------------------------------------------------------

    fn item_property(&self, name: &str) -> glib::Variant {
        match name {
            "Category" => "ApplicationStatus".to_variant(),
            "Id" => "amp".to_variant(),
            "Title" => self.title().to_variant(),
            "Status" => "Active".to_variant(),
            // Hosts prefer the name to the pixmap, so the name is left empty while there is one.
            "IconName" | "AttentionIconName" if !self.pixmaps.borrow().is_empty() => "".to_variant(),
            "IconName" | "AttentionIconName" => APP_ID.to_variant(),
            "IconPixmap" => self.pixmaps.borrow().to_variant(),
            "IconThemePath" => "".to_variant(),
            "ToolTip" => (APP_ID, Vec::<(i32, i32, Vec<u8>)>::new(), "Amp", self.tooltip_body()).to_variant(),
            "ItemIsMenu" => false.to_variant(),
            "Menu" => glib::variant::ObjectPath::try_from(MENU_PATH).expect("valid path").to_variant(),
            _ => "".to_variant(),
        }
    }

    fn tooltip_body(&self) -> String {
        let running =
            self.threads.borrow().iter().filter(|t| t.get("isRunning").is_some_and(amp_common::config::truthy)).count();
        let mut parts = Vec::new();
        if self.unread.get() > 0 {
            parts.push(format!("{} unread", self.unread.get()));
        }
        if running > 0 {
            parts.push(format!("{running} running"));
        }
        parts.join(", ")
    }

    // ---- dbusmenu --------------------------------------------------------------------------

    /// Rebuilds the menu; returns the IDs of the entries under root 0.
    fn layout(&self) -> Vec<i32> {
        let mut items: Vec<(Props, Option<Action>)> = Vec::new();
        let mut item = |label: &str, action: Option<Action>| {
            let mut props = Props::new();
            props.insert("label".into(), label.to_variant());
            if action.is_none() {
                props.insert("enabled".into(), false.to_variant());
            }
            items.push((props, action));
        };
        let action = |f: fn(&Rc<App>)| -> Option<Action> { Some(Rc::new(f)) };
        item("Open Amp", action(|app| app.present_main()));
        item("New Thread", action(|app| app.adw.activate_action("new-thread", None)));
        item("Show Puck", action(|app| app.adw.activate_action("toggle-puck", None)));
        let threads = self.threads.borrow().clone();
        let separator = |items: &mut Vec<(Props, Option<Action>)>| {
            let mut props = Props::new();
            props.insert("type".into(), "separator".to_variant());
            items.push((props, None));
        };
        if !threads.is_empty() {
            separator(&mut items);
            let mut item = |label: &str, action: Option<Action>| {
                let mut props = Props::new();
                props.insert("label".into(), label.to_variant());
                if action.is_none() {
                    props.insert("enabled".into(), false.to_variant());
                }
                items.push((props, action));
            };
            item(&if self.unread.get() > 0 { self.title() } else { "Recent Threads".into() }, None);
            for thread in threads {
                let flag = |key: &str| thread.get(key).is_some_and(amp_common::config::truthy);
                let marks = if flag("needsAttention") {
                    "⚠ "
                } else if flag("isUnread") {
                    "● "
                } else if flag("isRunning") {
                    "◐ "
                } else {
                    ""
                };
                let title =
                    thread.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()).unwrap_or("Untitled thread");
                let mut label = format!("{marks}{title}").replace('_', "__");
                if label.chars().count() > 60 {
                    label = label.chars().take(59).collect::<String>() + "…";
                }
                let id = thread.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                item(&label, Some(Rc::new(move |app: &Rc<App>| app.open_thread(&id))));
            }
        }
        separator(&mut items);
        let mut item = |label: &str, action: Option<Action>| {
            let mut props = Props::new();
            props.insert("label".into(), label.to_variant());
            items.push((props, action));
        };
        item("Preferences", action(|app| app.show_app_settings(None)));
        item("Quit Amp", action(|app| app.quit_all()));
        let ids = (1..=items.len() as i32).collect();
        *self.items.borrow_mut() = items;
        ids
    }

    fn props(&self, id: i32) -> Props {
        let items = self.items.borrow();
        usize::try_from(id - 1).ok().and_then(|i| items.get(i)).map(|(p, _)| p.clone()).unwrap_or_default()
    }

    fn clicked(&self, app: &Rc<App>, id: i32) {
        let action = usize::try_from(id - 1).ok().and_then(|i| self.items.borrow().get(i).and_then(|(_, a)| a.clone()));
        if let Some(action) = action {
            let weak: Weak<App> = Rc::downgrade(app);
            glib::idle_add_local_once(move || {
                if let Some(app) = weak.upgrade() {
                    action(&app);
                }
            });
        }
    }

    fn menu_call(&self, app: &Rc<App>, method: &str, params: &glib::Variant, invocation: gio::DBusMethodInvocation) {
        match method {
            "GetLayout" => {
                let entries = self.layout();
                let mut root_props = Props::new();
                root_props.insert("children-display".into(), "submenu".to_variant());
                let children: Vec<glib::Variant> =
                    entries.iter().map(|&i| (i, self.props(i), Vec::<glib::Variant>::new()).to_variant()).collect();
                let root = (0i32, root_props, children);
                invocation.return_value(Some(&(self.revision.get(), root).to_variant()));
            }
            "GetGroupProperties" => {
                let (ids, _) = params.get::<(Vec<i32>, Vec<String>)>().unwrap_or_default();
                let ids = if ids.is_empty() { (1..=self.items.borrow().len() as i32).collect() } else { ids };
                let result: Vec<(i32, Props)> = ids.into_iter().map(|i| (i, self.props(i))).collect();
                invocation.return_value(Some(&(result,).to_variant()));
            }
            "GetProperty" => {
                let (id, name) = params.get::<(i32, String)>().unwrap_or_default();
                let value = self.props(id).remove(&name).unwrap_or_else(|| "".to_variant());
                invocation.return_value(Some(&(value,).to_variant()));
            }
            "Event" => {
                if let Some((id, event, _, _)) = params.get::<(i32, String, glib::Variant, u32)>()
                    && event == "clicked"
                {
                    self.clicked(app, id);
                }
                invocation.return_value(None);
            }
            "EventGroup" => {
                let (events,) = params.get::<(Vec<(i32, String, glib::Variant, u32)>,)>().unwrap_or_default();
                for (id, event, _, _) in events {
                    if event == "clicked" {
                        self.clicked(app, id);
                    }
                }
                invocation.return_value(Some(&(Vec::<i32>::new(),).to_variant()));
            }
            "AboutToShow" => {
                // Ask the page for fresh threads, as the Mac menu does when it opens.
                app.request_menu_refresh();
                invocation.return_value(Some(&(false,).to_variant()));
            }
            "AboutToShowGroup" => {
                app.request_menu_refresh();
                invocation.return_value(Some(&(Vec::<i32>::new(), Vec::<i32>::new()).to_variant()));
            }
            _ => invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", method),
        }
    }
}

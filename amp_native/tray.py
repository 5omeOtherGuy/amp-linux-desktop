"""The status icon: the Mac app's menu bar extra, as a StatusNotifierItem.

Waybar, KDE, and GNOME's AppIndicator extension show StatusNotifierItems. The menu lists the
recent threads the page sends through ampNativeMenuBar (version 2), with the unread ones marked,
plus New Thread, Puck, Preferences, and Quit. Without a StatusNotifierWatcher on the bus the
icon stays unregistered and nothing else changes.
"""

import logging
import os

from gi.repository import Gio, GLib

from . import config

log = logging.getLogger(__name__)

WATCHER = "org.kde.StatusNotifierWatcher"
ITEM_PATH = "/StatusNotifierItem"
MENU_PATH = "/MenuBar"
MAX_THREADS = 10

SNI_XML = """
<node>
  <interface name="org.kde.StatusNotifierItem">
    <property name="Category" type="s" access="read"/>
    <property name="Id" type="s" access="read"/>
    <property name="Title" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="IconName" type="s" access="read"/>
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
"""

MENU_XML = """
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
"""


class Tray:
    def __init__(self, app):
        self.app = app
        self.threads = []
        self.unread = 0
        self.revision = 1
        self._items = {}
        self._actions = {}
        self._connection = None
        self._service = f"org.kde.StatusNotifierItem-{os.getpid()}-1"
        self._registered = False

    def start(self):
        self._connection = self.app.get_dbus_connection()
        if self._connection is None:
            return
        self._connection.register_object(
            ITEM_PATH, Gio.DBusNodeInfo.new_for_xml(SNI_XML).interfaces[0],
            self._item_call, self._item_property, None)
        self._connection.register_object(
            MENU_PATH, Gio.DBusNodeInfo.new_for_xml(MENU_XML).interfaces[0],
            self._menu_call, self._menu_property, None)
        Gio.bus_own_name_on_connection(self._connection, self._service, Gio.BusNameOwnerFlags.NONE, None, None)
        Gio.bus_watch_name_on_connection(self._connection, WATCHER, Gio.BusNameWatcherFlags.NONE,
                                         lambda *_: self._register(), lambda *_: setattr(self, "_registered", False))

    @property
    def registered(self):
        return self._registered

    def _register(self):
        def done(connection, result):
            try:
                connection.call_finish(result)
                self._registered = True
            except GLib.Error as error:
                log.info("status icon not registered: %s", error.message)

        self._connection.call(WATCHER, "/StatusNotifierWatcher", WATCHER, "RegisterStatusNotifierItem",
                              GLib.Variant("(s)", (self._service,)), None, Gio.DBusCallFlags.NONE, -1, None, done)

    # ---- state from the page -----------------------------------------------------------------

    def update(self, threads, unread):
        self.threads = [t for t in threads if isinstance(t, dict) and isinstance(t.get("id"), str)][:MAX_THREADS]
        self.unread = max(0, int(unread or 0))
        self.revision += 1
        if self._connection:
            self._emit(MENU_PATH, "com.canonical.dbusmenu", "LayoutUpdated", GLib.Variant("(ui)", (self.revision, 0)))
            self._emit(ITEM_PATH, "org.kde.StatusNotifierItem", "NewToolTip", None)
            self._emit(ITEM_PATH, "org.kde.StatusNotifierItem", "NewTitle", None)

    def _emit(self, path, interface, name, params):
        try:
            self._connection.emit_signal(None, path, interface, name, params)
        except GLib.Error as error:
            log.debug("signal %s failed: %s", name, error.message)

    def title(self):
        if self.unread:
            return f"Amp — {self.unread} unread thread{'s' if self.unread != 1 else ''}"
        return "Amp"

    # ---- StatusNotifierItem ------------------------------------------------------------------

    def _item_property(self, _conn, _sender, _path, _iface, name):
        values = {
            "Category": GLib.Variant("s", "ApplicationStatus"),
            "Id": GLib.Variant("s", "amp"),
            "Title": GLib.Variant("s", self.title()),
            "Status": GLib.Variant("s", "Active"),
            "IconName": GLib.Variant("s", config.APP_ID),
            "IconThemePath": GLib.Variant("s", ""),
            "AttentionIconName": GLib.Variant("s", config.APP_ID),
            "ToolTip": GLib.Variant("(sa(iiay)ss)", (config.APP_ID, [], "Amp", self._tooltip_body())),
            "ItemIsMenu": GLib.Variant("b", False),
            "Menu": GLib.Variant("o", MENU_PATH),
        }
        return values.get(name)

    def _tooltip_body(self):
        running = sum(1 for t in self.threads if t.get("isRunning"))
        parts = []
        if self.unread:
            parts.append(f"{self.unread} unread")
        if running:
            parts.append(f"{running} running")
        return ", ".join(parts)

    def _item_call(self, _conn, _sender, _path, _iface, method, _params, invocation):
        if method in ("Activate", "SecondaryActivate"):
            self.app.present_main()
        invocation.return_value(None)

    # ---- dbusmenu ----------------------------------------------------------------------------

    def _layout(self):
        """Rebuilds the menu: a list of (id, properties, children) under root 0."""
        self._items, self._actions = {}, {}
        entries = []

        def item(label, action=None, **extra):
            item_id = len(self._items) + 1
            props = {"label": GLib.Variant("s", label)}
            if action is None and not extra:
                props["enabled"] = GLib.Variant("b", False)
            for key, value in extra.items():
                props[key.replace("_", "-")] = value
            self._items[item_id] = props
            if action:
                self._actions[item_id] = action
            entries.append(item_id)

        def separator():
            item_id = len(self._items) + 1
            self._items[item_id] = {"type": GLib.Variant("s", "separator")}
            entries.append(item_id)

        item("Open Amp", self.app.present_main)
        item("New Thread", lambda: self.app.activate_action("new-thread", None))
        item("Show Puck", lambda: self.app.activate_action("toggle-puck", None))
        if self.threads:
            separator()
            item(self.title() if self.unread else "Recent Threads")
            for thread in self.threads:
                marks = "● " if thread.get("isUnread") else ("◐ " if thread.get("isRunning") else "")
                if thread.get("needsAttention"):
                    marks = "⚠ "
                label = f"{marks}{thread.get('title') or 'Untitled thread'}".replace("_", "__")
                if len(label) > 60:
                    label = label[:59] + "…"
                item(label, lambda tid=thread["id"]: self.app.open_thread(tid))
        separator()
        item("Preferences", lambda: self.app.show_app_settings())
        item("Quit Amp", self.app.quit_all)
        return entries

    def _menu_property(self, _conn, _sender, _path, _iface, name):
        return {
            "Version": GLib.Variant("u", 3),
            "TextDirection": GLib.Variant("s", "ltr"),
            "Status": GLib.Variant("s", "normal"),
            "IconThemePath": GLib.Variant("as", []),
        }.get(name)

    def _node(self, item_id, children):
        return GLib.Variant("(ia{sv}av)", (item_id, self._items.get(item_id, {}), children))

    def _menu_call(self, _conn, _sender, _path, _iface, method, params, invocation):
        if method == "GetLayout":
            entries = self._layout()
            root = GLib.Variant("(ia{sv}av)", (0, {"children-display": GLib.Variant("s", "submenu")},
                                               [self._node(i, []) for i in entries]))
            invocation.return_value(GLib.Variant.new_tuple(GLib.Variant("u", self.revision), root))
        elif method == "GetGroupProperties":
            ids = params.unpack()[0] or list(self._items)
            invocation.return_value(GLib.Variant("(a(ia{sv}))", ([(i, self._items.get(i, {})) for i in ids],)))
        elif method == "GetProperty":
            item_id, name = params.unpack()
            value = self._items.get(item_id, {}).get(name, GLib.Variant("s", ""))
            invocation.return_value(GLib.Variant("(v)", (value,)))
        elif method == "Event":
            item_id, event, _data, _time = params.unpack()
            if event == "clicked" and item_id in self._actions:
                GLib.idle_add(lambda: self._actions[item_id]() and False)
            invocation.return_value(None)
        elif method == "EventGroup":
            for item_id, event, _data, _time in params.unpack()[0]:
                if event == "clicked" and item_id in self._actions:
                    GLib.idle_add(lambda i=item_id: self._actions[i]() and False)
            invocation.return_value(GLib.Variant("(ai)", ([],)))
        elif method == "AboutToShow":
            # Ask the page for fresh threads, as the Mac menu does when it opens.
            self.app.request_menu_refresh()
            invocation.return_value(GLib.Variant("(b)", (False,)))
        elif method == "AboutToShowGroup":
            self.app.request_menu_refresh()
            invocation.return_value(GLib.Variant("(aiai)", ([], [])))
        else:
            invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", method)

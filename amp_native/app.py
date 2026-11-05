"""The application: windows, the Mac menu bar's commands and shortcuts, notifications, and badges."""

import json
import logging
import os
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

from gi.repository import Adw, Gdk, Gio, GLib, Gtk, WebKit

from . import config, scripts
from .runner import Runner
from .settings_window import SettingsWindow
from .signin import SignIn
from .theme import OmarchyTheme, palette_css
from .tray import Tray
from .huddle import HuddleWindow
from .window import AmpWindow, is_app_url

log = logging.getLogger(__name__)

CSS = Path(__file__).with_name("style.css")

# Menu bar of the Mac app (Amp, File, Edit, View, Window, Help), with ⌘ mapped to Ctrl.
# Each entry: (label, action, accelerator or None). None as an entry separates sections.
MENUS = [
    ("File", [
        ("New Thread", "win.new-thread", None),
        ("New Window", "app.new-window", "<Control><Shift>n"),
        None,
        ("Print…", "win.print", "<Control>p"),
        None,
        ("Close", "win.close", "<Control>w"),
        ("Quit", "app.quit", "<Control>q"),
    ]),
    ("Edit", [
        ("Undo", "win.undo", "<Control>z"),
        ("Redo", "win.redo", "<Control><Shift>z"),
        None,
        ("Cut", "win.cut", "<Control>x"),
        ("Copy", "win.copy", "<Control>c"),
        ("Paste", "win.paste", "<Control>v"),
        ("Select All", "win.select-all", "<Control>a"),
        None,
        ("Personal Settings", "win.personal-settings", "<Control>comma"),
        ("Workspace Settings", "win.workspace-settings", "<Control><Alt>comma"),
        ("Preferences", "app.app-settings", "<Control><Shift>comma"),
    ]),
    ("View", [
        ("Show Sidebar", "win.toggle-sidebar", "<Control>b"),
        ("Show Puck", "win.toggle-puck", "<Control>slash"),
        None,
        ("Back", "win.back", "<Alt>Left"),
        ("Forward", "win.forward", "<Alt>Right"),
        ("Reload Portal", "win.reload-portal", "<Control>r"),
        ("Reload Window", "win.reload", "<Control><Shift>r"),
        None,
        ("Zoom In", "app.zoom-in", "<Control>equal"),
        ("Zoom Out", "app.zoom-out", "<Control>minus"),
        ("Reset Zoom", "app.zoom-reset", "<Control>0"),
        ("Full Screen", "win.fullscreen", "F11"),
        None,
        ("Developer Tools", "win.inspector", "<Control><Shift>i"),
    ]),
    ("Help", [
        ("Keyboard Shortcuts", "win.keyboard-shortcuts", "<Control>question"),
        ("Amp Manual", "app.docs", None),
        ("Report Bug…", "win.report-bug", None),
        None,
        ("About Amp", "app.about", None),
    ]),
]

# Shortcuts that also work without the menu, matching common Linux habits.
EXTRA_ACCELS = {
    "win.back": ["<Control>bracketleft"],
    "win.forward": ["<Control>bracketright"],
    "app.zoom-in": ["<Control>plus", "<Control>KP_Add"],
    "app.zoom-out": ["<Control>KP_Subtract"],
    "win.minimize": [],
}
# Left to the web view, which already does them; the menu only names them.
WEB_HANDLED = {"win.undo", "win.redo", "win.cut", "win.copy", "win.paste", "win.select-all"}

QUIT_HOLD_MS = 900

# App actions run from outside the window, mapped to the page's app commands (None only shows Amp).
GLOBAL_COMMANDS = {
    "show": None,
    "new-thread": "createThread",
    "dictate-new-thread": "dictateNewThread",
    "toggle-puck": "togglePuck",
    "puck-voice": "togglePuckRealtimeVoice",
}


class AmpApplication(Adw.Application):
    def __init__(self):
        super().__init__(application_id=config.APP_ID, flags=Gio.ApplicationFlags.HANDLES_OPEN)
        GLib.set_application_name(config.APP_NAME)
        self.settings = config.Settings()
        self.windows = []
        self.settings_window = None
        self.unread = 0
        self.sign_in = SignIn(self)
        self.runner = Runner(self)
        self._quit_hold = None
        self._shortcuts = []
        self._labels = {}
        self._pending_command = None
        self.huddles = {}
        self.tray = Tray(self)
        self.add_main_option("action", 0, GLib.OptionFlags.NONE, GLib.OptionArg.STRING,
                             f"Run an action in the running Amp: {', '.join(GLOBAL_COMMANDS)}", "NAME")
        self.connect("handle-local-options", self._handle_local_options)
        self.user_agent_suffix = (
            f"AmpNativeApp/1 AmpPlatform/Linux AmpAppVersion/0.1.{config.BUILD_NUMBER} "
            # WebKit only accepts RFC 9110 product tokens, so the two-slash AmpApp token goes in a
            # comment, where the page's /\bAmpApp\/(...)\/(\d+)\b/ still finds it.
            f"(AmpApp/{config.BUNDLE_ID}/{config.BUILD_NUMBER}) "
            "AmpMCPOAuth/1 AmpConnectOAuth/1 AmpConnectOAuth/2 AmpConnectOAuth/3"
        )

    # ---- lifecycle ---------------------------------------------------------------------------

    def do_startup(self):
        Adw.Application.do_startup(self)
        for d in (config.DATA_DIR, config.CACHE_DIR, config.CONFIG_DIR):
            d.mkdir(parents=True, exist_ok=True)
        session = WebKit.NetworkSession.new(str(config.DATA_DIR / "web"), str(config.CACHE_DIR / "web"))
        session.get_cookie_manager().set_persistent_storage(
            str(config.DATA_DIR / "web" / "cookies.sqlite"), WebKit.CookiePersistentStorage.SQLITE
        )
        session.connect("download-started", self._download_started)
        self.network_session = session

        provider = Gtk.CssProvider()
        provider.load_from_path(str(CSS))
        Gtk.StyleContext.add_provider_for_display(Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        Gtk.Window.set_default_icon_name(config.APP_ID)
        self.host_palette = None
        self._apply_appearance()
        self.omarchy = OmarchyTheme(self._palette_changed)

        self._add_actions()
        self.menu_model = self._build_menu()
        self.tray.start()
        if self.settings["runner_enabled"]:
            self.runner.apply()

    def do_activate(self):
        window = self.main_window()
        if window is None:
            window = self.open_window("/")
        window.present()

    def do_open(self, files, _count, _hint):
        if self.main_window() is None:
            self.open_window("/")
        for file in files:
            uri = file.get_uri()
            if uri.startswith(config.SIGN_IN_SCHEME + ":"):
                handled = self.sign_in.handle_callback(uri)
                log.info("sign-in callback %s", "accepted" if handled else "ignored (no matching request)")
            elif is_app_url(uri):
                self.open_window(uri[len(config.APP_URL):] or "/")
        self.main_window().present()

    def do_shutdown(self):
        for window in list(self.huddles.values()):
            window.close()
            window.wait_left(3)
        self.runner.stop()
        Adw.Application.do_shutdown(self)

    # ---- windows -----------------------------------------------------------------------------

    def main_window(self):
        return next((w for w in self.windows if w.get_application() is not None), None)

    def present_main(self):
        window = self.main_window() or self.open_window("/")
        window.present()

    def open_window(self, path, auxiliary=None):
        window = AmpWindow(self, path, auxiliary)
        self.windows.append(window)
        window.connect("destroy", lambda w: self.windows.remove(w) if w in self.windows else None)
        self._add_window_actions(window)
        window.present()
        return window

    def show_app_settings(self, tab=None):
        if self.settings_window is None:
            self.settings_window = SettingsWindow(self)
            self.settings_window.connect("destroy", lambda *_: setattr(self, "settings_window", None))
        self.settings_window.show_tab(tab or "general")

    def launch_uri(self, uri):
        """Opens a link in the default browser. Uses the XDG handler directly, since the portal
        Gtk.show_uri goes through is missing on many window-manager-only desktops. $BROWSER, the
        usual override, comes first."""
        browser = os.environ.get("BROWSER")
        if browser:
            try:
                subprocess.Popen([*shlex.split(browser), uri], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                 stderr=subprocess.DEVNULL, start_new_session=True)
                return
            except OSError:
                log.exception("$BROWSER failed")
        try:
            if Gio.AppInfo.launch_default_for_uri(uri, None):
                return
        except GLib.Error as error:
            log.info("no default handler for %s: %s", uri.split("?")[0], error.message)
        try:
            subprocess.Popen(["xdg-open", uri], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL, start_new_session=True)
        except OSError:
            log.exception("could not open %s", uri.split("?")[0])

    def open_in_browser(self, uri):
        if not uri.startswith(("http://", "https://", "mailto:")):
            return
        self.sign_in.remember_relay(uri)
        self.launch_uri(uri)

    # ---- page state --------------------------------------------------------------------------

    def app_state_changed(self, window):
        if window is self.main_window():
            state = window.app_state
            self.runner.account_changed(state.get("signedInEmail"))
            preference = state.get("colorSchemePreference")
            if preference in ("system", "light", "dark") and preference != self.settings["appearance"]:
                self.settings.set("appearance", preference)
                self._apply_appearance()
            if self.settings_window:
                self.settings_window.refresh_account()
            if self._pending_command and state.get("websiteReady"):
                command, self._pending_command = self._pending_command, None
                window.app_command(command)
        self.update_actions()

    def run_global_command(self, command):
        window = self.main_window()
        if window is None:
            # A new window takes the command once the page reports it is ready.
            self._pending_command = command
            self.present_main()
            return
        window.present()
        if command:
            window.app_command(command)

    def join_huddle(self, body):
        """Joins a Huddle in its own window, or shows the window when this call is already open."""
        thread_id = body.get("threadID")
        current = self.huddles.get(thread_id)
        if current:
            current.present()
            return
        title = body.get("title") or ""
        call_name = body.get("callName") if body.get("callName") in ("Huddle", "Space") else "Huddle"
        window = HuddleWindow(self, thread_id, title, call_name, body["serverURL"], body["participantToken"])
        self.huddles[thread_id] = window
        window.present()

    def huddle_ended(self, window, message):
        if self.huddles.get(window.thread_id) is window:
            del self.huddles[window.thread_id]
        main = self.main_window()
        if message and main:
            main.show_toast(message)

    def huddle_reconcile(self, thread_id):
        """Asks the server to update the thread's live call state, as the page does after it
        joins or leaves."""
        main = self.main_window()
        if main:
            main.run_js(f"fetch('/api/threads/' + encodeURIComponent({json.dumps(thread_id)}) + '/space-token', "
                        "{method: 'POST', headers: {'Content-Type': 'application/json'}, "
                        "body: JSON.stringify({action: 'reconcile'}), keepalive: true}).catch(() => {}); true")

    def open_thread(self, thread_id):
        self._open_notification(f"/threads/{thread_id}")

    def request_menu_refresh(self):
        """Tells the page its menu opened, so it sends the threads now instead of within 10 s."""
        window = self.main_window()
        if window:
            window.run_js("window.dispatchEvent(new CustomEvent('amp:macos-menu-bar-menu', {detail: {open: true}}));"
                          "setTimeout(() => window.dispatchEvent(new CustomEvent('amp:macos-menu-bar-menu',"
                          " {detail: {open: false}})), 2000)")

    def menu_bar_changed(self, body):
        self.set_unread_count(body.get("unreadThreadCount", 0) if isinstance(body.get("unreadThreadCount"), int) else self.unread)
        self.tray.update(body.get("threads") if body.get("enabled", True) and isinstance(body.get("threads"), list) else [],
                         self.unread)

    def _handle_local_options(self, _app, options):
        action = options.lookup_value("action", GLib.VariantType.new("s"))
        if action is None:
            return -1
        name = action.get_string()
        if name not in GLOBAL_COMMANDS:
            print(f"amp-linux-desktop: unknown action {name!r}; one of: {', '.join(GLOBAL_COMMANDS)}", file=sys.stderr)
            return 2
        self.register(None)
        if self.get_is_remote():
            self.activate_action(name, None)
            return 0
        self._pending_command = GLOBAL_COMMANDS[name]
        return -1

    def update_actions(self):
        window = self.get_active_window()
        state = window.app_state if isinstance(window, AmpWindow) else {}
        self._set_label("win.toggle-sidebar", "Hide Sidebar" if state.get("sidebarOpen") else "Show Sidebar")
        self._set_label("win.toggle-puck", "Hide Puck" if state.get("puckOpen") else "Show Puck")
        tab = state.get("paneTabClose") or {}
        self._set_label("win.close", "Close Tab" if tab.get("hasClosableTab") and not tab.get("blocked") else "Close")
        self._set_label("win.report-bug", state.get("reportBugLabel") or "Report Bug…")
        if isinstance(window, AmpWindow):
            for name, enabled in (
                ("reload-portal", True),
                ("print", True),
                ("back", window.webview.can_go_back()),
                ("forward", window.webview.can_go_forward()),
            ):
                action = window.lookup_action(name)
                if action:
                    action.set_enabled(enabled)

    def _set_label(self, action, label):
        entry = self._labels.get(action)
        if not entry or entry[2] == label:
            return
        section, index, _old, accel = entry
        item = Gio.MenuItem.new(label, action)
        if accel:
            item.set_attribute_value("accel", GLib.Variant("s", accel))
        section.remove(index)
        section.insert_item(index, item)
        self._labels[action] = (section, index, label, accel)

    # ---- actions -----------------------------------------------------------------------------

    def _add_actions(self):
        simple = {
            "about": lambda *_: self.show_app_settings("about"),
            "app-settings": lambda *_: self.show_app_settings(),
            "quit": lambda *_: self.quit_all(),
            "new-window": lambda *_: self.open_window("/"),
            "zoom-in": lambda *_: self.set_zoom(config.step_zoom(self.settings["zoom"], 1)),
            "zoom-out": lambda *_: self.set_zoom(config.step_zoom(self.settings["zoom"], -1)),
            "zoom-reset": lambda *_: self.set_zoom(1.0),
            "bring-all": lambda *_: [w.present() for w in self.windows],
            "docs": lambda *_: self.launch_uri("https://ampcode.com/manual"),
        }
        for name, callback in simple.items():
            action = Gio.SimpleAction.new(name, None)
            action.connect("activate", callback)
            self.add_action(action)
        warn = Gio.SimpleAction.new_stateful("warn-before-quitting", None, GLib.Variant("b", self.settings["warn_before_quitting"]))
        warn.connect("activate", lambda a, _p: (a.set_state(GLib.Variant("b", not a.get_state().get_boolean())),
                                                self.settings.set("warn_before_quitting", a.get_state().get_boolean())))
        self.settings.connect(lambda k, v: k == "warn_before_quitting" and warn.set_state(GLib.Variant("b", v)))
        self.add_action(warn)
        # Desktop-wide entry points, as the Mac's global shortcuts: the tray menu, the launcher's
        # desktop actions and `amp-linux-desktop --action NAME` (bind that in the compositor) run these.
        for name, command in GLOBAL_COMMANDS.items():
            action = Gio.SimpleAction.new(name, None)
            action.connect("activate", lambda *_, c=command: self.run_global_command(c))
            self.add_action(action)
        open_url = Gio.SimpleAction.new("open-url", GLib.VariantType.new("s"))
        open_url.connect("activate", lambda _a, p: self._open_notification(p.get_string()))
        self.add_action(open_url)

    def _add_window_actions(self, window):
        def command(name, fallback=None):
            return lambda *_: window.app_command(name, fallback=fallback)

        def close(*_):
            tab = window.app_state.get("paneTabClose") or {}
            if tab.get("hasClosableTab") and not tab.get("blocked"):
                window.app_command("closeActiveTab", fallback=window.close)
            else:
                window.close()

        def edit(name):
            return lambda *_: window.webview.execute_editing_command(name)

        def reload_portal(*_):
            if window.app_state.get("portalReloadAvailable"):
                window.app_command("reloadPortal", fallback=window.webview.reload)
            else:
                window.webview.reload()

        def print_page(*_):
            if window.app_state.get("printAvailable"):
                window.app_command("print", fallback=lambda: WebKit.PrintOperation.new(window.webview).run_dialog(window))
            else:
                WebKit.PrintOperation.new(window.webview).run_dialog(window)

        actions = {
            "personal-settings": command("openSettings"),
            "workspace-settings": command("openWorkspaceSettings"),
            "new-thread": command("createThread"),
            "close": close,
            "print": print_page,
            "undo": edit(WebKit.EDITING_COMMAND_UNDO),
            "redo": edit(WebKit.EDITING_COMMAND_REDO),
            "cut": edit(WebKit.EDITING_COMMAND_CUT),
            "copy": edit(WebKit.EDITING_COMMAND_COPY),
            "paste": edit(WebKit.EDITING_COMMAND_PASTE),
            "select-all": edit(WebKit.EDITING_COMMAND_SELECT_ALL),
            "toggle-sidebar": command("toggleSidebar"),
            "toggle-puck": command("togglePuck"),
            "back": lambda *_: window.webview.go_back(),
            "forward": lambda *_: window.webview.go_forward(),
            "reload-portal": reload_portal,
            "reload": lambda *_: window.webview.reload(),
            "fullscreen": lambda *_: window.toggle_full_screen(),
            "inspector": lambda *_: window.webview.get_inspector().show(),
            "minimize": lambda *_: window.minimize(),
            "zoom": lambda *_: window.toggle_maximized(),
            "keyboard-shortcuts": lambda *_: window.run_js(
                "document.dispatchEvent(new KeyboardEvent('keydown', {key: '?', bubbles: true}))"),
            "report-bug": command("reportBug"),
        }
        for name, callback in actions.items():
            action = Gio.SimpleAction.new(name, None)
            action.connect("activate", callback)
            window.add_action(action)
        window.webview.connect("load-changed", lambda *_: self.update_actions())
        window.connect("notify::is-active", lambda *_: self.update_actions())
        window.connect("notify::fullscreened", lambda *_: self.update_actions())

    def _build_menu(self):
        bar = Gio.Menu()
        for title, entries in MENUS:
            menu = Gio.Menu()
            section = Gio.Menu()
            for entry in entries:
                if entry is None:
                    menu.append_section(None, section)
                    section = Gio.Menu()
                    continue
                label, action, accel = entry
                item = Gio.MenuItem.new(label, action)
                if accel:
                    item.set_attribute_value("accel", GLib.Variant("s", accel))
                    if action not in WEB_HANDLED:
                        self._shortcuts.append((*Gtk.accelerator_parse(accel), action))
                self._labels[action] = (section, section.get_n_items(), label, accel)
                section.append_item(item)
            menu.append_section(None, section)
            bar.append_submenu(title, menu)
        for action, accels in EXTRA_ACCELS.items():
            for accel in accels:
                self._shortcuts.append((*Gtk.accelerator_parse(accel), action))
        return bar

    def handle_shortcut(self, window, keyval, keycode, state):
        """Runs a menu shortcut before the page sees the key, as AppKit's key equivalents do.

        Matches the physical key as well as the keyval, so Ctrl+Shift+comma still matches when
        Shift turns the comma into "<".
        """
        mods = state & Gtk.accelerator_get_default_mod_mask()
        display = window.get_display()
        for ok, key, accel_mods, action in self._shortcuts:
            if not ok:
                continue
            # A shifted symbol such as "?" or "+" already includes Shift in its keyval.
            wanted = mods & ~Gdk.ModifierType.SHIFT_MASK if keyval == key and key < 0x100 and not chr(key).isalpha() else mods
            if accel_mods != wanted:
                continue
            same_key = Gdk.keyval_to_lower(keyval) == Gdk.keyval_to_lower(key)
            if not same_key:
                found, keys = display.map_keyval(key)
                same_key = found and any(k.keycode == keycode for k in keys)
            if same_key:
                return self._run_shortcut(window, action)
        return False

    def _run_shortcut(self, window, action):
        if action == "app.quit" and self.settings["warn_before_quitting"]:
            self._hold_to_quit(window)
            return True
        scope, name = action.split(".", 1)
        target = (self if scope == "app" else window).lookup_action(name)
        if target and target.get_enabled():
            target.activate(None)
            return True
        return False

    def shortcut_list(self):
        shortcuts = []
        for title, entries in MENUS:
            for entry in entries:
                if entry and entry[2]:
                    ok, key, mods = Gtk.accelerator_parse(entry[2])
                    shortcuts.append({"title": entry[0], "shortcut": Gtk.accelerator_get_label(key, mods) if ok else None})
        return shortcuts

    def _hold_to_quit(self, window):
        if self._quit_hold is not None:
            return
        window.show_toast("Hold Ctrl+Q to Quit")

        def released(*_):
            if self._quit_hold:
                GLib.source_remove(self._quit_hold[0])
                window.disconnect_key_release(self._quit_hold[1])
                self._quit_hold = None

        timer = GLib.timeout_add(QUIT_HOLD_MS, lambda: (self.quit_all(), False)[1])
        handler = window.on_key_release(Gdk.KEY_q, released)
        self._quit_hold = (timer, handler)

    def quit_all(self):
        for window in list(self.windows):
            window.close()
        if self.settings_window:
            self.settings_window.close()
        self.quit()

    # ---- settings ----------------------------------------------------------------------------

    def set_zoom(self, factor):
        factor = config.nearest_zoom(factor)
        self.settings.set("zoom", factor)
        for window in self.windows:
            window.set_zoom(factor)

    def set_appearance(self, value):
        self.settings.set("appearance", value)
        self._apply_appearance()
        for window in self.windows:
            window.app_command("setColorScheme", colorScheme=value)

    def host_css(self):
        return palette_css(self.host_palette) if self.host_palette else None

    def _palette_changed(self, palette):
        self.host_palette = palette
        self._apply_appearance()
        for window in self.windows:
            window.apply_host_css()

    def _apply_appearance(self):
        if self.host_palette and self.settings["appearance"] == "system":
            # A desktop theme decides light or dark, as amp-desktop does with nativeTheme.
            Adw.StyleManager.get_default().set_color_scheme(
                Adw.ColorScheme.FORCE_DARK if self.host_palette["mode"] == "dark" else Adw.ColorScheme.FORCE_LIGHT
            )
            return
        scheme = {
            "light": Adw.ColorScheme.FORCE_LIGHT,
            "dark": Adw.ColorScheme.FORCE_DARK,
        }.get(self.settings["appearance"], Adw.ColorScheme.DEFAULT)
        Adw.StyleManager.get_default().set_color_scheme(scheme)

    def set_open_at_login(self, enabled):
        self.settings.set("open_at_login", enabled)
        autostart = Path(GLib.get_user_config_dir()) / "autostart" / f"{config.APP_ID}.desktop"
        if enabled:
            autostart.parent.mkdir(parents=True, exist_ok=True)
            autostart.write_text(
                "[Desktop Entry]\nType=Application\nName=Amp\n"
                f"Exec={sys.executable} -m amp_native\nIcon={config.APP_ID}\nX-GNOME-Autostart-enabled=true\n"
            )
        else:
            autostart.unlink(missing_ok=True)

    def set_app_icon(self, key):
        self.settings.set("app_icon", key)
        source = config.ICON_DIR / f"{key}.png"
        if not source.exists():
            return
        target = Path(GLib.get_user_data_dir()) / "icons/hicolor/512x512/apps" / f"{config.APP_ID}.png"
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
        Gtk.IconTheme.get_for_display(Gdk.Display.get_default()).add_search_path(str(target.parent.parent.parent))
        for window in self.windows:
            window.set_icon_name(None)
            window.set_icon_name(config.APP_ID)

    # ---- notifications and badge -------------------------------------------------------------

    def enable_notifications(self):
        self.settings.set("notifications", "authorized")
        for window in self.windows:
            window.run_js(scripts.notification_permission("authorized"))
        if self.settings_window:
            self.settings_window.refresh_notifications()

    def post_notification(self, notification):
        if not isinstance(notification, dict) or self.settings["notifications"] != "authorized":
            return False
        title, body = notification.get("title"), notification.get("body")
        collapse, url = notification.get("collapseID"), notification.get("url") or ""
        if not isinstance(title, str) or not isinstance(collapse, str):
            return False
        if notification.get("subtitle"):
            body = f"{notification['subtitle']}\n{body or ''}"
        n = Gio.Notification.new(title)
        if body:
            n.set_body(body)
        n.set_icon(Gio.ThemedIcon.new(config.APP_ID))
        if notification.get("interruptionLevel") == "passive":
            n.set_priority(Gio.NotificationPriority.LOW)
        if url:
            n.set_default_action_and_target("app.open-url", GLib.Variant("s", url))
        self.send_notification(collapse, n)
        return True

    def _open_notification(self, url):
        window = self.main_window() or self.open_window("/")
        window.present()
        path = url[len(config.APP_URL):] if url.startswith(config.APP_URL) else url
        if path.startswith("/"):
            window.navigate(path)

    def set_unread_count(self, count):
        self.unread = max(0, count)
        self.refresh_badge()

    def refresh_badge(self):
        """Sets the launcher badge through the Unity LauncherEntry API (KDE, Dash to Dock, Plank)."""
        connection = self.get_dbus_connection()
        if connection is None:
            return
        visible = self.settings["show_unread_count"] and self.unread > 0
        connection.emit_signal(
            None,
            "/" + config.APP_ID.replace(".", "/"),
            "com.canonical.Unity.LauncherEntry",
            "Update",
            GLib.Variant(
                "(sa{sv})",
                (f"application://{config.APP_ID}.desktop",
                 {"count": GLib.Variant("x", self.unread), "count-visible": GLib.Variant("b", visible)}),
            ),
        )

    def runner_changed(self):
        runner_id = self.runner.runner_id if self.runner.running else None
        for window in self.windows:
            window.run_js(scripts.runner_state(runner_id))
        if self.settings_window:
            self.settings_window.refresh_runner()

    # ---- downloads ---------------------------------------------------------------------------

    def _download_started(self, _session, download):
        def destination(d, suggested):
            folder = GLib.get_user_special_dir(GLib.UserDirectory.DIRECTORY_DOWNLOAD) or str(Path.home())
            target = Path(folder) / Path(suggested).name
            stem, suffix, n = target.stem, target.suffix, 1
            while target.exists():
                target = target.with_name(f"{stem} ({n}){suffix}")
                n += 1
            d.set_destination(str(target))
            return True

        def finished(d):
            n = Gio.Notification.new("Download Finished")
            n.set_body(Path(GLib.filename_from_uri(d.get_destination())[0] if d.get_destination().startswith("file:") else d.get_destination()).name)
            self.send_notification(None, n)

        download.connect("decide-destination", destination)
        download.connect("finished", finished)

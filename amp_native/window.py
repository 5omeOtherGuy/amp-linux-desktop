"""The Amp window: a WebKitGTK view of ampcode.com in the macOS app's shell mode."""

import json
import logging
from urllib.parse import urlsplit

from gi.repository import Adw, Gdk, Gio, GLib, Gtk, JavaScriptCore, WebKit

from . import config, scripts, signin
from . import dictation, huddle
from .bridge import NativeBridge, Pending
from .confirmation import ConfirmationSheet
from .window_controls import WindowControls

log = logging.getLogger(__name__)

APP_HOST = urlsplit(config.APP_URL).hostname
ALLOW_LIST = [f"{urlsplit(config.APP_URL).scheme}://{APP_HOST}/*", f"https://*.{APP_HOST}/*"]

# Handlers the page calls and the Mac app answers. Names with a reply are awaited by the page.
HANDLERS = [
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
] + (["ampNativeDictation"] if dictation.available() else []) + (["ampNativeHuddle"] if huddle.available() else [])
REPLY_HANDLERS = [
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
]


def is_app_url(uri):
    try:
        parts = urlsplit(uri)
    except ValueError:
        return False
    host = parts.hostname or ""
    local = APP_HOST in ("localhost", "127.0.0.1")
    return (parts.scheme == "https" or (local and parts.scheme == "http")) and (
        host == APP_HOST or host.endswith("." + APP_HOST)
    )


def js_value(reply, context, value):
    reply.return_value(JavaScriptCore.Value.new_from_json(context, json.dumps(value)))


class AmpWindow(NativeBridge, Adw.ApplicationWindow):
    def __init__(self, app, path="/", auxiliary=None):
        super().__init__(application=app, title=config.APP_NAME)
        self.app = app
        self.settings = app.settings
        self.app_state = {"version": 1, "websiteReady": False}
        self.current_url = None
        self._auxiliary = auxiliary
        self._last_press = None
        self._visibility_sequence = 0
        self._confirmation = None

        geometry = self.settings["window"]
        self.set_default_size(geometry["width"], geometry["height"])
        self.set_size_request(480, 360)
        if geometry.get("maximized") and auxiliary is None:
            self.maximize()

        self.webview = self._make_webview()
        self.controls = WindowControls(self._push_metrics)

        overlay = Gtk.Overlay()
        overlay.set_child(self.webview)
        overlay.add_overlay(self.controls.start)
        overlay.add_overlay(self.controls.end)
        overlay.add_overlay(self._make_menu_bar())
        self.toasts = Adw.ToastOverlay(child=overlay)
        self.set_content(self.toasts)

        self._install_shortcuts()
        self.connect("notify::is-active", lambda *_: self.publish_host_visibility())
        self.connect("notify::fullscreened", lambda *_: self._fullscreen_changed())
        self.connect("close-request", self._close_request)
        GLib.timeout_add_seconds(15, self._visibility_tick)

        self.load_path(path)

    # ---- web view --------------------------------------------------------------------------

    def _make_webview(self):
        manager = WebKit.UserContentManager()
        self._manager = manager
        self.reload_user_scripts()
        manager.add_style_sheet(
            WebKit.UserStyleSheet.new(
                scripts.LINUX_CSS,
                WebKit.UserContentInjectedFrames.TOP_FRAME,
                WebKit.UserStyleLevel.AUTHOR,
                ALLOW_LIST,
                None,
            )
        )
        self.theme_sheet = None
        self.apply_host_css(view_ready=False)
        for name in HANDLERS:
            manager.register_script_message_handler(name, None)
            manager.connect(f"script-message-received::{name}", self._on_message, name)
        for name in REPLY_HANDLERS:
            manager.register_script_message_handler_with_reply(name, None)
            manager.connect(f"script-message-with-reply-received::{name}", self._on_reply_message, name)

        view = WebKit.WebView(
            network_session=self.app.network_session,
            user_content_manager=manager,
        )
        settings = view.get_settings()
        settings.set_user_agent(f"{settings.get_user_agent()} {self.app.user_agent_suffix}")
        settings.set_enable_developer_extras(True)
        settings.set_javascript_can_access_clipboard(True)
        settings.set_enable_back_forward_navigation_gestures(True)
        settings.set_media_playback_requires_user_gesture(False)
        settings.set_enable_media_stream(True)
        view.set_zoom_level(config.nearest_zoom(self.settings["zoom"]))
        view.set_background_color(self._background_rgba())

        view.connect("decide-policy", self._decide_policy)
        view.connect("create", self._create_web_view)
        view.connect("permission-request", self._permission_request)
        view.connect("load-changed", self._load_changed)
        view.connect("notify::title", lambda v, _p: self.set_title(v.get_title() or config.APP_NAME))
        view.connect("context-menu", self._context_menu)

        press = Gtk.GestureClick(button=0)
        press.set_propagation_phase(Gtk.PropagationPhase.CAPTURE)
        press.connect("pressed", self._remember_press)
        view.add_controller(press)
        return view

    def _user_scripts(self):
        return [
            scripts.shell_markers(),
            scripts.auxiliary_window(self._auxiliary),
            scripts.window_metrics(0, 0, False, self.settings["zoom"]),
            scripts.notification_permission(self.settings["notifications"]),
            scripts.APP_STATE,
            scripts.NAVIGATION_STATE,
            scripts.EXTERNAL_WINDOW_OPEN,
            scripts.OPEN_IN_NEW_WINDOW,
            scripts.CLIPBOARD,
        ] + ([scripts.HUDDLE] if huddle.available() else []) + ([scripts.runner_state(self.app.runner.runner_id)] if self.app.runner.running else [])

    def reload_user_scripts(self):
        """Builds the user scripts from the current settings; they apply from the next page load."""
        self._manager.remove_all_scripts()
        for source in self._user_scripts():
            self._manager.add_script(
                WebKit.UserScript.new(
                    source,
                    WebKit.UserContentInjectedFrames.TOP_FRAME,
                    WebKit.UserScriptInjectionTime.START,
                    ALLOW_LIST,
                    None,
                )
            )

    def _background_rgba(self):
        rgba = Gdk.RGBA()
        palette = self.app.host_palette
        if palette:
            rgba.parse(palette["windowBackground"])
        else:
            rgba.parse("#0b0d0b" if Adw.StyleManager.get_default().get_dark() else "#fafaf8")
        return rgba

    def apply_host_css(self, view_ready=True):
        """Swaps in the desktop theme's page colors (see theme.py), or removes them."""
        if self.theme_sheet is not None:
            self._manager.remove_style_sheet(self.theme_sheet)
            self.theme_sheet = None
        css = self.app.host_css()
        if css:
            self.theme_sheet = WebKit.UserStyleSheet.new(
                css, WebKit.UserContentInjectedFrames.TOP_FRAME, WebKit.UserStyleLevel.USER, ALLOW_LIST, None
            )
            self._manager.add_style_sheet(self.theme_sheet)
        if view_ready:
            self.webview.set_background_color(self._background_rgba())

    def load_path(self, path):
        if not path.startswith("/"):
            path = "/"
        if path == "/":
            path = "/__native/start"
        self.webview.load_uri(config.APP_URL + path)

    def run_js(self, source, callback=None):
        def done(view, result):
            try:
                value = view.evaluate_javascript_finish(result)
                if callback:
                    callback(json.loads(value.to_json(0) or "null"))
            except GLib.Error as error:
                log.debug("script failed: %s", error.message)
                if callback:
                    callback(None)

        self.webview.evaluate_javascript(source, -1, None, None, None, done)

    def app_command(self, command, fallback=None, **fields):
        def handled(result):
            if result is not True and fallback:
                fallback()

        self.run_js(scripts.app_command(command, **fields), handled)

    def navigate(self, path):
        """Navigates inside the single-page app, or loads the page when the app is not ready."""
        self.app_command("navigate", fallback=lambda: self.load_path(path), url=path, replaceState=False)

    # ---- policy ------------------------------------------------------------------------------

    def _decide_policy(self, _view, decision, kind):
        if kind == WebKit.PolicyDecisionType.RESPONSE:
            if not decision.is_mime_type_supported():
                decision.download()
                return True
            return False
        action = decision.get_navigation_action()
        uri = action.get_request().get_uri()
        if kind == WebKit.PolicyDecisionType.NEW_WINDOW_ACTION:
            decision.ignore()
            # A link the user opens in a new window stays in Amp. A window the page opens from
            # script, such as "Open in Browser" for passkeys, which WebKitGTK cannot do, goes to
            # the browser.
            if is_app_url(uri) and action.get_navigation_type() == WebKit.NavigationType.LINK_CLICKED:
                self.app.open_window(urlsplit(uri).path or "/")
            else:
                self.app.open_in_browser(uri)
            return True
        frame_is_main = action.get_frame_name() in (None, "")
        if not frame_is_main:
            return False
        if signin.is_sign_in_page(uri):
            decision.ignore()
            self.app.sign_in.start(self)
            return True
        if uri.startswith(("about:", "data:", "blob:")) or is_app_url(uri):
            return False
        decision.ignore()
        self.app.open_in_browser(uri)
        return True

    def _create_web_view(self, _view, action):
        uri = action.get_request().get_uri()
        if is_app_url(uri) and action.get_navigation_type() == WebKit.NavigationType.LINK_CLICKED:
            self.app.open_window(urlsplit(uri).path or "/")
        else:
            self.app.open_in_browser(uri)
        return None

    def _permission_request(self, _view, request):
        if isinstance(request, (WebKit.UserMediaPermissionRequest, WebKit.ClipboardPermissionRequest)):
            request.allow()
            return True
        if isinstance(request, WebKit.NotificationPermissionRequest):
            request.allow()
            return True
        return False

    def _load_changed(self, view, event):
        if event == WebKit.LoadEvent.COMMITTED:
            uri = view.get_uri() or ""
            if signin.is_sign_in_page(uri):
                self.load_path("/")
        elif event == WebKit.LoadEvent.FINISHED:
            self.publish_host_visibility()
            self._push_metrics()

    def _context_menu(self, _view, menu, _hit):
        # Navigation entries belong in the View menu, as on the Mac.
        for item in list(menu.get_items()):
            if item.get_stock_action() in (
                WebKit.ContextMenuAction.GO_BACK,
                WebKit.ContextMenuAction.GO_FORWARD,
                WebKit.ContextMenuAction.STOP,
                WebKit.ContextMenuAction.RELOAD,
                WebKit.ContextMenuAction.OPEN_LINK_IN_NEW_WINDOW,
                WebKit.ContextMenuAction.DOWNLOAD_LINK_TO_DISK,
            ):
                menu.remove(item)
        return False

    # ---- bridge ------------------------------------------------------------------------------

    def _on_message(self, _manager, value, name):
        try:
            body = json.loads(value.to_json(0) or "null")
        except ValueError:
            body = None
        # A Huddle message carries a call token; keep it out of the log.
        log.debug("message %s %s", name, "{…}" if name == "ampNativeHuddle" else str(body)[:300])
        getattr(self, "_msg_" + name)(body)

    def _on_reply_message(self, _manager, value, reply, name):
        try:
            body = json.loads(value.to_json(0) or "null")
        except ValueError:
            body = None
        log.debug("message %s %s", name, str(body)[:300])
        try:
            result = getattr(self, "_reply_" + name)(body)
            if isinstance(result, Pending):
                result.attach(reply, value.get_context())
            else:
                js_value(reply, value.get_context(), result)
        except Exception as error:  # the page awaits a reply either way
            log.exception("handler %s failed", name)
            reply.return_error_message(str(error))
        return True

    def _msg_ampNativeWindowDrag(self, body):
        if body == "doubleClick":
            self.toggle_maximized()
        elif isinstance(body, dict) and body.get("action") == "drag":
            self.begin_move()

    def _msg_ampNativeOpenExternalURL(self, body):
        if isinstance(body, str):
            self.app.open_in_browser(body)

    def _msg_ampNativeOpenWindow(self, body):
        if isinstance(body, str):
            self.app.open_window(body)

    def _msg_ampNativeOpenThreadInMainWindow(self, body):
        path = body if isinstance(body, str) else (body or {}).get("url") or (body or {}).get("path")
        if path:
            self.app.main_window().navigate(path)
            self.app.main_window().present()

    def _msg_ampNativeAppSettings(self, body):
        self.app.show_app_settings((body or {}).get("tab") if isinstance(body, dict) else None)

    def _msg_ampNativeEnableNotifications(self, _body):
        self.app.enable_notifications()

    def _msg_ampNativeClipboard(self, body):
        self._write_clipboard(body)

    def _reply_ampNativeClipboardWrite(self, body):
        self._write_clipboard(body)
        return None

    def _write_clipboard(self, body):
        if not isinstance(body, dict) or not isinstance(body.get("text"), str):
            return
        providers = [Gdk.ContentProvider.new_for_bytes("text/plain;charset=utf-8", GLib.Bytes.new(body["text"].encode()))]
        if isinstance(body.get("html"), str):
            providers.insert(0, Gdk.ContentProvider.new_for_bytes("text/html", GLib.Bytes.new(body["html"].encode())))
        self.get_clipboard().set_content(Gdk.ContentProvider.new_union(providers))

    def _msg_ampNativeAppState(self, body):
        if isinstance(body, dict) and body.get("version") == 1:
            self.app_state = body
            self.app.app_state_changed(self)

    def _msg_ampNavigationState(self, body):
        if isinstance(body, str):
            self.current_url = body
            self.app.update_actions()

    def _msg_ampNativeConfirmationSheet(self, body):
        if not isinstance(body, dict) or body.get("version") != 1:
            return
        if self._confirmation:
            self._confirmation.close()
            self._confirmation = None
        if body.get("open") and isinstance(body.get("state"), dict):
            self._confirmation = ConfirmationSheet(self, body["state"], self._confirmation_result)

    def _confirmation_result(self, detail):
        self._confirmation = None
        self.run_js(scripts.confirmation_result(detail))

    def _msg_ampNativeHuddle(self, body):
        if isinstance(body, dict) and body.get("command") == "join":
            self.app.join_huddle(body)

    def _msg_ampNativeMenuBar(self, body):
        if isinstance(body, dict):
            self.app.menu_bar_changed(body)

    def _reply_ampNativeLocalNotification(self, body):
        notification = (body or {}).get("notification") if isinstance(body, dict) else None
        return {"delivered": self.app.post_notification(notification)}

    def _reply_ampNativeGlobalShortcuts(self, body):
        action = (body or {}).get("action")
        if action == "openSettings":
            self.app.show_app_settings("general")
            return None
        return {"version": 1, "shortcuts": self.app.shortcut_list()}

    def _reply_ampNativeClearCache(self, _body):
        self.app.network_session.get_website_data_manager().clear(
            WebKit.WebsiteDataTypes.MEMORY_CACHE | WebKit.WebsiteDataTypes.DISK_CACHE, 0, None, None, None
        )
        return True

    def _reply_ampNativeDiagnostics(self, _body):
        return {
            "app": "amp-native",
            "version": config.VERSION,
            "platform": "linux",
            "webkit": f"{WebKit.get_major_version()}.{WebKit.get_minor_version()}.{WebKit.get_micro_version()}",
            "gtk": f"{Gtk.get_major_version()}.{Gtk.get_minor_version()}.{Gtk.get_micro_version()}",
            "runner": self.app.runner.status(),
        }

    def _reply_ampNativeUnifiedLog(self, _body):
        try:
            return {"log": config.LOG_FILE.read_text()[-200_000:]}
        except OSError:
            return {"log": ""}

    # ---- window behaviour --------------------------------------------------------------------

    def _remember_press(self, gesture, _n, x, y):
        self._last_press = (gesture.get_current_button(), gesture.get_device(), x, y, gesture.get_current_event_time())

    def begin_move(self):
        if not self._last_press:
            return
        button, device, x, y, timestamp = self._last_press
        native = self.webview.get_native()
        ok, nx, ny = self.webview.translate_coordinates(native, x, y)
        surface = native.get_surface()
        if ok and isinstance(surface, Gdk.Toplevel):
            surface.begin_move(device, button or Gdk.BUTTON_PRIMARY, nx, ny, timestamp)

    def toggle_maximized(self):
        if self.is_maximized():
            self.unmaximize()
        else:
            self.maximize()

    def toggle_full_screen(self):
        if self.is_fullscreen():
            self.unfullscreen()
        else:
            self.fullscreen()

    def _fullscreen_changed(self):
        self.controls.set_visible(not self.is_fullscreen())

    def _push_metrics(self):
        left, right = self.controls.insets()
        self.run_js(scripts.window_metrics(left, right, self.is_fullscreen(), self.webview.get_zoom_level()))
        self.menu_revealer.set_margin_start(left)

    def set_zoom(self, factor):
        self.webview.set_zoom_level(factor)
        self._push_metrics()

    def publish_host_visibility(self):
        self._visibility_sequence += 1
        self.run_js(scripts.host_visibility(self._visibility_sequence, self.is_active(), None))

    def _visibility_tick(self):
        if not self.get_mapped() and self.webview.get_parent() is None:
            return False
        self.publish_host_visibility()
        return True

    def _close_request(self, _window):
        if self._auxiliary is None and self is self.app.main_window():
            self.settings.set(
                "window",
                {
                    "width": self.get_width() or self.get_default_size()[0],
                    "height": self.get_height() or self.get_default_size()[1],
                    "maximized": self.is_maximized(),
                },
            )
        return False

    # ---- menu bar --------------------------------------------------------------------------

    def _make_menu_bar(self):
        """The Mac menu bar, shown with Alt the way Linux apps without a menu bar do it."""
        self.menu_bar = Gtk.PopoverMenuBar.new_from_model(self.app.menu_model)
        self.menu_bar.add_css_class("amp-menu-bar")
        self.menu_revealer = Gtk.Revealer(
            transition_type=Gtk.RevealerTransitionType.SLIDE_DOWN,
            halign=Gtk.Align.FILL,
            valign=Gtk.Align.START,
        )
        self.menu_revealer.set_child(self.menu_bar)
        return self.menu_revealer

    def _install_shortcuts(self):
        keys = Gtk.EventControllerKey()
        keys.set_propagation_phase(Gtk.PropagationPhase.CAPTURE)
        keys.connect("key-released", self._key_released)
        keys.connect("key-pressed", self._key_pressed)
        self.add_controller(keys)
        self._alt_alone = False
        self._release_handlers = {}

    def _key_pressed(self, _controller, keyval, keycode, state):
        self._alt_alone = keyval in (Gdk.KEY_Alt_L, Gdk.KEY_Alt_R) and not (
            state & Gtk.accelerator_get_default_mod_mask()
        )
        if keyval == Gdk.KEY_Escape and self.menu_revealer.get_reveal_child():
            self.menu_revealer.set_reveal_child(False)
            return True
        return self.app.handle_shortcut(self, keyval, keycode, state)

    def show_toast(self, title):
        self.toasts.add_toast(Adw.Toast(title=title, timeout=2))

    def on_key_release(self, keyval, callback):
        self._release_handlers[id(callback)] = (keyval, callback)
        return id(callback)

    def disconnect_key_release(self, handler):
        self._release_handlers.pop(handler, None)

    def _key_released(self, _controller, keyval, _code, _state):
        lower = Gdk.keyval_to_lower(keyval)
        for key, callback in list(self._release_handlers.values()):
            if key == lower or keyval in (Gdk.KEY_Control_L, Gdk.KEY_Control_R):
                callback()
        if keyval in (Gdk.KEY_Alt_L, Gdk.KEY_Alt_R) and self._alt_alone:
            self.menu_revealer.set_reveal_child(not self.menu_revealer.get_reveal_child())
        self._alt_alone = False

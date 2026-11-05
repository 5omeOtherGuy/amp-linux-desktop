"""The page's toasts as Adw.Toast (ampNativeToast, version 1).

The page sends { version, op: show | replace | update | dismiss, id, replacementID?, toast?,
rescheduleDismissal? } with toast = { title, description, loader: { progress } | null,
durationMs, actionLabel, secondaryActionLabel, tone, placement }, and listens for
amp:native-toast events { version: 1, type: action | secondaryAction | dismissed, id }.
"""

import json
import math

from gi.repository import Adw, GLib, Gtk, Pango


class ToastCenter:
    def __init__(self, overlay, run_js):
        self._overlay = overlay
        self._run_js = run_js
        self._toasts = {}  # id -> Adw.Toast
        self._silent = set()  # Adw.Toasts dismissed by the page, which already knows

    def handle(self, body):
        if not isinstance(body, dict) or body.get("version") != 1 or not isinstance(body.get("id"), str):
            return
        op, toast_id, content = body.get("op"), body["id"], body.get("toast")
        if op == "show" and isinstance(content, dict):
            self._show(toast_id, content, Adw.ToastPriority.NORMAL)
        elif op == "replace" and isinstance(content, dict) and isinstance(body.get("replacementID"), str):
            self._dismiss(toast_id)
            self._show(body["replacementID"], content, Adw.ToastPriority.HIGH)
        elif op == "update" and isinstance(content, dict) and toast_id in self._toasts:
            if body.get("rescheduleDismissal"):
                # A shown Adw.Toast keeps the timeout it started with, so show it again.
                self._dismiss(toast_id)
                self._show(toast_id, content, Adw.ToastPriority.HIGH)
            else:
                _apply(self._toasts[toast_id], content, self._secondary_handler(toast_id))
        elif op == "dismiss":
            self._dismiss(toast_id)

    def _show(self, toast_id, content, priority):
        toast = Adw.Toast(priority=priority)
        _apply(toast, content, self._secondary_handler(toast_id))
        toast.connect("button-clicked", lambda _t: self._emit("action", toast_id))
        toast.connect("dismissed", self._dismissed, toast_id)
        self._toasts[toast_id] = toast
        self._overlay.add_toast(toast)

    def _secondary_handler(self, toast_id):
        def clicked(_button):
            self._emit("secondaryAction", toast_id)
            toast = self._toasts.get(toast_id)
            if toast:
                toast.dismiss()

        return clicked

    def _dismiss(self, toast_id):
        toast = self._toasts.pop(toast_id, None)
        if toast:
            self._silent.add(toast)
            toast.dismiss()

    def _dismissed(self, toast, toast_id):
        if toast in self._silent:
            self._silent.discard(toast)
            return
        if self._toasts.get(toast_id) is toast:
            del self._toasts[toast_id]
        self._emit("dismissed", toast_id)

    def _emit(self, kind, toast_id):
        detail = {"version": 1, "type": kind, "id": toast_id}
        self._run_js(f"window.dispatchEvent(new CustomEvent('amp:native-toast', {{ detail: {json.dumps(detail)} }}))")


def _apply(toast, content, on_secondary):
    title = str(content.get("title") or "")
    description = content.get("description")
    loader = content.get("loader")
    secondary = content.get("secondaryActionLabel")
    duration = content.get("durationMs")

    toast.set_timeout(0 if not isinstance(duration, (int, float)) else max(1, math.ceil(duration / 1000)))
    toast.set_button_label(content.get("actionLabel") or None)
    if description or isinstance(loader, dict) or secondary:
        toast.set_custom_title(_title_widget(title, description, loader, secondary, on_secondary))
    else:
        toast.set_custom_title(None)
        toast.set_title(GLib.markup_escape_text(title))  # Adw.Toast titles are Pango markup


def _title_widget(title, description, loader, secondary, on_secondary):
    row = Gtk.Box(spacing=12, valign=Gtk.Align.CENTER)
    text = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2, hexpand=True, valign=Gtk.Align.CENTER)
    heading = Gtk.Label(label=title, xalign=0, wrap=True, wrap_mode=Pango.WrapMode.WORD_CHAR, max_width_chars=48)
    heading.add_css_class("heading")
    text.append(heading)
    if description:
        body = Gtk.Label(label=str(description), xalign=0, wrap=True, max_width_chars=48)
        body.add_css_class("caption")
        body.add_css_class("dim-label")
        text.append(body)
    if isinstance(loader, dict):
        progress = loader.get("progress")
        bar = Gtk.ProgressBar(margin_top=4)
        if isinstance(progress, (int, float)):
            bar.set_fraction(max(0.0, min(1.0, float(progress))))
        else:  # null: indeterminate
            _pulse_while_shown(bar)
        text.append(bar)
    row.append(text)
    if secondary:
        button = Gtk.Button(label=str(secondary), valign=Gtk.Align.CENTER)
        button.add_css_class("flat")
        button.connect("clicked", on_secondary)
        row.append(button)
    return row


def _pulse_while_shown(bar):
    timer = []

    def start(_bar):
        timer.append(GLib.timeout_add(120, lambda: bar.pulse() or True))

    def stop(_bar):
        while timer:
            GLib.source_remove(timer.pop())

    bar.connect("map", start)
    bar.connect("unmap", stop)

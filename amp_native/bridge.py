"""Mac bridge handlers for files, toasts, the Puck window, and local project import.

AmpWindow mixes this in; window.py registers the handler names. Payload and reply shapes follow
the page's bridge module on ampcode.com (the `ampNative*` callers in /_app/immutable/chunks).
"""

import json
import logging

from gi.repository import JavaScriptCore

from . import files, local_projects
from .toasts import ToastCenter

log = logging.getLogger(__name__)

PUCK_PATH = "/puck-window"
PUCK_SIZE = (480, 720)


class Pending:
    """A reply the page awaits that a handler sends later. A `_reply_` handler returns one and
    calls resolve or reject once its work finishes."""

    def __init__(self):
        self._reply = None
        self._context = None
        self._outcome = None

    def attach(self, reply, context):
        self._reply, self._context = reply, context
        if self._outcome:
            self._send(*self._outcome)

    def resolve(self, value=None):
        self._send(True, value)

    def reject(self, message):
        self._send(False, message)

    def _send(self, ok, payload):
        if self._reply is None:
            self._outcome = self._outcome or (ok, payload)
            return
        reply, self._reply = self._reply, None
        if ok:
            reply.return_value(JavaScriptCore.Value.new_from_json(self._context, json.dumps(payload)))
        else:
            reply.return_error_message(str(payload))


class NativeBridge:
    # ---- files -------------------------------------------------------------------------------

    def _reply_ampNativeOpenInEditor(self, body):
        files.open_in_editor(body)
        return None

    def _reply_ampNativeShowInFinder(self, body):
        pending = Pending()
        files.show_in_file_manager(body, pending.resolve, pending.reject)
        return pending

    def _msg_ampNativeSaveFile(self, body):
        try:
            files.save_file(self.app, body)
        except Exception as error:
            log.warning("could not save file: %s", error)
            name = body.get("fileName") if isinstance(body, dict) else None
            self.show_toast(f"Could not save {name or 'the file'}")

    def _reply_ampNativeSaveFileStream(self, body):
        return files.save_stream(self.app, body)

    def _reply_ampNativePasteboardImages(self, body):
        pending = Pending()
        files.read_pasteboard_images(self.get_clipboard(), pending.resolve, pending.reject)
        return pending

    # ---- toasts ------------------------------------------------------------------------------

    def _msg_ampNativeToast(self, body):
        if not hasattr(self, "_toast_center"):
            self._toast_center = ToastCenter(self.toasts, self.run_js)
        self._toast_center.handle(body)

    # ---- dictation ---------------------------------------------------------------------------

    def _msg_ampNativeDictation(self, body):
        if not hasattr(self, "_dictation"):
            from .dictation import Dictation  # GStreamer loads only once the page dictates

            self._dictation = Dictation(self.run_js)
            self.connect("close-request", lambda *_: self._dictation.close())
        self._dictation.handle(body)

    # ---- Puck window -------------------------------------------------------------------------

    def _msg_ampNativeOpenPuckWindow(self, body):
        path = _puck_path(body)
        window = self._puck_window()
        if window is None:
            self._new_puck_window(path)
        else:
            if path != PUCK_PATH:
                window.navigate(path)
            window.present()

    def _msg_ampNativeTogglePuckWindow(self, body):
        window = self._puck_window()
        if window is None:
            self._new_puck_window(_puck_path(body))
        elif window is self or window.is_active():
            window.close()
        else:
            window.present()

    def _puck_window(self):
        return next(
            (w for w in self.app.windows if w._auxiliary == "puck" and w.get_application() is not None), None
        )

    def _new_puck_window(self, path):
        # Built here rather than with app.open_window so the default size applies before the
        # window maps.
        window = type(self)(self.app, path, "puck")
        window.set_default_size(*PUCK_SIZE)
        self.app.windows.append(window)
        self.app._add_window_actions(window)
        window.present()

    # ---- local projects ----------------------------------------------------------------------

    def _reply_ampNativeImportLocalProjects(self, body):
        pending = Pending()
        local_projects.handle(self, body, pending.resolve)
        return pending


def _puck_path(body):
    if isinstance(body, str) and (body == PUCK_PATH or body.startswith(PUCK_PATH + "/")):
        return body
    return PUCK_PATH

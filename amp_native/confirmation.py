"""Native confirmation sheets for the page's confirmation dialogs (protocol version v2)."""

from gi.repository import Adw, Gtk


class ConfirmationSheet:
    def __init__(self, window, state, on_result):
        self._state = state
        self._on_result = on_result
        self._done = False
        scope = state.get("scopeID")

        dialog = Adw.MessageDialog(transient_for=window, modal=True)
        dialog.set_heading(state.get("title") or "")
        body = state.get("description") or ""
        if state.get("warning"):
            body = f"{body}\n\n{state['warning']}" if body else state["warning"]
        dialog.set_body(body)

        extra = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        self._entry = None
        message = state.get("message")
        phrase = state.get("confirmationPhrase")
        if message or phrase:
            self._entry = Gtk.Entry()
            if message:
                self._entry.set_text(message.get("text") or "")
                if message.get("placeholder"):
                    self._entry.set_placeholder_text(message["placeholder"])
            else:
                self._entry.set_placeholder_text(phrase)
            extra.append(self._entry)
        self._toggle = None
        toggle = state.get("toggle")
        if toggle:
            self._toggle = Gtk.CheckButton(label=toggle.get("label") or "", active=bool(toggle.get("checked")))
            self._toggle.set_sensitive(not toggle.get("disabled"))
            extra.append(self._toggle)
        if state.get("hint"):
            hint = Gtk.Label(label=state["hint"], wrap=True, xalign=0)
            hint.add_css_class("dim-label")
            hint.add_css_class("caption")
            extra.append(hint)
        if extra.get_first_child():
            dialog.set_extra_child(extra)

        if state.get("cancelLabel"):
            dialog.add_response("cancel", state["cancelLabel"])
            dialog.set_close_response("cancel")
        confirm = state.get("confirmLabel") or "OK"
        dialog.add_response("confirm", confirm)
        dialog.set_response_appearance(
            "confirm",
            Adw.ResponseAppearance.DESTRUCTIVE if state.get("destructive") else Adw.ResponseAppearance.SUGGESTED,
        )
        dialog.set_default_response("confirm")

        if self._entry is not None:
            def validate(*_):
                text = self._entry.get_text()
                ok = (phrase is None or text == phrase) and (
                    not message or message.get("optional") or text.strip() != ""
                )
                dialog.set_response_enabled("confirm", ok)

            self._entry.connect("changed", validate)
            self._entry.connect("activate", lambda *_: dialog.get_response_enabled("confirm") and dialog.response("confirm"))
            validate()

        def responded(_dialog, response):
            if self._done:
                return
            self._done = True
            if response == "confirm":
                self._on_result(
                    {
                        "version": 1,
                        "type": "confirm",
                        "scopeID": scope,
                        "message": self._entry.get_text() if self._entry else "",
                        "toggleChecked": self._toggle.get_active() if self._toggle else False,
                        "choiceID": None,
                        "values": None,
                        "checked": None,
                    }
                )
            else:
                self._on_result({"version": 1, "type": "cancel", "scopeID": scope})

        dialog.connect("response", responded)
        self._dialog = dialog
        dialog.present()

    def close(self):
        """The page closed the sheet itself; dismiss it without answering."""
        self._done = True
        self._dialog.close()

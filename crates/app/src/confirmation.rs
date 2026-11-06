//! Native confirmation sheets for the page's confirmation dialogs (protocol version v2).

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use serde_json::{Value, json};

pub struct ConfirmationSheet {
    dialog: adw::MessageDialog,
    done: Rc<Cell<bool>>,
}

fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

impl ConfirmationSheet {
    /// Shows the sheet; `on_result` receives the page's amp:native-confirmation-sheet detail.
    pub fn new(window: &impl IsA<gtk::Window>, state: &Value, on_result: impl Fn(Value) + 'static) -> Self {
        let scope = state.get("scopeID").cloned().unwrap_or(Value::Null);
        let dialog = adw::MessageDialog::builder().transient_for(window).modal(true).build();
        dialog.set_heading(Some(string(state, "title").unwrap_or("")));
        let mut body = string(state, "description").unwrap_or("").to_string();
        if let Some(warning) = string(state, "warning") {
            body = if body.is_empty() { warning.to_string() } else { format!("{body}\n\n{warning}") };
        }
        dialog.set_body(&body);

        let extra = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let message = state.get("message").filter(|m| amp_common::config::truthy(m));
        let phrase = state.get("confirmationPhrase").and_then(Value::as_str).map(String::from);
        let entry = (message.is_some() || phrase.as_deref().is_some_and(|p| !p.is_empty())).then(|| {
            let entry = gtk::Entry::new();
            match message {
                Some(message) => {
                    entry.set_text(string(message, "text").unwrap_or(""));
                    if let Some(placeholder) = string(message, "placeholder") {
                        entry.set_placeholder_text(Some(placeholder));
                    }
                }
                None => entry.set_placeholder_text(phrase.as_deref()),
            }
            extra.append(&entry);
            entry
        });
        let toggle = state.get("toggle").filter(|t| amp_common::config::truthy(t)).map(|toggle| {
            let check = gtk::CheckButton::with_label(string(toggle, "label").unwrap_or(""));
            check.set_active(toggle.get("checked").is_some_and(amp_common::config::truthy));
            check.set_sensitive(!toggle.get("disabled").is_some_and(amp_common::config::truthy));
            extra.append(&check);
            check
        });
        if let Some(hint) = string(state, "hint") {
            let hint = gtk::Label::builder().label(hint).wrap(true).xalign(0.0).build();
            hint.add_css_class("dim-label");
            hint.add_css_class("caption");
            extra.append(&hint);
        }
        if extra.first_child().is_some() {
            dialog.set_extra_child(Some(&extra));
        }

        if let Some(cancel) = string(state, "cancelLabel") {
            dialog.add_response("cancel", cancel);
            dialog.set_close_response("cancel");
        }
        dialog.add_response("confirm", string(state, "confirmLabel").unwrap_or("OK"));
        let destructive = state.get("destructive").is_some_and(amp_common::config::truthy);
        dialog.set_response_appearance(
            "confirm",
            if destructive { adw::ResponseAppearance::Destructive } else { adw::ResponseAppearance::Suggested },
        );
        dialog.set_default_response(Some("confirm"));

        if let Some(entry) = &entry {
            let optional = message.and_then(|m| m.get("optional")).is_some_and(amp_common::config::truthy);
            let has_message = message.is_some();
            let validate = {
                let dialog = dialog.clone();
                let phrase = phrase.clone();
                move |entry: &gtk::Entry| {
                    let text = entry.text();
                    let ok = phrase.as_deref().is_none_or(|p| text == p)
                        && (!has_message || optional || !text.trim().is_empty());
                    dialog.set_response_enabled("confirm", ok);
                }
            };
            validate(entry);
            entry.connect_changed(validate);
            let d = dialog.clone();
            entry.connect_activate(move |_| {
                if d.is_response_enabled("confirm") {
                    d.response("confirm");
                }
            });
        }

        let done = Rc::new(Cell::new(false));
        let finished = done.clone();
        dialog.connect_response(None, move |_, response| {
            if finished.replace(true) {
                return;
            }
            if response == "confirm" {
                on_result(json!({
                    "version": 1,
                    "type": "confirm",
                    "scopeID": scope,
                    "message": entry.as_ref().map(|e| e.text().to_string()).unwrap_or_default(),
                    "toggleChecked": toggle.as_ref().is_some_and(|t| t.is_active()),
                    "choiceID": null,
                    "values": null,
                    "checked": null,
                }));
            } else {
                on_result(json!({"version": 1, "type": "cancel", "scopeID": scope}));
            }
        });
        dialog.present();
        Self { dialog, done }
    }

    /// The page closed the sheet itself; dismiss it without answering.
    pub fn close(&self) {
        self.done.set(true);
        self.dialog.close();
    }
}

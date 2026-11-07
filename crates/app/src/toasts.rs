//! The page's toasts as Adw.Toast (ampNativeToast, version 1).
//!
//! The page sends { version, op: show | replace | update | dismiss, id, replacementID?, toast?,
//! rescheduleDismissal? } with toast = { title, description, loader: { progress } | null,
//! durationMs, actionLabel, secondaryActionLabel, tone, placement }, and listens for
//! amp:native-toast events { version: 1, type: action | secondaryAction | dismissed, id }.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::glib;
use serde_json::{Value, json};

use amp_common::scripts;

struct Inner {
    overlay: adw::ToastOverlay,
    run_js: Box<dyn Fn(String)>,
    toasts: RefCell<HashMap<String, adw::Toast>>,
    /// Toasts dismissed by the page, which already knows.
    silent: RefCell<Vec<adw::Toast>>,
}

#[derive(Clone)]
pub struct ToastCenter(Rc<Inner>);

impl ToastCenter {
    pub fn new(overlay: adw::ToastOverlay, run_js: impl Fn(String) + 'static) -> Self {
        Self(Rc::new(Inner {
            overlay,
            run_js: Box::new(run_js),
            toasts: RefCell::default(),
            silent: RefCell::default(),
        }))
    }

    pub fn handle(&self, body: &Value) {
        if body.get("version") != Some(&json!(1)) {
            return;
        }
        let Some(id) = body.get("id").and_then(Value::as_str) else { return };
        let content = body.get("toast").filter(|t| t.is_object());
        match (body.get("op").and_then(Value::as_str), content) {
            (Some("show"), Some(content)) => self.show(id, content, adw::ToastPriority::Normal),
            (Some("replace"), Some(content)) => {
                if let Some(replacement) = body.get("replacementID").and_then(Value::as_str) {
                    self.dismiss(id);
                    self.show(replacement, content, adw::ToastPriority::High);
                }
            }
            (Some("update"), Some(content)) => {
                let Some(toast) = self.0.toasts.borrow().get(id).cloned() else { return };
                if config_truthy(body.get("rescheduleDismissal")) {
                    // A shown Adw.Toast keeps the timeout it started with, so show it again.
                    self.dismiss(id);
                    self.show(id, content, adw::ToastPriority::High);
                } else {
                    apply(&toast, content, self.secondary_handler(id));
                }
            }
            (Some("dismiss"), _) => self.dismiss(id),
            _ => {}
        }
    }

    fn show(&self, id: &str, content: &Value, priority: adw::ToastPriority) {
        let toast = adw::Toast::builder().priority(priority).build();
        apply(&toast, content, self.secondary_handler(id));
        let weak = Rc::downgrade(&self.0);
        let toast_id = id.to_string();
        toast.connect_button_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                emit(&inner, "action", &toast_id);
            }
        });
        let weak = Rc::downgrade(&self.0);
        let toast_id = id.to_string();
        toast.connect_dismissed(move |toast| {
            if let Some(inner) = weak.upgrade() {
                dismissed(&inner, toast, &toast_id);
            }
        });
        self.0.toasts.borrow_mut().insert(id.to_string(), toast.clone());
        self.0.overlay.add_toast(toast);
    }

    fn secondary_handler(&self, id: &str) -> impl Fn() + 'static {
        let weak: Weak<Inner> = Rc::downgrade(&self.0);
        let id = id.to_string();
        move || {
            if let Some(inner) = weak.upgrade() {
                emit(&inner, "secondaryAction", &id);
                let toast = inner.toasts.borrow().get(&id).cloned();
                if let Some(toast) = toast {
                    toast.dismiss();
                }
            }
        }
    }

    fn dismiss(&self, id: &str) {
        let toast = self.0.toasts.borrow_mut().remove(id);
        if let Some(toast) = toast {
            self.0.silent.borrow_mut().push(toast.clone());
            toast.dismiss();
        }
    }
}

fn config_truthy(value: Option<&Value>) -> bool {
    value.is_some_and(amp_common::config::truthy)
}

fn dismissed(inner: &Inner, toast: &adw::Toast, id: &str) {
    {
        let mut silent = inner.silent.borrow_mut();
        if let Some(index) = silent.iter().position(|t| t == toast) {
            silent.remove(index);
            return;
        }
    }
    {
        let mut toasts = inner.toasts.borrow_mut();
        if toasts.get(id) == Some(toast) {
            toasts.remove(id);
        }
    }
    emit(inner, "dismissed", id);
}

fn emit(inner: &Inner, kind: &str, id: &str) {
    (inner.run_js)(scripts::custom_event("amp:native-toast", &json!({"version": 1, "type": kind, "id": id})));
}

fn text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Null => None,
        Value::String(s) if s.is_empty() => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

fn apply(toast: &adw::Toast, content: &Value, on_secondary: impl Fn() + 'static) {
    let title = text(content.get("title")).unwrap_or_default();
    let description = text(content.get("description"));
    let loader = content.get("loader").filter(|l| l.is_object());
    let secondary = text(content.get("secondaryActionLabel"));
    let timeout =
        content.get("durationMs").and_then(Value::as_f64).map_or(0, |ms| ((ms / 1000.0).ceil() as u32).max(1));
    toast.set_timeout(timeout);
    toast.set_button_label(text(content.get("actionLabel")).as_deref());
    if description.is_some() || loader.is_some() || secondary.is_some() {
        toast.set_custom_title(Some(&title_widget(&title, description, loader, secondary, on_secondary)));
    } else {
        toast.set_custom_title(gtk::Widget::NONE);
        // Adw.Toast titles are Pango markup.
        toast.set_title(&glib::markup_escape_text(&title));
    }
}

fn title_widget(
    title: &str,
    description: Option<String>,
    loader: Option<&Value>,
    secondary: Option<String>,
    on_secondary: impl Fn() + 'static,
) -> gtk::Widget {
    let row = gtk::Box::builder().spacing(12).valign(gtk::Align::Center).build();
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    let heading = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .max_width_chars(48)
        .build();
    heading.add_css_class("heading");
    text.append(&heading);
    if let Some(description) = description {
        let body = gtk::Label::builder().label(description).xalign(0.0).wrap(true).max_width_chars(48).build();
        body.add_css_class("caption");
        body.add_css_class("dim-label");
        text.append(&body);
    }
    if let Some(loader) = loader {
        let bar = gtk::ProgressBar::builder().margin_top(4).build();
        match loader.get("progress").and_then(Value::as_f64) {
            Some(progress) => bar.set_fraction(progress.clamp(0.0, 1.0)),
            None => pulse_while_shown(&bar), // null: indeterminate
        }
        text.append(&bar);
    }
    row.append(&text);
    if let Some(secondary) = secondary {
        let button = gtk::Button::builder().label(secondary).valign(gtk::Align::Center).build();
        button.add_css_class("flat");
        button.connect_clicked(move |_| on_secondary());
        row.append(&button);
    }
    row.upcast()
}

fn pulse_while_shown(bar: &gtk::ProgressBar) {
    let timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let start = timer.clone();
    bar.connect_map(move |bar| {
        let bar = bar.downgrade();
        *start.borrow_mut() =
            Some(glib::timeout_add_local(std::time::Duration::from_millis(120), move || match bar.upgrade() {
                Some(bar) => {
                    bar.pulse();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            }));
    });
    bar.connect_unmap(move |_| {
        if let Some(id) = timer.borrow_mut().take() {
            id.remove();
        }
    });
}

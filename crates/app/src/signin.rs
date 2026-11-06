//! Sign-in through the system browser, and OAuth callbacks relayed back into the page.
//!
//! Same protocol as the official Linux build: PKCE against /auth/app/authorize with the
//! com.ampcode.amp.linux.auth callback scheme, then the code is redeemed at /auth/app/redeem from
//! inside the page so the session cookie lands in the web view.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};

use amp_common::config::{APP_URL, SIGN_IN_SCHEME};
use base64::Engine;
use gtk::{glib, prelude::*};
use serde_json::json;
use url::Url;
use webkit::prelude::*;

use crate::app::App;
use crate::window::AmpWindow;

const CALLBACK_PARAMETERS: [&str; 4] = ["state", "code", "error", "error_description"];
const MAX_PENDING_RELAYS: usize = 8;

fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the system's random source");
    b64(&bytes)
}

pub fn is_sign_in_page(uri: &str) -> bool {
    Url::parse(uri).is_ok_and(|u| u.origin().ascii_serialization() == *APP_URL && u.path() == "/auth/sign-in")
}

/// The query as a map, or None when a parameter repeats.
fn query(url: &Url) -> Option<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    for (k, v) in url.query_pairs() {
        if map.insert(k.into_owned(), v.into_owned()).is_some() {
            return None;
        }
    }
    Some(map)
}

struct Pending {
    state: String,
    verifier: String,
    window: Weak<AmpWindow>,
}

#[derive(Default)]
pub struct SignIn {
    // Every Sign In click opens its own browser tab; accept whichever one the user finishes.
    pending: RefCell<Vec<Pending>>,
    relay_states: RefCell<Vec<String>>,
}

impl SignIn {
    /// Opens the sign-in in the browser. `operation` is the account operation the page asks to
    /// finish there (ampNativeAuthenticate), such as adding another account.
    pub fn start(&self, app: &App, window: &Rc<AmpWindow>, operation: Option<&str>) {
        let verifier = random_token();
        let state = random_token();
        let challenge = b64(&glib::Checksum::new(glib::ChecksumType::Sha256)
            .map(|mut c| {
                c.update(verifier.as_bytes());
                c.digest()
            })
            .unwrap_or_default());
        {
            let mut pending = self.pending.borrow_mut();
            pending.push(Pending { state: state.clone(), verifier, window: Rc::downgrade(window) });
            let excess = pending.len().saturating_sub(MAX_PENDING_RELAYS);
            pending.drain(..excess);
        }
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("challenge", &challenge).append_pair("state", &state).append_pair("scheme", SIGN_IN_SCHEME);
        if let Some(operation) = operation {
            query.append_pair("operation", operation);
        }
        let query = query.finish();
        app.launch_uri(&format!("{}/auth/app/authorize?{query}", *APP_URL));
    }

    /// Records the state of an OAuth flow the page opened in the browser, to accept its callback.
    pub fn remember_relay(&self, url: &str) {
        let Ok(url) = Url::parse(url) else { return };
        let states: Vec<String> =
            url.query_pairs().filter(|(k, _)| k == "state").map(|(_, v)| v.into_owned()).collect();
        if let [state] = states.as_slice()
            && !state.is_empty()
        {
            let mut relays = self.relay_states.borrow_mut();
            relays.retain(|s| s != state);
            relays.push(state.clone());
            let excess = relays.len().saturating_sub(MAX_PENDING_RELAYS);
            relays.drain(..excess);
        }
    }

    pub fn handle_callback(&self, app: &Rc<App>, uri: &str) -> bool {
        let Ok(url) = Url::parse(uri) else { return false };
        if url.scheme() != SIGN_IN_SCHEME
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.fragment().is_some()
        {
            return false;
        }
        if url.host_str() == Some("auth") { self.complete_sign_in(app, &url) } else { self.complete_relay(app, &url) }
    }

    fn complete_sign_in(&self, app: &Rc<App>, url: &Url) -> bool {
        let Some(query) = query(url).filter(|_| url.path() == "/callback") else { return false };
        let Some(state) = query.get("state") else { return false };
        let pending = {
            let mut list = self.pending.borrow_mut();
            let Some(index) = list.iter().position(|p| &p.state == state) else { return false };
            list.remove(index)
        };
        let window = pending
            .window
            .upgrade()
            .filter(|w| w.window.is_mapped())
            .or_else(|| app.main_window())
            .unwrap_or_else(|| app.open_window("/", None));
        window.window.present();
        if let (Some(code), Some(flow), 3) = (query.get("code"), query.get("flow"), query.len()) {
            let body = json!({"code": code, "flow": flow, "verifier": pending.verifier}).to_string();
            let script = format!(
                r#"
(async () => {{
  const response = await fetch('/auth/app/redeem', {{
    method: 'POST',
    headers: {{ 'Content-Type': 'application/json' }},
    credentials: 'same-origin',
    body: {},
  }});
  location.assign(response.redirected ? response.url : '/');
}})();
"#,
                json!(body)
            );
            let loaded = !window.webview.is_loading()
                && window.webview.uri().is_some_and(|uri| uri.starts_with(APP_URL.as_str()));
            if !loaded {
                // A window opened for the callback (Amp had none) has no ampcode.com page yet, so
                // the relative fetch would fail and waste the single-use code: redeem once its
                // page has loaded.
                let handler = Rc::new(RefCell::new(None));
                let slot = handler.clone();
                let weak = Rc::downgrade(&window);
                let id = window.webview.connect_load_changed(move |view, event| {
                    if event != webkit::LoadEvent::Finished {
                        return;
                    }
                    if let Some(id) = slot.borrow_mut().take() {
                        view.disconnect(id);
                    }
                    if let Some(window) = weak.upgrade() {
                        window.run_js(&script);
                    }
                });
                *handler.borrow_mut() = Some(id);
            } else {
                window.run_js(&script);
            }
        } else {
            window.load_path("/");
        }
        true
    }

    fn complete_relay(&self, app: &Rc<App>, url: &Url) -> bool {
        let query = query(url).unwrap_or_default();
        let path = url.path();
        let kind = match url.host_str() {
            Some("mcp") if path == "/callback" => Some("mcp"),
            Some("connect")
                if (path.starts_with("/auth/") || path.starts_with("/api/"))
                    && path.split('/').skip(1).all(|s| !matches!(s, "" | "." | "..")) =>
            {
                Some("connect")
            }
            _ => None,
        };
        let Some(kind) = kind else { return false };
        let Some(state) = query.get("state").filter(|s| !s.is_empty()) else { return false };
        if !self.relay_states.borrow().contains(state)
            || !query.keys().all(|k| CALLBACK_PARAMETERS.contains(&k.as_str()))
        {
            return false;
        }
        self.relay_states.borrow_mut().retain(|s| s != state);
        let path = if kind == "mcp" { "/auth/mcp/callback" } else { path };
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in &query {
            serializer.append_pair(k, v);
        }
        serializer.append_pair("appCallback", "1");
        let target = format!("{}{path}?{}", *APP_URL, serializer.finish());
        let command = if kind == "mcp" { "completeMCPOAuth" } else { "completeConnectOAuth" };
        let window = app.main_window().unwrap_or_else(|| app.open_window("/", None));
        window.window.present();
        let fallback_window = window.clone();
        let fallback_target = target.clone();
        window.app_command(
            command,
            json!({"url": target}),
            Some(Box::new(move || fallback_window.webview.load_uri(&fallback_target))),
        );
        true
    }
}

"""Sign-in through the system browser, and OAuth callbacks relayed back into the page.

Same protocol as the official Linux build: PKCE against /auth/app/authorize with the
com.ampcode.amp.linux.auth callback scheme, then the code is redeemed at /auth/app/redeem from
inside the page so the session cookie lands in the web view.
"""

import base64
import hashlib
import json
import secrets
from collections import OrderedDict
from urllib.parse import parse_qsl, urlencode, urlsplit

from . import config

CALLBACK_PARAMETERS = {"state", "code", "error", "error_description"}
MAX_PENDING_RELAYS = 8


def _b64(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def is_sign_in_page(uri):
    parts = urlsplit(uri)
    return f"{parts.scheme}://{parts.netloc}" == config.APP_URL and parts.path == "/auth/sign-in"


def _query(parts):
    pairs = parse_qsl(parts.query, keep_blank_values=True)
    names = [name for name, _ in pairs]
    if len(set(names)) != len(names):
        return None
    return dict(pairs)


class SignIn:
    def __init__(self, app):
        self.app = app
        # Every Sign In click opens its own browser tab; accept whichever one the user finishes.
        self.pending = OrderedDict()
        self.relay_states = OrderedDict()

    def start(self, window):
        verifier = _b64(secrets.token_bytes(32))
        state = _b64(secrets.token_bytes(32))
        self.pending[state] = {"verifier": verifier, "window": window}
        while len(self.pending) > MAX_PENDING_RELAYS:
            self.pending.popitem(last=False)
        challenge = _b64(hashlib.sha256(verifier.encode()).digest())
        query = urlencode({"challenge": challenge, "state": state, "scheme": config.SIGN_IN_SCHEME})
        self.app.launch_uri(f"{config.APP_URL}/auth/app/authorize?{query}")

    def remember_relay(self, url):
        """Records the state of an OAuth flow the page opened in the browser, to accept its callback."""
        states = [v for k, v in parse_qsl(urlsplit(url).query) if k == "state"]
        if len(states) == 1 and states[0]:
            self.relay_states.pop(states[0], None)
            self.relay_states[states[0]] = True
            while len(self.relay_states) > MAX_PENDING_RELAYS:
                self.relay_states.popitem(last=False)

    def handle_callback(self, uri):
        parts = urlsplit(uri)
        if parts.scheme != config.SIGN_IN_SCHEME or parts.username or parts.password or parts.port or parts.fragment:
            return False
        if parts.netloc == "auth":
            return self._complete_sign_in(parts)
        return self._complete_relay(parts)

    def _complete_sign_in(self, parts):
        query = _query(parts)
        if parts.path != "/callback" or query is None or query.get("state") not in self.pending:
            return False
        pending = self.pending.pop(query["state"])
        window = pending["window"] if pending["window"].get_mapped() else self.app.main_window()
        window.present()
        if "code" in query and "flow" in query and len(query) == 3:
            body = json.dumps({"code": query["code"], "flow": query["flow"], "verifier": pending["verifier"]})
            window.run_js(
                f"""
(async () => {{
  const response = await fetch('/auth/app/redeem', {{
    method: 'POST',
    headers: {{ 'Content-Type': 'application/json' }},
    credentials: 'same-origin',
    body: {json.dumps(body)},
  }});
  location.assign(response.redirected ? response.url : '/');
}})();
"""
            )
        else:
            window.load_path("/")
        return True

    def _complete_relay(self, parts):
        query = _query(parts)
        kind = None
        if parts.netloc == "mcp" and parts.path == "/callback":
            kind = "mcp"
        elif parts.netloc == "connect" and parts.path.startswith(("/auth/", "/api/")):
            segments = parts.path.split("/")[1:]
            if all(s not in ("", ".", "..") for s in segments):
                kind = "connect"
        state = (query or {}).get("state")
        if not kind or not state or state not in self.relay_states or not set(query) <= CALLBACK_PARAMETERS:
            return False
        del self.relay_states[state]
        path = "/auth/mcp/callback" if kind == "mcp" else parts.path
        url = f"{config.APP_URL}{path}?{urlencode({**query, 'appCallback': '1'})}"
        command = "completeMCPOAuth" if kind == "mcp" else "completeConnectOAuth"
        window = self.app.main_window()
        window.present()
        window.app_command(command, fallback=lambda: window.webview.load_uri(url), url=url)
        return True

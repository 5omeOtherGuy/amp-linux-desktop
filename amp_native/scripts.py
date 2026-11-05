"""Scripts the shell injects into ampcode.com.

They mirror the user scripts embedded in the macOS app (Amp.app/Contents/MacOS/Amp, build 1104):
the page reads `data-amp-shell="macos-v1"` and the other dataset markers to switch to its native
desktop layout, and talks back through `window.webkit.messageHandlers.<name>`, which WebKitGTK
provides the same way WKWebView does.
"""

import json


def shell_markers():
    return f"""
(() => {{
  const d = document.documentElement.dataset;
  d.ampShell = 'macos-v1';
  d.ampNativeCloseTab = 'v1';
  d.ampNativeConfirmationSheet = 'v1';
  // v2: title, description, editable message, confirmation phrase, and a checkbox. The page keeps
  // dialogs with choices, fields, or illustrations (v3 and later) in its own UI.
  d.ampNativeConfirmationAlert = 'v2';
  d.ampMacUpdateReady = 'false';
  d.ampMacUpdateOffer = 'none';
  d.ampMacUpdateRelaunchPhase = 'idle';
}})();
"""


def auxiliary_window(kind):
    return f"""
(() => {{
  let kind = {json.dumps(kind)};
  try {{ kind = sessionStorage.getItem('ampAuxiliaryWindowKind') || kind }} catch {{}}
  if (kind) document.documentElement.dataset.ampAuxiliaryWindow = kind;
}})();
"""


def window_metrics(left, right, full_screen, zoom):
    """Room for the window controls at either end of the page's title strip.

    The left inset is the Mac shell's own variable. The page has no right one, so the right
    inset goes in a variable that LINUX_CSS applies to the title bars.
    """
    return f"""
(() => {{
  const root = document.documentElement;
  const set = (name, value) => {{
    if (root.style.getPropertyValue(name) !== value) root.style.setProperty(name, value);
  }};
  set('--amp-macos-window-controls-left-inset', '{left}px');
  set('--amp-linux-window-controls-right-inset', '{right}px');
  root.toggleAttribute('data-amp-macos-window-full-screen', {json.dumps(full_screen)});
  const zoom = '{zoom}';
  if (root.getAttribute('data-amp-page-zoom') !== zoom) root.setAttribute('data-amp-page-zoom', zoom);
}})();
"""


# Linux-specific layout the page has no Mac-shell equivalent for: keep title-bar actions clear of
# window controls on the right.
LINUX_CSS = """
html[data-amp-shell=macos-v1] .app-title-bar,
html[data-amp-shell=macos-v1] .app-window-controls-title-bar {
  padding-right: calc(var(--amp-linux-window-controls-right-inset, 0px) + 0.25rem) !important;
}
"""


APP_STATE = """
(() => {
  let latest = { version: 1, websiteReady: false };
  const post = () => globalThis.webkit?.messageHandlers?.ampNativeAppState?.postMessage(latest);
  addEventListener('amp:native-app-state', (event) => {
    if (event.detail?.version !== 1) return;
    latest = event.detail;
    post();
  });
  addEventListener('pageshow', post);
  post();
})();
"""

NAVIGATION_STATE = """
(() => {
  const post = () => globalThis.webkit?.messageHandlers?.ampNavigationState?.postMessage(location.href);
  for (const method of ['pushState', 'replaceState']) {
    const original = history[method];
    history[method] = function (...args) {
      const result = original.apply(this, args);
      post();
      return result;
    };
  }
  addEventListener('popstate', post);
  addEventListener('hashchange', post);
  addEventListener('pageshow', post);
  post();
})();
"""

EXTERNAL_WINDOW_OPEN = """
(() => {
  if (window.__ampNativeExternalWindowOpenInstalled) return;
  window.__ampNativeExternalWindowOpenInstalled = true;
  const originalOpen = window.open.bind(window);
  window.open = (url, target, features) => {
    let parsed;
    try { parsed = new URL(url, location.href) } catch { return originalOpen(url, target, features) }
    if ((parsed.protocol === 'http:' || parsed.protocol === 'https:') && parsed.origin !== location.origin) {
      window.webkit?.messageHandlers?.ampNativeOpenExternalURL?.postMessage(parsed.href);
      return null;
    }
    return originalOpen(url, target, features);
  };
})();
"""

# The Mac app opens a same-origin link in a new window on ⌘-click or middle-click. Ctrl takes the
# place of ⌘ on Linux.
OPEN_IN_NEW_WINDOW = """
(() => {
  if (window.__ampNativeCommandClickInstalled) return;
  window.__ampNativeCommandClickInstalled = true;
  const open = (event) => {
    const modifierClick = event.type === 'click' && event.button === 0 &&
      (event.ctrlKey || event.metaKey) && !event.altKey;
    const middleClick = event.type === 'auxclick' && event.button === 1;
    if (!modifierClick && !middleClick) return;
    const anchor = event.composedPath().find((node) => node instanceof HTMLAnchorElement && node.href);
    if (!anchor || anchor.target === '_blank' || anchor.hasAttribute('download') ||
      anchor.hasAttribute('data-puck-trigger')) return;
    let url;
    try { url = new URL(anchor.href, location.href) } catch { return }
    if (url.origin !== location.origin) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    window.webkit?.messageHandlers?.ampNativeOpenWindow?.postMessage(url.pathname + url.search + url.hash);
  };
  document.addEventListener('click', open, true);
  document.addEventListener('auxclick', open, true);
})();
"""

# WebKitGTK only lets the page write the clipboard during a user gesture; the native clipboard
# does not need one, which is why the Mac app routes writes through it too.
CLIPBOARD = """
(() => {
  if (window !== window.top) return;
  const clipboard = navigator.clipboard;
  if (!clipboard) return;
  const write = clipboard.write.bind(clipboard);
  const post = (content) => window.webkit.messageHandlers.ampNativeClipboardWrite.postMessage(content);
  let sequence = 0;
  clipboard.writeText = async function (text) {
    if (arguments.length === 0) throw new TypeError('writeText requires text');
    ++sequence;
    await post({ text: String(text) });
  };
  clipboard.write = async (items) => {
    const current = ++sequence;
    if (items.length !== 1 || !items[0].types.includes('text/plain') ||
      items[0].types.some((type) => type !== 'text/plain' && type !== 'text/html')) {
      return write(items);
    }
    const item = items[0];
    const content = { text: await (await item.getType('text/plain')).text() };
    if (item.types.includes('text/html')) content.html = await (await item.getType('text/html')).text();
    if (current !== sequence) throw new DOMException('Superseded by a newer clipboard write', 'AbortError');
    await post(content);
  };
})();
"""


def app_command(command, **fields):
    """Dispatches a cancelable amp:native-app-command and returns whether the page handled it."""
    detail = {"version": 1, "command": command, **fields}
    return (
        "!window.dispatchEvent(new CustomEvent('amp:native-app-command', "
        f"{{ cancelable: true, detail: {json.dumps(detail)} }}))"
    )


def host_visibility(sequence, visible, present):
    detail = {"version": 1, "sequence": sequence, "visible": visible, "present": present}
    return (
        "window.dispatchEvent(new CustomEvent('amp:native-host-visibility', "
        f"{{ detail: {json.dumps(detail)} }}))"
    )


def notification_permission(permission):
    return f"""
document.documentElement.dataset.ampMacNotificationPermission = {json.dumps(permission)};
window.dispatchEvent(new Event('amp-mac-notification-permission-changed'));
"""


def runner_state(runner_id):
    if runner_id:
        body = f"document.documentElement.dataset.ampMacRunnerId = {json.dumps(runner_id)};"
    else:
        body = "delete document.documentElement.dataset.ampMacRunnerId;"
    return body + "\nwindow.dispatchEvent(new Event('amp-mac-runner-changed'));"


def confirmation_result(event_detail):
    return (
        "window.dispatchEvent(new CustomEvent('amp:native-confirmation-sheet', "
        f"{{ detail: {json.dumps(event_detail)} }}))"
    )


# Distribution WebKitGTK has no WebRTC, so the page's LiveKit client cannot join a Huddle. When
# the page asks for the call's token, hand it to the native client (huddle.py) and tell the page
# the call opened in its own window; the page's join then stops with that message.
HUDDLE = """
(() => {
  if (window !== window.top || typeof RTCPeerConnection === 'function' || window.__ampNativeHuddleInstalled) return;
  window.__ampNativeHuddleInstalled = true;
  const originalFetch = window.fetch.bind(window);
  const tokenPath = /^\\/api\\/threads\\/([^/]+)\\/space-token$/;
  // The page calls it a Huddle or a Space depending on a feature flag; take the word from the
  // button or menu item that started the join.
  let callName = 'Huddle';
  document.addEventListener('click', (event) => {
    const control = event.target instanceof Element && event.target.closest('button, [role=menuitem], a');
    const match = control && (control.getAttribute('aria-label') || control.textContent || '').match(/\\b(Huddle|Space)\\b/);
    if (match) callName = match[1];
  }, true);
  window.fetch = async (input, init) => {
    let url;
    try { url = new URL(input instanceof Request ? input.url : String(input), location.href) } catch { return originalFetch(input, init) }
    const match = url.origin === location.origin && url.pathname.match(tokenPath);
    // An empty body joins; reconcile, invite and the rest carry an action.
    if (!match || String(init?.method).toUpperCase() !== 'POST' || init?.body !== '{}') return originalFetch(input, init);
    const response = await originalFetch(input, init);
    const body = response.ok ? await response.clone().json().catch(() => null) : null;
    if (typeof body?.serverURL !== 'string' || typeof body?.participantToken !== 'string') return response;
    window.webkit.messageHandlers.ampNativeHuddle.postMessage({
      command: 'join', threadID: decodeURIComponent(match[1]), title: document.title, callName,
      serverURL: body.serverURL, participantToken: body.participantToken,
    });
    return new Response(JSON.stringify({ error: 'The call opened in its own window.' }),
      { status: 409, headers: { 'Content-Type': 'application/json' } });
  };
})();
"""

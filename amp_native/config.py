"""Paths, identity, and the persisted app settings."""

import json
import os
import socket
from pathlib import Path

APP_ID = "dev.ampnative.Amp"
APP_NAME = "Amp"
VERSION = "0.1.0"

# The web app reads these from the user agent. AmpApp names the Linux bundle the server already
# knows (its sign-in callback scheme is registered for it); the build number is that of the
# official Linux build this shell was reverse-engineered against.
BUNDLE_ID = "com.ampcode.amp.linux"
BUILD_NUMBER = 29894161
SIGN_IN_SCHEME = "com.ampcode.amp.linux.auth"

APP_URL = os.environ.get("AMP_DESKTOP_URL", "https://ampcode.com").rstrip("/")


def _xdg(name, default):
    return Path(os.environ.get(name) or Path.home() / default)


CONFIG_DIR = _xdg("XDG_CONFIG_HOME", ".config") / "amp-native"
DATA_DIR = _xdg("XDG_DATA_HOME", ".local/share") / "amp-native"
CACHE_DIR = _xdg("XDG_CACHE_HOME", ".cache") / "amp-native"
STATE_FILE = CONFIG_DIR / "settings.json"
LOG_FILE = CACHE_DIR / "amp-native.log"
RUNNER_LOG = CACHE_DIR / "runner.log"
STDERR_LOG = CACHE_DIR / "stderr.log"  # written by the launcher scripts/install creates
ICON_DIR = DATA_DIR / "icons"

ZOOM_FACTORS = [0.5, 0.67, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0]

DEFAULTS = {
    "show_unread_count": True,
    "warn_before_quitting": False,
    "open_at_login": False,
    "appearance": "system",  # system | light | dark
    "zoom": 1.0,
    "app_icon": "dark",  # dark | oled | light | puck
    "notifications": "notDetermined",  # notDetermined | authorized | denied
    "runner_enabled": False,
    "runner_name": f"{socket.gethostname().split('.')[0].lower()}-amp-app",
    "runner_desktop": False,
    "runner_terminal": False,
    "runner_keep_awake": False,
    "runner_home": False,
    "runner_dirs": [],
    "window": {"width": 1280, "height": 800, "maximized": False},
}


class Settings:
    def __init__(self):
        self._data = dict(DEFAULTS)
        try:
            self._data.update(json.loads(STATE_FILE.read_text()))
        except (OSError, ValueError):
            pass
        self._listeners = []

    def __getitem__(self, key):
        return self._data[key]

    def get(self, key, default=None):
        return self._data.get(key, default)

    def set(self, key, value):
        if self._data.get(key) == value:
            return
        self._data[key] = value
        self.save()
        for listener in list(self._listeners):
            listener(key, value)

    def connect(self, listener):
        self._listeners.append(listener)

    def save(self):
        CONFIG_DIR.mkdir(parents=True, exist_ok=True)
        tmp = STATE_FILE.with_suffix(".tmp")
        tmp.write_text(json.dumps(self._data, indent=2))
        tmp.replace(STATE_FILE)


def nearest_zoom(value):
    try:
        value = float(value)
    except (TypeError, ValueError):
        return 1.0
    return min(ZOOM_FACTORS, key=lambda f: abs(f - value))


def step_zoom(current, direction):
    if direction == 0:
        return 1.0
    index = ZOOM_FACTORS.index(nearest_zoom(current)) + direction
    return ZOOM_FACTORS[max(0, min(index, len(ZOOM_FACTORS) - 1))]

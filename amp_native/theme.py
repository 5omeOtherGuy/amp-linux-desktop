"""Follows the Omarchy theme: recolors the page from ~/.local/state/omarchy/current/theme.

A port of amp-desktop's omarchy-theme.ts and theme.ts (amp-desktop 0.1.29894161), so the page gets
the same palette as in that app. The CSS applies only while the page's color scheme is "system",
which is how the page lets a host theme it.
"""

import math
import os
import re
from pathlib import Path

from gi.repository import Gio, GLib

HEX = re.compile(r"^#(?:[0-9a-f]{3}|[0-9a-f]{6})$", re.I)
ANSI_KEYS = ["background", "red", "green", "yellow", "blue", "magenta", "cyan", "foreground", "muted",
             "bright_red", "bright_green", "bright_yellow", "bright_blue", "bright_magenta", "bright_cyan",
             "bright_foreground"]
TERMINAL_NAMES = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"]


def parse_hex(value):
    if not value or not HEX.match(value):
        return None
    h = value[1:].lower()
    return "#" + ("".join(c * 2 for c in h) if len(h) == 3 else h)


def channels(c):
    return [int(c[1 + i * 2:3 + i * 2], 16) for i in range(3)]


def to_hex(values):
    # Math.round, as in amp-desktop: halves round up, where Python's round() rounds to even.
    return "#" + "".join(f"{max(0, min(255, math.floor(v + 0.5))):02x}" for v in values)


def mix(a, b, amount):
    return to_hex([x * (1 - amount) + y * amount for x, y in zip(channels(a), channels(b))])


def _lin(v):
    c = v / 255
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def _delin(v):
    return 255 * (v * 12.92 if v <= 0.0031308 else 1.055 * v ** (1 / 2.4) - 0.055)


def mix_linear(a, b, amount):
    return to_hex([_delin(x + (y - x) * amount) for x, y in zip(map(_lin, channels(a)), map(_lin, channels(b)))])


def luminance(c):
    r, g, b = map(_lin, channels(c))
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast(a, b):
    hi, lo = sorted((luminance(a), luminance(b)), reverse=True)
    return (hi + 0.05) / (lo + 0.05)


def on_color(bg):
    return "#ffffff" if contrast("#ffffff", bg) >= contrast("#000000", bg) else "#000000"


def _stand_out(selection, accent, under):
    # Preserve the theme's hue where possible, but don't depend on its accent
    # being distinguishable from its surfaces (white and vantablack, for example).
    fallback = max(("#000000", "#ffffff"), key=lambda c: min(contrast(c, s) for s in under))
    for target in (accent, fallback):
        for step in range(21):
            c = mix(selection, target, step * 0.05)
            if all(contrast(c, s) >= 3 for s in under):
                return c
    return fallback


def _readable(fg, bg, under):
    best = fg
    for step in range(1, 9):
        c = mix(fg, bg, step * 0.05)
        if not all(contrast(c, s) >= 4.5 for s in under):
            break
        best = c
    return best


def parse_colors(toml, has_light_mode_file):
    values = dict(re.findall(r'^\s*(\w+)\s*=\s*"([^"]*)"', toml, re.M))

    def color(*keys):
        return next((v for v in (parse_hex(values.get(k)) for k in keys) if v), None)

    bg = color("background", "bg", "color0")
    fg = color("foreground", "fg", "color7")
    if not bg or not fg:
        return None
    ansi = []
    for i, key in enumerate(ANSI_KEYS):
        fallback = fg if i < 8 else (ansi[i - 8] if i - 8 < len(ansi) else fg)
        ansi.append(color(key, f"color{i}") or fallback)
    ansi[0], ansi[7] = bg, fg
    dark_bg = color("dark_background", "dark_bg") or mix(bg, "#000000", 0.25)
    darker_bg = color("darker_background", "darker_bg") or mix(bg, "#000000", 0.5)
    lighter_bg = color("lighter_background", "lighter_bg") or bg
    dark_fg = color("dark_foreground", "dark_fg", "color8") or fg
    muted = color("muted", "color8") or dark_fg
    accent = color("accent", "blue", "color4") or fg
    red = color("red", "color1") or fg
    declared = values.get("mode") or values.get("theme_type")
    if declared in ("light", "dark"):
        mode = declared
    else:
        mode = "light" if has_light_mode_file or sum(channels(bg)) > 382 else "dark"
    backgrounds = [bg, dark_bg, darker_bg]
    return {
        "mode": mode, "background": bg, "foreground": fg,
        "windowBackground": dark_bg, "sidebarBackground": darker_bg,
        "controlBackground": lighter_bg, "controlHover": mix(lighter_bg, fg, 0.1),
        "mutedForeground": _readable(fg, bg, backgrounds), "border": muted,
        "selection": _stand_out(color("selection", "color8") or bg, accent,
                                backgrounds + [lighter_bg, mix(lighter_bg, fg, 0.1)]),
        "accent": accent, "accentHover": mix_linear(accent, "#ffffff" if mode == "dark" else "#000000", 0.12),
        "accentForeground": on_color(accent), "destructive": red, "destructiveForeground": on_color(red),
        "success": color("green", "color2") or fg, "warning": color("yellow", "color3") or fg,
        "ansi": ansi,
    }


def palette_css(p):
    bg, fg, ansi = p["background"], p["foreground"], p["ansi"]

    def a(i):
        return ansi[i] if i < len(ansi) else fg

    variables = {
        "background": bg, "foreground": fg, "card": bg, "card-foreground": fg, "popover": bg,
        "popover-foreground": fg, "editor-background": bg, "editor-foreground": fg, "input-background": bg,
        "app-surface-background": bg, "app-shell-background": p["windowBackground"], "app-shell-foreground": fg,
        "sidebar": p["sidebarBackground"], "sidebar-foreground": fg, "muted": p["controlBackground"],
        "muted-foreground": p["mutedForeground"], "secondary": p["controlBackground"],
        "secondary-hover": p["controlHover"], "secondary-foreground": fg, "accent": p["controlHover"],
        "accent-foreground": fg, "border": p["border"], "sidebar-border": p["border"], "input-border": p["border"],
        "primary": p["accent"], "primary-hover": p["accentHover"], "primary-foreground": p["accentForeground"],
        "ring": p["accent"], "link": p["accent"], "selection-background": p["selection"],
        "transcript-selection-background": p["selection"], "destructive": p["destructive"],
        "destructive-background": p["destructive"], "destructive-foreground": p["destructiveForeground"],
        "success": p["success"], "warning": p["warning"], "mention": a(5),
        "diff-addition-foreground": a(2), "diff-removal-foreground": a(1),
        "diff-addition-background": mix(bg, a(2), 0.15), "diff-removal-background": mix(bg, a(1), 0.15),
        "diff-meta-foreground": a(3), "hljs-function": a(4), "hljs-keyword": a(5), "hljs-class": a(3),
        "hljs-string": a(2), "hljs-comment": p["mutedForeground"], "hljs-number": a(6),
    }
    for i, c in enumerate(ansi):
        variables[f"terminal-{'' if i < 8 else 'bright-'}{TERMINAL_NAMES[i % 8]}"] = c
    decls = "\n".join(f"\t--{k}: {v} !important;" for k, v in variables.items())
    system = ":root[data-amp-color-scheme='system']"
    return (f"{system},\n{system} :is(.scheme-light, .scheme-dark, [data-amp-content-theme]) {{\n"
            f"\tcolor-scheme: {p['mode']} !important;\n}}\n{system},\n{system} * {{\n{decls}\n}}\n"
            # The page sets only selection backgrounds and otherwise retains syntax,
            # link or muted text colors, which need not be readable on the highlight.
            f"{system}::selection, {system} *::selection {{\n"
            # WebKit darkens opaque selection colors. An explicit alpha avoids
            # that adjustment and preserves the contrast.
            f"\tbackground-color: {p['selection']}fe !important;\n"
            f"\tcolor: {on_color(p['selection'])} !important;\n}}\n")


def current_directory():
    home = Path.home()
    for base in (Path(os.environ.get("XDG_STATE_HOME") or home / ".local/state"),
                 Path(os.environ.get("XDG_CONFIG_HOME") or home / ".config")):
        directory = base / "omarchy/current"
        if (directory / "theme").exists():
            return directory
    return None


def read_palette(directory):
    try:
        theme = directory / "theme"
        return parse_colors((theme / "colors.toml").read_text(), (theme / "light.mode").exists())
    except (OSError, ValueError):
        return None


class OmarchyTheme:
    """Calls on_change(palette or None) now and whenever the Omarchy theme changes."""

    def __init__(self, on_change):
        self._on_change = on_change
        self._timer = 0
        self.directory = current_directory()
        self.palette = None
        if self.directory is None:
            return
        # `omarchy theme set` repoints the current/theme symlink, so watch its directory.
        self._monitor = Gio.File.new_for_path(str(self.directory)).monitor_directory(Gio.FileMonitorFlags.WATCH_MOVES, None)
        self._monitor.connect("changed", lambda *_: self._debounce())
        self._reload()

    def _debounce(self):
        if self._timer:
            GLib.source_remove(self._timer)
        self._timer = GLib.timeout_add(100, self._reload)

    def _reload(self):
        self._timer = 0
        self.palette = read_palette(self.directory)
        self._on_change(self.palette)
        return False

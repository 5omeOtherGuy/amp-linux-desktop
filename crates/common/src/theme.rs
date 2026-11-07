//! The colours of the Omarchy theme in ~/.local/state/omarchy/current/theme, as page CSS.
//!
//! A port of amp-desktop's omarchy-theme.ts and theme.ts (amp-desktop 0.1.29894161), so the page
//! gets the same palette as in that app. The CSS applies only while the page's color scheme is
//! "system", which is how the page lets a host theme it. Watching the theme for changes is the
//! app's part.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

const ANSI_KEYS: [&str; 16] = [
    "background",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "foreground",
    "muted",
    "bright_red",
    "bright_green",
    "bright_yellow",
    "bright_blue",
    "bright_magenta",
    "bright_cyan",
    "bright_foreground",
];
const TERMINAL_NAMES: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];

#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub background: String,
    pub foreground: String,
    pub window_background: String,
    pub sidebar_background: String,
    pub control_background: String,
    pub control_hover: String,
    pub muted_foreground: String,
    pub border: String,
    pub selection: String,
    pub accent: String,
    pub accent_hover: String,
    pub accent_foreground: String,
    pub destructive: String,
    pub destructive_foreground: String,
    pub success: String,
    pub warning: String,
    pub ansi: Vec<String>,
}

/// `#rgb` or `#rrggbb` as lowercase `#rrggbb`.
fn parse_hex(value: Option<&String>) -> Option<String> {
    let value = value?;
    let hex = value.strip_prefix('#')?;
    if !(hex.len() == 3 || hex.len() == 6) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let hex = hex.to_ascii_lowercase();
    Some(if hex.len() == 3 {
        format!("#{}", hex.chars().flat_map(|c| [c, c]).collect::<String>())
    } else {
        format!("#{hex}")
    })
}

fn channels(c: &str) -> [f64; 3] {
    std::array::from_fn(|i| u8::from_str_radix(&c[1 + i * 2..3 + i * 2], 16).unwrap_or(0) as f64)
}

/// Math.round, as in amp-desktop: halves round up.
fn to_hex(values: [f64; 3]) -> String {
    values.iter().fold("#".to_string(), |s, v| s + &format!("{:02x}", (v + 0.5).floor().clamp(0.0, 255.0) as u8))
}

pub fn mix(a: &str, b: &str, amount: f64) -> String {
    let (a, b) = (channels(a), channels(b));
    to_hex(std::array::from_fn(|i| a[i] * (1.0 - amount) + b[i] * amount))
}

fn lin(v: f64) -> f64 {
    let c = v / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn delin(v: f64) -> f64 {
    255.0 * if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

fn mix_linear(a: &str, b: &str, amount: f64) -> String {
    let (a, b) = (channels(a).map(lin), channels(b).map(lin));
    to_hex(std::array::from_fn(|i| delin(a[i] + (b[i] - a[i]) * amount)))
}

fn luminance(c: &str) -> f64 {
    let [r, g, b] = channels(c).map(lin);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn contrast(a: &str, b: &str) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

pub fn on_color(bg: &str) -> &'static str {
    if contrast("#ffffff", bg) >= contrast("#000000", bg) { "#ffffff" } else { "#000000" }
}

fn min_contrast(c: &str, under: &[String]) -> f64 {
    under.iter().map(|s| contrast(c, s)).fold(f64::INFINITY, f64::min)
}

/// Preserve the theme's hue where possible, but don't depend on its accent being
/// distinguishable from its surfaces (white and vantablack, for example).
fn stand_out(selection: &str, accent: &str, under: &[String]) -> String {
    let fallback = if min_contrast("#ffffff", under) > min_contrast("#000000", under) { "#ffffff" } else { "#000000" };
    for target in [accent, fallback] {
        for step in 0..21 {
            let c = mix(selection, target, step as f64 * 0.05);
            if under.iter().all(|s| contrast(&c, s) >= 3.0) {
                return c;
            }
        }
    }
    fallback.to_string()
}

fn readable(fg: &str, bg: &str, under: &[String]) -> String {
    let mut best = fg.to_string();
    for step in 1..9 {
        let c = mix(fg, bg, step as f64 * 0.05);
        if !under.iter().all(|s| contrast(&c, s) >= 4.5) {
            break;
        }
        best = c;
    }
    best
}

/// `key = "value"` pairs at the start of lines, as the regex `^\s*(\w+)\s*=\s*"([^"]*)"` finds them.
fn toml_strings(toml: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    for line in toml.lines() {
        let rest = line.trim_start();
        let key_len = rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len());
        if key_len == 0 {
            continue;
        }
        let (key, rest) = rest.split_at(key_len);
        let Some(rest) = rest.trim_start().strip_prefix('=') else { continue };
        let Some(rest) = rest.trim_start().strip_prefix('"') else { continue };
        if let Some(end) = rest.find('"') {
            values.insert(key.to_string(), rest[..end].to_string());
        }
    }
    values
}

pub fn parse_colors(toml: &str, has_light_mode_file: bool) -> Option<Palette> {
    let values = toml_strings(toml);
    let color = |keys: &[&str]| keys.iter().find_map(|k| parse_hex(values.get(*k)));

    let bg = color(&["background", "bg", "color0"])?;
    let fg = color(&["foreground", "fg", "color7"])?;
    let mut ansi: Vec<String> = Vec::new();
    for (i, key) in ANSI_KEYS.iter().enumerate() {
        let fallback = if i < 8 { fg.clone() } else { ansi.get(i - 8).cloned().unwrap_or_else(|| fg.clone()) };
        ansi.push(color(&[key, &format!("color{i}")]).unwrap_or(fallback));
    }
    ansi[0] = bg.clone();
    ansi[7] = fg.clone();
    let dark_bg = color(&["dark_background", "dark_bg"]).unwrap_or_else(|| mix(&bg, "#000000", 0.25));
    let darker_bg = color(&["darker_background", "darker_bg"]).unwrap_or_else(|| mix(&bg, "#000000", 0.5));
    let lighter_bg = color(&["lighter_background", "lighter_bg"]).unwrap_or_else(|| bg.clone());
    let dark_fg = color(&["dark_foreground", "dark_fg", "color8"]).unwrap_or_else(|| fg.clone());
    let muted = color(&["muted", "color8"]).unwrap_or(dark_fg);
    let accent = color(&["accent", "blue", "color4"]).unwrap_or_else(|| fg.clone());
    let red = color(&["red", "color1"]).unwrap_or_else(|| fg.clone());
    let declared = values.get("mode").or_else(|| values.get("theme_type")).map(String::as_str);
    let dark = match declared {
        Some("light") => false,
        Some("dark") => true,
        _ => !(has_light_mode_file || channels(&bg).iter().sum::<f64>() > 382.0),
    };
    let backgrounds = vec![bg.clone(), dark_bg.clone(), darker_bg.clone()];
    let control_hover = mix(&lighter_bg, &fg, 0.1);
    let mut selection_under = backgrounds.clone();
    selection_under.extend([lighter_bg.clone(), control_hover.clone()]);
    let selection =
        stand_out(&color(&["selection", "color8"]).unwrap_or_else(|| bg.clone()), &accent, &selection_under);
    Some(Palette {
        dark,
        muted_foreground: readable(&fg, &bg, &backgrounds),
        window_background: dark_bg,
        sidebar_background: darker_bg,
        control_background: lighter_bg,
        control_hover,
        border: muted,
        selection,
        accent_hover: mix_linear(&accent, if dark { "#ffffff" } else { "#000000" }, 0.12),
        accent_foreground: on_color(&accent).to_string(),
        destructive_foreground: on_color(&red).to_string(),
        destructive: red,
        accent,
        success: color(&["green", "color2"]).unwrap_or_else(|| fg.clone()),
        warning: color(&["yellow", "color3"]).unwrap_or_else(|| fg.clone()),
        background: bg,
        foreground: fg,
        ansi,
    })
}

pub fn palette_css(p: &Palette) -> String {
    let (bg, fg) = (p.background.as_str(), p.foreground.as_str());
    let a = |i: usize| p.ansi.get(i).map(String::as_str).unwrap_or(fg);
    let (diff_add, diff_remove) = (mix(bg, a(2), 0.15), mix(bg, a(1), 0.15));
    let mut variables: Vec<(String, &str)> = [
        ("background", bg),
        ("foreground", fg),
        ("card", bg),
        ("card-foreground", fg),
        ("popover", bg),
        ("popover-foreground", fg),
        ("editor-background", bg),
        ("editor-foreground", fg),
        ("input-background", bg),
        ("app-surface-background", bg),
        ("app-shell-background", &p.window_background),
        ("app-shell-foreground", fg),
        ("sidebar", &p.sidebar_background),
        ("sidebar-foreground", fg),
        ("muted", &p.control_background),
        ("muted-foreground", &p.muted_foreground),
        ("secondary", &p.control_background),
        ("secondary-hover", &p.control_hover),
        ("secondary-foreground", fg),
        ("accent", &p.control_hover),
        ("accent-foreground", fg),
        ("border", &p.border),
        ("sidebar-border", &p.border),
        ("input-border", &p.border),
        ("primary", &p.accent),
        ("primary-hover", &p.accent_hover),
        ("primary-foreground", &p.accent_foreground),
        ("ring", &p.accent),
        ("link", &p.accent),
        ("selection-background", &p.selection),
        ("transcript-selection-background", &p.selection),
        ("destructive", &p.destructive),
        ("destructive-background", &p.destructive),
        ("destructive-foreground", &p.destructive_foreground),
        ("success", &p.success),
        ("warning", &p.warning),
        ("mention", a(5)),
        ("diff-addition-foreground", a(2)),
        ("diff-removal-foreground", a(1)),
        ("diff-addition-background", &diff_add),
        ("diff-removal-background", &diff_remove),
        ("diff-meta-foreground", a(3)),
        ("hljs-function", a(4)),
        ("hljs-keyword", a(5)),
        ("hljs-class", a(3)),
        ("hljs-string", a(2)),
        ("hljs-comment", &p.muted_foreground),
        ("hljs-number", a(6)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    for (i, c) in p.ansi.iter().enumerate() {
        variables.push((format!("terminal-{}{}", if i < 8 { "" } else { "bright-" }, TERMINAL_NAMES[i % 8]), c));
    }
    let decls: Vec<String> = variables.iter().map(|(k, v)| format!("\t--{k}: {v} !important;")).collect();
    let system = ":root[data-amp-color-scheme='system']";
    let mode = if p.dark { "dark" } else { "light" };
    // The page sets only selection backgrounds and otherwise retains syntax, link or muted text
    // colors, which need not be readable on the highlight. WebKit darkens opaque selection
    // colors; an explicit alpha avoids that adjustment and preserves the contrast.
    format!(
        "{system},\n{system} :is(.scheme-light, .scheme-dark, [data-amp-content-theme]) {{\n\tcolor-scheme: {mode} !important;\n}}\n\
         {system},\n{system} * {{\n{}\n}}\n\
         {system}::selection, {system} *::selection {{\n\tbackground-color: {}fe !important;\n\tcolor: {} !important;\n}}\n",
        decls.join("\n"),
        p.selection,
        on_color(&p.selection)
    )
}

/// The Omarchy `current` directory, whose `theme` link `omarchy theme set` repoints.
pub fn current_directory() -> Option<PathBuf> {
    let home = glib::home_dir();
    let env = |name: &str, default: &str| match std::env::var_os(name) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home.join(default),
    };
    [env("XDG_STATE_HOME", ".local/state"), env("XDG_CONFIG_HOME", ".config")]
        .into_iter()
        .map(|base| base.join("omarchy/current"))
        .find(|d| d.join("theme").exists())
}

pub fn read_palette(directory: &Path) -> Option<Palette> {
    let theme = directory.join("theme");
    let toml = std::fs::read(theme.join("colors.toml")).ok()?;
    parse_colors(std::str::from_utf8(&toml).ok()?, theme.join("light.mode").exists())
}

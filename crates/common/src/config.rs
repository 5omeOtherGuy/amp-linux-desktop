//! Paths, identity, and the persisted app settings.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde_json::{Map, Value, json};

pub const APP_ID: &str = "dev.ampnative.Amp";
pub const APP_NAME: &str = "Amp";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// The web app reads these from the user agent. AmpApp names the Linux bundle the server already
// knows (its sign-in callback scheme is registered for it); the build number is that of the
// official Linux build this shell was reverse-engineered against.
pub const BUNDLE_ID: &str = "com.ampcode.amp.linux";
pub const BUILD_NUMBER: u32 = 29894161;
pub const SIGN_IN_SCHEME: &str = "com.ampcode.amp.linux.auth";

/// ampcode.com, or $AMP_DESKTOP_URL, without a trailing slash.
pub static APP_URL: LazyLock<String> = LazyLock::new(|| {
    std::env::var("AMP_DESKTOP_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "https://ampcode.com".into())
        .trim_end_matches('/')
        .to_string()
});

fn home() -> PathBuf {
    glib::home_dir()
}

fn xdg(name: &str, default: &str) -> PathBuf {
    match std::env::var_os(name) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(default),
    }
}

pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub state_file: PathBuf,
    pub log_file: PathBuf,
    pub runner_log: PathBuf,
    /// Where the app sends its own stderr when it does not run in a terminal.
    pub stderr_log: PathBuf,
    /// Where the icon extraction puts Amp's icons for this user.
    pub icon_dir: PathBuf,
    /// Where a package installs them for all users instead.
    pub system_icon_dirs: Vec<PathBuf>,
}

pub static PATHS: LazyLock<Paths> = LazyLock::new(|| {
    let config_dir = xdg("XDG_CONFIG_HOME", ".config").join("amp-native");
    let data_dir = xdg("XDG_DATA_HOME", ".local/share").join("amp-native");
    let cache_dir = xdg("XDG_CACHE_HOME", ".cache").join("amp-native");
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    Paths {
        state_file: config_dir.join("settings.json"),
        log_file: cache_dir.join("amp-native.log"),
        runner_log: cache_dir.join("runner.log"),
        stderr_log: cache_dir.join("stderr.log"),
        icon_dir: data_dir.join("icons"),
        system_icon_dirs: data_dirs
            .split(':')
            .filter(|d| !d.is_empty())
            .map(|d| Path::new(d).join("amp-native/icons"))
            .collect(),
        config_dir,
        data_dir,
        cache_dir,
    }
});

/// The PNG of an app icon variant (dark, oled, light, puck, app), or None without Amp's icons.
pub fn icon_file(key: &str) -> Option<PathBuf> {
    std::iter::once(&PATHS.icon_dir)
        .chain(&PATHS.system_icon_dirs)
        .map(|d| d.join(format!("{key}.png")))
        .find(|p| p.exists())
}

pub const ZOOM_FACTORS: [f64; 12] = [0.5, 0.67, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];

pub fn nearest_zoom(value: &Value) -> f64 {
    let Some(value) = value.as_f64().or_else(|| value.as_str().and_then(|s| s.trim().parse().ok())) else {
        return 1.0;
    };
    nearest_zoom_f64(value)
}

pub fn nearest_zoom_f64(value: f64) -> f64 {
    // The first of two equally near factors wins, as Python's min() does.
    ZOOM_FACTORS
        .iter()
        .copied()
        .fold((f64::INFINITY, 1.0), |best, f| if (f - value).abs() < best.0 { ((f - value).abs(), f) } else { best })
        .1
}

pub fn step_zoom(current: f64, direction: i32) -> f64 {
    if direction == 0 {
        return 1.0;
    }
    let index = ZOOM_FACTORS.iter().position(|&f| f == nearest_zoom_f64(current)).unwrap_or(4) as i32 + direction;
    ZOOM_FACTORS[index.clamp(0, ZOOM_FACTORS.len() as i32 - 1) as usize]
}

fn defaults() -> Map<String, Value> {
    let host = glib::host_name();
    let runner_name = format!("{}-amp-app", host.split('.').next().unwrap_or("").to_lowercase());
    let Value::Object(map) = json!({
        "show_unread_count": true,
        "warn_before_quitting": false,
        "open_at_login": false,
        "appearance": "system",
        "zoom": 1.0,
        "app_icon": "dark",
        "puck_opening": "separateWindow",
        "notifications": "notDetermined",
        "runner_enabled": false,
        "runner_name": runner_name,
        "runner_desktop": false,
        "runner_terminal": false,
        "runner_keep_awake": false,
        "runner_home": false,
        "runner_dirs": [],
        "window": {"width": 1280, "height": 800, "maximized": false},
    }) else {
        unreachable!()
    };
    map
}

type Listener = Box<dyn Fn(&str, &Value)>;

/// The settings in settings.json. Keys the app does not know are kept as they are.
pub struct Settings {
    data: RefCell<Map<String, Value>>,
    listeners: RefCell<Vec<Listener>>,
}

impl Default for Settings {
    fn default() -> Self {
        Self::load()
    }
}

impl Settings {
    pub fn load() -> Self {
        let mut data = defaults();
        if let Some(Value::Object(saved)) =
            std::fs::read(&PATHS.state_file).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        {
            data.extend(saved);
        }
        Self { data: RefCell::new(data), listeners: RefCell::default() }
    }

    pub fn get(&self, key: &str) -> Value {
        self.data.borrow().get(key).cloned().unwrap_or(Value::Null)
    }

    /// Python truthiness, as the settings were written by the Python app.
    pub fn bool(&self, key: &str) -> bool {
        truthy(&self.get(key))
    }

    pub fn str(&self, key: &str) -> String {
        self.get(key).as_str().unwrap_or_default().to_string()
    }

    pub fn strings(&self, key: &str) -> Vec<String> {
        strings(&self.get(key))
    }

    pub fn set(&self, key: &str, value: Value) {
        if self.data.borrow().get(key) == Some(&value) {
            return;
        }
        self.data.borrow_mut().insert(key.to_string(), value.clone());
        self.save();
        // A listener may read the settings, so none is borrowed while they run.
        let listeners = self.listeners.take();
        for listener in &listeners {
            listener(key, &value);
        }
        let mut current = self.listeners.borrow_mut();
        let added = std::mem::replace(&mut *current, listeners);
        current.extend(added);
    }

    pub fn connect(&self, listener: impl Fn(&str, &Value) + 'static) {
        self.listeners.borrow_mut().push(Box::new(listener));
    }

    fn save(&self) {
        let text = match serde_json::to_string_pretty(&*self.data.borrow()) {
            Ok(t) => t,
            Err(_) => return,
        };
        let result = std::fs::create_dir_all(&PATHS.config_dir).and_then(|_| {
            let tmp = PATHS.state_file.with_extension("tmp");
            std::fs::write(&tmp, text)?;
            std::fs::rename(&tmp, &PATHS.state_file)
        });
        if let Err(error) = result {
            log::warn!("could not save settings: {error}");
        }
    }
}

pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn strings(value: &Value) -> Vec<String> {
    value.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
}

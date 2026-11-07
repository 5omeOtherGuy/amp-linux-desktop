//! The app log, ~/.cache/amp-native/amp-native.log, in the format the Python app wrote:
//! `2026-11-06 12:00:00,123 INFO amp_app.window: message`.

use std::io::Write;
use std::sync::Mutex;

struct FileLogger {
    file: Mutex<std::fs::File>,
    level: log::LevelFilter,
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // Libraries (LiveKit, for one) log a lot at info; only their warnings go in.
        let own = metadata.target().starts_with("amp_");
        metadata.level() <= if own { self.level } else { log::LevelFilter::Warn }
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let now = glib::DateTime::now_local().ok();
        let time = now
            .as_ref()
            .and_then(|t| t.format("%F %T").ok())
            .map(|t| format!("{t},{:03}", now.as_ref().map_or(0, |n| n.microsecond() / 1000)))
            .unwrap_or_default();
        let level = match record.level() {
            log::Level::Error => "ERROR",
            log::Level::Warn => "WARNING",
            log::Level::Info => "INFO",
            log::Level::Debug | log::Level::Trace => "DEBUG",
        };
        let line = format!("{time} {level} {}: {}\n", record.target().replace("::", "."), record.args());
        if let Ok(mut file) = self.file.lock() {
            let _ = file.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {}
}

/// Logs to `path`, at debug level when `debug` is set and at info otherwise.
pub fn init(path: &std::path::Path, debug: bool) {
    let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(path) else { return };
    let level = if debug { log::LevelFilter::Debug } else { log::LevelFilter::Info };
    if log::set_boxed_logger(Box::new(FileLogger { file: Mutex::new(file), level })).is_ok() {
        log::set_max_level(level.max(log::LevelFilter::Warn));
    }
}

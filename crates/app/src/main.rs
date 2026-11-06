//! Amp Linux Desktop: ampcode.com in a GTK window, answering the page's native bridge the way
//! Amp's macOS app does.

mod app;
mod bridge;
mod choose_projects;
mod confirmation;
mod file_drag;
mod files;
mod local_projects;
mod menus;
mod runner;
mod settings_window;
mod signin;
mod thread_import;
mod toasts;
mod tray;
mod window;
mod window_controls;

use std::io::IsTerminal;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

use amp_common::config::PATHS;

/// A launch from the desktop has its stderr on /dev/null, which would lose GTK and WebKit warnings
/// and panics. Keep it in a file instead, rotated at 1 MB.
fn keep_stderr() {
    if std::io::stderr().is_terminal() {
        return;
    }
    let log = &PATHS.stderr_log;
    if std::fs::metadata(log).is_ok_and(|m| m.len() > 1_000_000) {
        let _ = std::fs::rename(log, log.with_extension("log.old"));
    }
    if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
        // SAFETY: dup2 onto stderr's descriptor; `file` stays open until the duplicate exists.
        unsafe { libc::dup2(file.as_raw_fd(), 2) };
    }
}

/// `amp-linux-desktop --extract-icons [--dmg PATH] [--out DIR]`: writes Amp's icons from its macOS
/// disk image, downloading the latest one without --dmg, to the user's data dir or DIR.
fn extract_icons(args: &[String]) -> std::process::ExitCode {
    let mut dmg = None;
    let mut out = PATHS.icon_dir.clone();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--dmg", Some(path)) => dmg = Some(PathBuf::from(path)),
            ("--out", Some(path)) => out = PathBuf::from(path),
            _ => {
                eprintln!("usage: amp-linux-desktop --extract-icons [--dmg PATH] [--out DIR]");
                return std::process::ExitCode::from(2);
            }
        }
    }
    match amp_common::icons::fetch(&out, dmg.as_deref()) {
        Ok(names) => {
            for name in names {
                println!("icons: {name}");
            }
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("icons: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--extract-icons") {
        return extract_icons(&args[2..]);
    }
    // SAFETY (set_var): no other thread runs yet.
    unsafe {
        // GTK 4.16+ defaults to its Vulkan renderer, which shows WebKit's frames late: on an RTX
        // 3060 (driver 615, GTK 4.24, WebKitGTK 2.54) keypress-to-screen in the composer was 135 ms
        // median with Vulkan and 50 ms with GL (Chromium: 36 ms). GSK_RENDERER set by the user
        // still wins.
        if std::env::var_os("GSK_RENDERER").is_none() {
            std::env::set_var("GSK_RENDERER", "gl");
        }
        // WebKit picks the NVIDIA decoders for the Desktop tab's WebCodecs video, but they cannot
        // reach the GPU inside its web process sandbox, and WebKit does not fall back. Demote them
        // so the software decoders (gst-libav for H.264) are used. The web process inherits this.
        let nvidia: Vec<String> =
            ["h264", "h265", "vp8", "vp9", "av1"].iter().map(|c| format!("nv{c}dec:NONE")).collect();
        let nvidia = nvidia.join(",");
        let ranks = match std::env::var("GST_PLUGIN_FEATURE_RANK") {
            Ok(r) if !r.is_empty() => format!("{r},{nvidia}"),
            _ => nvidia,
        };
        std::env::set_var("GST_PLUGIN_FEATURE_RANK", ranks);
    }
    let _ = std::fs::create_dir_all(&PATHS.cache_dir);
    keep_stderr();
    amp_common::logger::init(&PATHS.log_file, std::env::var_os("AMP_NATIVE_DEBUG").is_some_and(|v| !v.is_empty()));
    app::run()
}

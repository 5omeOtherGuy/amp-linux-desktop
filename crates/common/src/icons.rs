//! Extracts the Amp app icons from Amp's own published macOS disk image.
//!
//! The icons are Amp's artwork, so neither the repository nor a prebuilt package carries them. They
//! come from https://static.ampcode.com/mac/latest.dmg: scripts/install and the Arch package
//! extract them at install or build time (`amp-linux-desktop --extract-icons`), and the app fetches
//! them on first start when neither did (the .deb and .rpm). It needs 7z (the 7zip package) to
//! open the disk image.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use soup::prelude::*;

pub const DMG_URL: &str = "https://static.ampcode.com/mac/latest.dmg";
const VARIANTS: [(&str, &str); 4] = [
    ("dark", "AppIcon.icns"),
    ("oled", "AppIconBlack.icns"),
    ("light", "AppIconLight.icns"),
    ("puck", "AppIconPuck.icns"),
];

/// The largest PNG image in an .icns file.
pub fn largest_png(icns: &[u8]) -> &[u8] {
    let mut best: &[u8] = &[];
    let mut offset = 8;
    while offset + 8 <= icns.len() {
        let length = u32::from_be_bytes(icns[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let chunk = &icns[(offset + 8).min(icns.len())..(offset + length).clamp(offset + 8, icns.len())];
        if chunk.starts_with(b"\x89PNG") && chunk.len() > best.len() {
            best = chunk;
        }
        offset += length.max(8);
    }
    best
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> std::io::Result<Self> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("amp-icons-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn which(name: &str) -> Option<PathBuf> {
    glib::find_program_in_path(name)
}

fn download(url: &str, to: &Path) -> Result<(), String> {
    let session = soup::Session::new();
    let message = soup::Message::new("GET", url).map_err(|e| e.to_string())?;
    let bytes = session.send_and_read(&message, gio::Cancellable::NONE).map_err(|e| e.to_string())?;
    if message.status_code() != 200 {
        return Err(format!("{url} answered {}", message.status_code()));
    }
    std::fs::write(to, &bytes).map_err(|e| e.to_string())
}

/// Writes the icon variants and app.png to `out`; returns the written names.
pub fn fetch(out: &Path, dmg: Option<&Path>) -> Result<Vec<String>, String> {
    let sevenzip =
        which("7z").or_else(|| which("7zz")).ok_or("install 7z (7zip) to extract the icons from Amp's disk image")?;
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let tmp = TempDir::new().map_err(|e| e.to_string())?;
    let dmg = match dmg {
        Some(d) => d.to_path_buf(),
        None => {
            let d = tmp.0.join("amp.dmg");
            download(DMG_URL, &d)?;
            d
        }
    };
    let status = Command::new(sevenzip)
        .arg("x")
        .arg("-y")
        .arg(format!("-o{}/x", tmp.0.display()))
        .arg(&dmg)
        .arg("Amp.app/Contents/Resources/*.icns")
        .stdout(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("7z failed: {status}"));
    }
    let resources = tmp.0.join("x/Amp.app/Contents/Resources");
    let mut written = Vec::new();
    for (key, name) in VARIANTS {
        let Ok(icns) = std::fs::read(resources.join(name)) else { continue };
        let png = largest_png(&icns);
        if !png.is_empty() {
            std::fs::write(out.join(format!("{key}.png")), png).map_err(|e| e.to_string())?;
            written.push(format!("{key}.png"));
        }
    }
    if written.is_empty() {
        return Err("Amp's disk image had none of the expected icons".into());
    }
    if out.join("dark.png").exists() {
        std::fs::copy(out.join("dark.png"), out.join("app.png")).map_err(|e| e.to_string())?;
    }
    Ok(written)
}

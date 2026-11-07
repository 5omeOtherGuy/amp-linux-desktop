//! Files the page hands to the app: opening and revealing local files, saving downloads, and
//! reading pasted images.
//!
//! Saving follows the Mac app (NativeSaveFileMessageHandler): files go straight to the Downloads
//! folder under a unique name, one streamed save at a time, written to a temporary file first.

use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use gtk::{gdk, gio, glib, prelude::*};
use serde_json::{Value, json};

const IMAGE_TYPES: [&str; 7] =
    ["image/png", "image/jpeg", "image/gif", "image/webp", "image/heic", "image/tiff", "image/bmp"];
const FILE_LIST_TYPES: [&str; 2] = ["text/uri-list", "x-special/gnome-copied-files"];
const MAX_PASTE_BYTES: u64 = 64 * 1024 * 1024;

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn local_file(uri: &Value) -> Result<gio::File, String> {
    let uri = uri.as_str().ok_or("expected a file URI")?;
    let url = url::Url::parse(uri).map_err(|_| "only local file URIs can be opened")?;
    if url.scheme() != "file" || !matches!(url.host_str(), None | Some("") | Some("localhost")) {
        return Err("only local file URIs can be opened".into());
    }
    let path = url.to_file_path().map_err(|_| "only local file URIs can be opened")?;
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    Ok(gio::File::for_path(path))
}

fn launch_context() -> Option<gdk::AppLaunchContext> {
    gdk::Display::default().map(|d| d.app_launch_context())
}

/// Opens a text file in the app the desktop uses for its type, or for plain text.
pub fn open_in_editor(uri: &Value) -> Result<(), String> {
    let file = local_file(uri)?;
    let info = file
        .query_info(gio::FILE_ATTRIBUTE_STANDARD_CONTENT_TYPE, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
        .map_err(|e| e.to_string())?;
    let content_type = info.content_type().map(|c| c.to_string()).unwrap_or_else(|| "text/plain".into());
    let mut app = gio::AppInfo::default_for_type(&content_type, false);
    if app.is_none() || !gio::content_type_is_a(&content_type, "text/plain") {
        app = gio::AppInfo::default_for_type("text/plain", false).or(app);
    }
    let app = app.ok_or("no editor is set up for text files")?;
    app.launch(&[file], launch_context().as_ref()).map_err(|e| e.to_string())
}

/// Selects the file in the file manager through org.freedesktop.FileManager1, or opens its folder
/// when no file manager provides that interface.
pub fn show_in_file_manager(uri: &Value, done: impl FnOnce(Result<Value, String>) + 'static) {
    let file = match local_file(uri) {
        Ok(f) => f,
        Err(error) => return done(Err(error)),
    };
    let open_folder = move |file: gio::File, done: Box<dyn FnOnce(Result<Value, String>)>| {
        let parent = file.parent().unwrap_or(file);
        done(
            gio::AppInfo::launch_default_for_uri(&parent.uri(), launch_context().as_ref())
                .map(|_| Value::Null)
                .map_err(|e| e.message().to_string()),
        )
    };
    let done: Box<dyn FnOnce(Result<Value, String>)> = Box::new(done);
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        return open_folder(file, done);
    };
    let params = (vec![file.uri().to_string()], "").to_variant();
    bus.call(
        Some("org.freedesktop.FileManager1"),
        "/org/freedesktop/FileManager1",
        "org.freedesktop.FileManager1",
        "ShowItems",
        Some(&params),
        None,
        gio::DBusCallFlags::NONE,
        5000,
        gio::Cancellable::NONE,
        move |result| match result {
            Ok(_) => done(Ok(Value::Null)),
            Err(_) => open_folder(file, done),
        },
    );
}

// ---- saving ------------------------------------------------------------------------------------

fn downloads() -> std::io::Result<PathBuf> {
    let folder =
        glib::user_special_dir(glib::UserDirectory::Downloads).unwrap_or_else(|| glib::home_dir().join("Downloads"));
    std::fs::create_dir_all(&folder)?;
    Ok(folder)
}

/// A name in `folder` no file has yet: `name`, `name (1)`, … like the Mac app.
pub fn unique_target(folder: &Path, name: &str) -> Result<PathBuf, String> {
    let name = Path::new(name).file_name().and_then(|n| n.to_str()).map(str::trim).filter(|n| !n.is_empty());
    let name = name.unwrap_or("download");
    let (stem, suffix) = match name.rfind('.') {
        // A leading dot names a hidden file, not a suffix, as in pathlib.
        Some(i) if i > 0 && i < name.len() - 1 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let mut target = folder.join(name);
    for n in 1..1000 {
        if !target.exists() {
            return Ok(target);
        }
        target = folder.join(format!("{stem} ({n}){suffix}"));
    }
    Err(format!("too many files named {name}"))
}

fn finished(app: &adw::Application, path: &Path) {
    let notification = gio::Notification::new("Download Finished");
    notification.set_body(path.file_name().and_then(|n| n.to_str()));
    app.send_notification(None, &notification);
}

pub fn save_file(app: &adw::Application, body: &Value) -> Result<(), String> {
    let data = body.get("base64").and_then(Value::as_str).ok_or("invalid save request")?;
    let data = b64().decode(data).map_err(|e| e.to_string())?;
    let target = unique_target(
        &downloads().map_err(|e| e.to_string())?,
        body.get("fileName").and_then(Value::as_str).unwrap_or(""),
    )?;
    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(&target).map_err(|e| e.to_string())?;
    out.write_all(&data).map_err(|e| e.to_string())?;
    finished(app, &target);
    Ok(())
}

/// A streamed save in progress: chunks go to a hidden temporary file in Downloads.
pub struct Transfer {
    id: String,
    name: String,
    size: u64,
    written: u64,
    folder: PathBuf,
    file: Option<std::fs::File>,
    path: PathBuf,
}

impl Transfer {
    fn new(name: &str, size: u64) -> std::io::Result<Self> {
        let folder = downloads()?;
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).map_err(std::io::Error::other)?;
        let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let path = folder.join(format!(".amp-download-{token}.part"));
        // create_new leaves the mode to the umask (0666 & ~umask), as for any download.
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
        Ok(Self {
            id: glib::uuid_string_random().to_string(),
            name: name.into(),
            size,
            written: 0,
            folder,
            file: Some(file),
            path,
        })
    }

    fn discard(mut self) {
        self.file = None;
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Streams a save in chunks: begin returns a transfer ID, then append, finish, or cancel.
pub fn save_stream(app: &adw::Application, active: &mut Option<Transfer>, body: &Value) -> Result<Value, String> {
    let action = body.get("action").and_then(Value::as_str);
    if action == Some("begin") {
        let size = body.get("size").and_then(Value::as_u64).ok_or("invalid save request")?;
        let name = body.get("fileName").and_then(Value::as_str).ok_or("invalid save request")?;
        if active.is_some() {
            return Err("another save is already active".into());
        }
        let transfer = Transfer::new(name, size).map_err(|e| format!("could not create temporary file: {e}"))?;
        let id = transfer.id.clone();
        *active = Some(transfer);
        return Ok(json!(id));
    }
    if active.as_ref().is_none_or(|t| body.get("transferID").and_then(Value::as_str) != Some(t.id.as_str())) {
        return Err("invalid save request".into());
    }
    match action {
        Some("append") => {
            let transfer = active.as_mut().expect("checked above");
            if body.get("offset").and_then(Value::as_u64) != Some(transfer.written) {
                return Err("save chunks are out of order".into());
            }
            let data = b64()
                .decode(body.get("base64").and_then(Value::as_str).unwrap_or(""))
                .map_err(|_| "invalid save chunk")?;
            if transfer.written + data.len() as u64 > transfer.size {
                return Err("invalid save chunk".into());
            }
            transfer.file.as_mut().expect("open until finished").write_all(&data).map_err(|e| e.to_string())?;
            transfer.written += data.len() as u64;
            Ok(Value::Null)
        }
        Some("finish") => {
            let mut transfer = active.take().expect("checked above");
            if transfer.written != transfer.size {
                transfer.discard();
                return Err("saved file size does not match".into());
            }
            transfer.file = None;
            // A hard link fails instead of replacing a file created meanwhile.
            let target = unique_target(&transfer.folder, &transfer.name)
                .and_then(|t| std::fs::hard_link(&transfer.path, &t).map(|_| t).map_err(|e| e.to_string()));
            match target {
                Ok(target) => {
                    let _ = std::fs::remove_file(&transfer.path);
                    finished(app, &target);
                    Ok(Value::Null)
                }
                Err(error) => {
                    transfer.discard();
                    Err(format!("could not save file: {error}"))
                }
            }
        }
        Some("cancel") => {
            active.take().expect("checked above").discard();
            Ok(Value::Null)
        }
        _ => Err("invalid save request".into()),
    }
}

// ---- pasteboard --------------------------------------------------------------------------------

/// Reads the first offered type of `mime_types` from the clipboard: its bytes and type.
async fn read_all(clipboard: &gdk::Clipboard, mime_types: &[&str]) -> Option<(glib::Bytes, String)> {
    let (stream, mime) = clipboard.read_future(mime_types, glib::Priority::DEFAULT).await.ok()?;
    let out = gio::MemoryOutputStream::new_resizable();
    let flags = gio::OutputStreamSpliceFlags::CLOSE_SOURCE | gio::OutputStreamSpliceFlags::CLOSE_TARGET;
    out.splice_future(&stream, flags, glib::Priority::DEFAULT).await.ok()?;
    Some((out.steal_as_bytes(), mime.to_string()))
}

fn image_files(uri_list: &str) -> std::io::Result<Vec<Value>> {
    let mut files = Vec::new();
    let mut total = 0;
    for line in uri_list.lines().map(str::trim) {
        // Skips comments, and the copy/cut verb of x-special/gnome-copied-files.
        if !line.starts_with("file://") {
            continue;
        }
        let Some(path) = url::Url::parse(line).ok().and_then(|u| u.to_file_path().ok()) else { continue };
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let (content_type, _) = gio::content_type_guess(Some(&name), None::<&[u8]>);
        let mime = gio::content_type_get_mime_type(&content_type).map(|m| m.to_string()).unwrap_or_default();
        if !mime.starts_with("image/") || !path.is_file() {
            continue;
        }
        total += std::fs::metadata(&path)?.len();
        if total > MAX_PASTE_BYTES {
            break;
        }
        files.push(json!({"name": name, "mimeType": mime, "dataBase64": b64().encode(std::fs::read(&path)?)}));
    }
    Ok(files)
}

/// { version: 1, files: [{ name, mimeType, dataBase64 }] } with the images on the clipboard:
/// copied image files first, then image data.
pub async fn read_pasteboard_images(clipboard: gdk::Clipboard) -> Result<Value, String> {
    let formats = clipboard.formats();
    let reply = |files: Vec<Value>| Ok(json!({"version": 1, "files": files}));
    if FILE_LIST_TYPES.iter().any(|t| formats.contain_mime_type(t)) {
        let data = read_all(&clipboard, &FILE_LIST_TYPES).await;
        let files = match data {
            Some((data, _)) => image_files(&String::from_utf8_lossy(&data)).map_err(|e| e.to_string())?,
            None => Vec::new(),
        };
        if !files.is_empty() {
            return reply(files);
        }
    }
    let offered: Vec<&str> = IMAGE_TYPES.iter().copied().filter(|t| formats.contain_mime_type(t)).collect();
    if offered.is_empty() {
        return reply(Vec::new());
    }
    match read_all(&clipboard, &offered).await {
        Some((data, mime)) if !data.is_empty() && data.len() as u64 <= MAX_PASTE_BYTES => {
            let extension = mime.split('/').nth(1).unwrap_or("png").split('+').next().unwrap_or("png").to_string();
            reply(vec![
                json!({"name": format!("image.{extension}"), "mimeType": mime, "dataBase64": b64().encode(&data)}),
            ])
        }
        _ => reply(Vec::new()),
    }
}

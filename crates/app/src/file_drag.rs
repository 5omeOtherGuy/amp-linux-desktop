//! Dragging a file out of the page into another app, as the macOS app's
//! MacFileDragMessageHandler does.
//!
//! The page asks for a drag with `ampNativeFileDrag {action: 'start', id, fileName,
//! previewDataURL}` and hands over no path or bytes. The app starts a desktop drag whose content
//! is a file in a private folder under the cache dir. The bytes are fetched once, the first time a
//! drop target asks for the content: the page answers `read` events (`amp:native-file-drag`) with
//! 1 MiB chunks, and every target that asked gets the file's URI once all of it is on disk. The
//! app's own web view asks as soon as the drag crosses it, so the fetch usually starts with the
//! drag.
//!
//! A dropped file stays until the app quits, because the target may copy it, or upload it, after
//! the drop has finished; a cancelled drag's folder goes at once, and startup removes what a
//! crash left behind.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use amp_common::config::PATHS;
use amp_common::scripts;
use base64::Engine;
use gtk::subclass::prelude::*;
use gtk::{gdk, gio, glib, prelude::*};
use serde_json::{Value, json};
use webkit::prelude::*;

use crate::bridge::Reply;
use crate::window::AmpWindow;

const EVENT: &str = "amp:native-file-drag";

/// Asks the page for the bytes from `offset`; resolves to `{base64, size}`. The page cancels the
/// event when it knows the drag, so an unknown one rejects instead of never settling.
const READ_CHUNK: &str = r#"return new Promise((resolve, reject) => {
  const event = new CustomEvent('amp:native-file-drag', {
    cancelable: true,
    detail: {type: 'read', id, offset, resolve, reject},
  });
  window.dispatchEvent(event);
  if (!event.defaultPrevented) reject(new Error('The dragged file is no longer available'));
});"#;

/// The longest side of the drag image, in logical pixels.
const ICON_SIZE: i32 = 128;

thread_local! {
    /// Drags in progress, held until they end.
    static ACTIVE: RefCell<HashMap<String, gdk::Drag>> = RefCell::default();
}

fn drags_dir() -> PathBuf {
    PATHS.cache_dir.join("file-drags")
}

/// Removes every dragged file; run at startup and shutdown.
pub fn remove_all() {
    let _ = std::fs::remove_dir_all(drags_dir());
}

/// Answers `ampNativeFileDrag`: `true` once a desktop drag has started.
pub fn handle(window: &Rc<AmpWindow>, body: &Value, reply: Reply) {
    if body.get("action").and_then(Value::as_str) != Some("start") {
        reply.send(Ok(json!(false)));
        return;
    }
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    let name = body.get("fileName").and_then(Value::as_str).and_then(file_name);
    let (Some(id), Some(name)) = (id, name) else {
        reply.send(Ok(json!(false)));
        return;
    };
    let preview = body.get("previewDataURL").and_then(Value::as_str);
    reply.send(Ok(json!(begin(window, id, &name, preview))));
}

/// The last component of the page's name, safe to create in the drag's folder.
fn file_name(name: &str) -> Option<String> {
    let name = name.rsplit('/').next()?.trim().replace('\0', "");
    (!name.is_empty() && name != "." && name != "..").then_some(name)
}

fn begin(window: &Rc<AmpWindow>, id: &str, name: &str, preview: Option<&str>) -> bool {
    let Some(surface) = window.window.surface() else { return false };
    let Some(device) = WidgetExt::display(&window.window).default_seat().and_then(|seat| seat.pointer()) else {
        return false;
    };
    let dir = drags_dir().join(id);
    let transfer = Rc::new(Transfer {
        id: id.to_string(),
        path: dir.join(name),
        dir,
        window: Rc::downgrade(window),
        fetch: RefCell::new(Fetch::Idle),
        outcome: Cell::new(None),
    });
    let content = FileDragContent::new(transfer.clone());
    let Some(drag) = gdk::Drag::begin(&surface, &device, &content, gdk::DragAction::COPY, 0.0, 0.0) else {
        log::warn!("could not start a file drag");
        return false;
    };
    // The drag takes the pointer, so the press that started it is never released in the web
    // view; its click gesture would then swallow the next press. A GtkDragSource resets the
    // widget's other gestures when it claims the press; this does the same.
    let controllers = window.webview.observe_controllers();
    for gesture in (0..controllers.n_items()).filter_map(|i| controllers.item(i).and_downcast::<gtk::Gesture>()) {
        gesture.reset();
    }
    let (icon, size) = drag_icon(name, preview);
    gtk::DragIcon::for_drag(&drag).set_child(Some(&icon));
    drag.set_hotspot(size / 2, size / 2);
    let finished = transfer.clone();
    drag.connect_dnd_finished(move |drag| finished.end(drag, true));
    drag.connect_cancel(move |drag, _| transfer.end(drag, false));
    ACTIVE.with(|active| active.borrow_mut().insert(id.to_string(), drag));
    true
}

/// The page's preview, scaled to fit `ICON_SIZE`, or the icon of the file's type, with its size.
fn drag_icon(name: &str, preview: Option<&str>) -> (gtk::Image, i32) {
    let texture = preview
        .and_then(|url| url.strip_prefix("data:image/png;base64,"))
        .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())
        .and_then(|bytes| gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).ok());
    let (image, size) = match texture {
        Some(texture) => (gtk::Image::from_paintable(Some(&texture)), ICON_SIZE),
        None => {
            let (content_type, _) = gio::content_type_guess(Some(name), None);
            (gtk::Image::from_gicon(&gio::content_type_get_icon(&content_type)), 64)
        }
    };
    image.set_pixel_size(size);
    (image, size)
}

enum Fetch {
    Idle,
    Running(Vec<async_channel::Sender<Result<PathBuf, String>>>),
    Done(Result<PathBuf, String>),
}

struct Transfer {
    id: String,
    dir: PathBuf,
    path: PathBuf,
    window: Weak<AmpWindow>,
    fetch: RefCell<Fetch>,
    /// Whether the drag ended in a drop, once it has ended.
    outcome: Cell<Option<bool>>,
}

impl Transfer {
    /// The file on disk, fetched from the page on the first call.
    async fn file(self: Rc<Self>) -> Result<PathBuf, String> {
        let receiver = {
            let mut fetch = self.fetch.borrow_mut();
            let (sender, receiver) = async_channel::bounded(1);
            match &mut *fetch {
                Fetch::Done(result) => return result.clone(),
                Fetch::Running(waiters) => waiters.push(sender),
                Fetch::Idle => {
                    *fetch = Fetch::Running(vec![sender]);
                    glib::spawn_future_local(self.clone().fetch());
                }
            }
            receiver
        };
        receiver.recv().await.unwrap_or_else(|_| Err("the drag ended".into()))
    }

    async fn fetch(self: Rc<Self>) {
        let result = self.read_all().await;
        match &result {
            // A cancelled drag's bytes are gone from the page; the web view's own drop target
            // starts a fetch as soon as the drag passes over it.
            Err(error) if self.outcome.get() == Some(false) => log::debug!("dragged file not fetched: {error}"),
            Err(error) => log::warn!("could not fetch the dragged file: {error}"),
            Ok(_) => {}
        }
        let previous = std::mem::replace(&mut *self.fetch.borrow_mut(), Fetch::Done(result.clone()));
        if let Fetch::Running(waiters) = previous {
            for waiter in waiters {
                let _ = waiter.try_send(result.clone());
            }
        }
        if self.outcome.get() == Some(true) {
            self.dispatch(json!({"type": "release", "id": self.id}));
        }
    }

    async fn read_all(&self) -> Result<PathBuf, String> {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&self.dir).map_err(|e| e.to_string())?;
        let mut file = std::fs::File::create(&self.path).map_err(|e| e.to_string())?;
        let mut offset: u64 = 0;
        loop {
            let window = self.window.upgrade().ok_or("the window closed")?;
            let arguments = glib::VariantDict::new(None);
            arguments.insert("id", self.id.as_str());
            arguments.insert("offset", offset as f64);
            let value = window
                .webview
                .call_async_javascript_function_future(READ_CHUNK, Some(&arguments.end()), None, None)
                .await
                .map_err(|e| e.message().to_string())?;
            let chunk: Value = value.to_json(0).and_then(|j| serde_json::from_str(j.as_str()).ok()).unwrap_or_default();
            let size = chunk.get("size").and_then(Value::as_u64).ok_or("the page sent no size")?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(chunk.get("base64").and_then(Value::as_str).unwrap_or_default())
                .map_err(|e| e.to_string())?;
            file.write_all(&bytes).map_err(|e| e.to_string())?;
            offset += bytes.len() as u64;
            if offset >= size {
                return Ok(self.path.clone());
            }
            if bytes.is_empty() {
                return Err("the page sent fewer bytes than the file's size".into());
            }
        }
    }

    fn end(&self, drag: &gdk::Drag, dropped: bool) {
        if self.outcome.replace(Some(dropped)).is_some() {
            return;
        }
        drag.drop_done(dropped);
        ACTIVE.with(|active| active.borrow_mut().remove(&self.id));
        self.dispatch(json!({"type": "ended", "id": self.id, "dropped": dropped}));
        if !dropped {
            // The page forgets a cancelled drag's bytes itself.
            let _ = std::fs::remove_dir_all(&self.dir);
        } else if !matches!(*self.fetch.borrow(), Fetch::Running(_)) {
            // Fetched, or never asked for: the page can drop its copy now. A fetch still
            // running releases it when it is done.
            self.dispatch(json!({"type": "release", "id": self.id}));
        }
    }

    fn dispatch(&self, detail: Value) {
        if let Some(window) = self.window.upgrade() {
            window.run_js(&scripts::custom_event(EVENT, &detail));
        }
    }
}

fn file_list(path: &std::path::Path) -> gdk::FileList {
    gdk::FileList::from_array(&[gio::File::for_path(path)])
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct FileDragContent {
        pub(super) transfer: OnceCell<Rc<Transfer>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FileDragContent {
        const NAME: &'static str = "AmpFileDragContent";
        type Type = super::FileDragContent;
        type ParentType = gdk::ContentProvider;
    }

    impl ObjectImpl for FileDragContent {}

    impl gdk::subclass::prelude::ContentProviderImpl for FileDragContent {
        fn formats(&self) -> gdk::ContentFormats {
            gdk::ContentFormats::for_type(gdk::FileList::static_type()).union_serialize_mime_types()
        }

        fn write_mime_type_future(
            &self,
            mime_type: &str,
            stream: &gio::OutputStream,
            io_priority: glib::Priority,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), glib::Error>> + 'static>> {
            let transfer = self.transfer.get().cloned();
            let mime_type = mime_type.to_string();
            let stream = stream.clone();
            Box::pin(async move {
                let transfer = transfer.ok_or_else(|| glib::Error::new(gio::IOErrorEnum::Failed, "no file"))?;
                let path = transfer.file().await.map_err(|e| glib::Error::new(gio::IOErrorEnum::Failed, &e))?;
                gdk::content_serialize_future(&stream, &mime_type, &file_list(&path).to_value(), io_priority).await
            })
        }

        fn value(&self, type_: glib::Type) -> Result<glib::Value, glib::Error> {
            if let Some(transfer) = self.transfer.get()
                && type_ == gdk::FileList::static_type()
                && let Fetch::Done(Ok(path)) = &*transfer.fetch.borrow()
            {
                return Ok(file_list(path).to_value());
            }
            Err(glib::Error::new(gio::IOErrorEnum::NotSupported, "the file is not fetched yet"))
        }
    }
}

glib::wrapper! {
    pub struct FileDragContent(ObjectSubclass<imp::FileDragContent>) @extends gdk::ContentProvider;
}

impl FileDragContent {
    fn new(transfer: Rc<Transfer>) -> Self {
        let content: Self = glib::Object::new();
        let _ = content.imp().transfer.set(transfer);
        content
    }
}

//! Huddles through the LiveKit SDK, for WebKitGTK builds without WebRTC.
//!
//! Distribution WebKitGTK is built with ENABLE_WEB_RTC off, so the page's livekit-client cannot
//! connect. The page still asks Amp for the call's token (POST /api/threads/<id>/space-token with
//! an empty body); the HUDDLE page script hands that token to ampNativeHuddle and this crate joins
//! the room natively: the microphone and speakers go through libwebrtc's audio device module (echo
//! cancellation included), the camera through GStreamer, and remote video into a GTK window.
//!
//! The room and identity are the same ones the page would use, so others see an ordinary
//! participant. Not covered: screen sharing from Linux and the page's per-phrase transcription
//! upload; the window says when a call is being transcribed.

mod call;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use call::{Command, Event, LOCAL};
use gtk::{gdk, glib};

/// What the page sent to join: `ampNativeHuddle { command: "join", ... }`.
pub struct JoinRequest {
    pub thread_id: String,
    pub title: String,
    /// "Huddle" or "Space", as the page calls the call.
    pub call_name: String,
    pub server_url: String,
    pub participant_token: String,
}

/// What the call window needs from the app.
pub struct AppHooks {
    /// Asks the server to update the thread's live call state, as the page does after it joins
    /// or leaves.
    pub reconcile: Box<dyn Fn()>,
    /// The window closed; the message says why when the call ended without the user leaving.
    pub ended: Box<dyn Fn(Option<String>)>,
}

struct Tile {
    root: gtk::Overlay,
    avatar: adw::Avatar,
    picture: gtk::Picture,
    muted: gtk::Image,
    ring: gtk::Box,
}

impl Tile {
    fn new(label: &str, name: Option<&str>) -> Self {
        let root = gtk::Overlay::new();
        root.add_css_class("huddle-tile");
        root.set_size_request(320, 180);
        let avatar = adw::Avatar::new(72, Some(name.unwrap_or(label)), true);
        avatar.set_halign(gtk::Align::Center);
        avatar.set_valign(gtk::Align::Center);
        let picture =
            gtk::Picture::builder().content_fit(gtk::ContentFit::Cover).can_shrink(true).visible(false).build();
        root.set_child(Some(&gtk::Box::builder().hexpand(true).vexpand(true).build()));
        root.add_overlay(&picture);
        root.add_overlay(&avatar);
        let caption = gtk::Box::builder()
            .spacing(4)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::End)
            .margin_start(8)
            .margin_bottom(8)
            .build();
        caption.add_css_class("huddle-caption");
        let muted = gtk::Image::builder().icon_name("microphone-disabled-symbolic").visible(false).build();
        caption.append(&muted);
        caption.append(&gtk::Label::builder().label(label).ellipsize(gtk::pango::EllipsizeMode::End).build());
        root.add_overlay(&caption);
        let ring = gtk::Box::builder().visible(false).can_target(false).build();
        ring.add_css_class("huddle-ring");
        root.add_overlay(&ring);
        Self { root, avatar, picture, muted, ring }
    }

    fn show_frame(&self, width: u32, height: u32, data: Vec<u8>) {
        let texture = gdk::MemoryTexture::new(
            width as i32,
            height as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(data),
            width as usize * 4,
        );
        self.picture.set_paintable(Some(&texture));
        self.picture.set_visible(true);
        self.avatar.set_visible(false);
    }

    fn clear_video(&self) {
        self.picture.set_paintable(gdk::Paintable::NONE);
        self.picture.set_visible(false);
        self.avatar.set_visible(true);
    }
}

struct Inner {
    window: adw::Window,
    hooks: AppHooks,
    call_name: String,
    commands: tokio::sync::mpsc::UnboundedSender<Command>,
    runtime: RefCell<Option<tokio::runtime::Runtime>>,
    local_identity: RefCell<Option<String>>,
    tiles: RefCell<HashMap<String, Tile>>,
    grid: gtk::FlowBox,
    banner: gtk::Label,
    status: gtk::Label,
    toasts: adw::ToastOverlay,
    mic_button: gtk::ToggleButton,
    camera_button: gtk::ToggleButton,
    /// Set while the window updates its buttons, so the change is not sent back to the call.
    updating: Cell<bool>,
    left: Cell<bool>,
    end_message: RefCell<Option<String>>,
    /// Receives once the call has disconnected after leaving.
    disconnected: RefCell<Option<std::sync::mpsc::Receiver<()>>>,
}

/// The call window: a tile per participant (and per shared screen), and mute, camera and leave
/// buttons.
#[derive(Clone)]
pub struct HuddleWindow(Rc<Inner>);

impl HuddleWindow {
    pub fn new(app: &adw::Application, request: JoinRequest, hooks: AppHooks) -> Self {
        let window = adw::Window::builder()
            .application(app)
            .default_width(760)
            .default_height(520)
            .title(&request.call_name)
            .build();
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&adw::WindowTitle::new(&request.call_name, &request.title)));
        let banner = gtk::Label::builder().wrap(true).visible(false).margin_top(6).margin_bottom(6).build();
        banner.add_css_class("huddle-banner");
        let grid = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .max_children_per_line(3)
            .min_children_per_line(1)
            .row_spacing(8)
            .column_spacing(8)
            .valign(gtk::Align::Center)
            .margin_start(12)
            .margin_end(12)
            .margin_top(12)
            .margin_bottom(12)
            .build();
        let scroller =
            gtk::ScrolledWindow::builder().vexpand(true).child(&grid).hscrollbar_policy(gtk::PolicyType::Never).build();
        let mic_button = gtk::ToggleButton::builder()
            .icon_name("audio-input-microphone-symbolic")
            .tooltip_text("Mute")
            .sensitive(false)
            .active(true)
            .build();
        let camera_button =
            gtk::ToggleButton::builder().icon_name("camera-web-symbolic").tooltip_text("Turn Camera On").build();
        let leave = gtk::Button::with_label("Leave");
        leave.add_css_class("destructive-action");
        leave.add_css_class("pill");
        for button in [&mic_button, &camera_button] {
            button.add_css_class("circular");
            button.add_css_class("huddle-control");
        }
        let controls =
            gtk::Box::builder().spacing(12).halign(gtk::Align::Center).margin_top(6).margin_bottom(12).build();
        controls.append(&mic_button);
        controls.append(&camera_button);
        controls.append(&leave);
        let status = gtk::Label::builder().label("Joining…").margin_bottom(4).build();
        status.add_css_class("dim-label");
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&header);
        column.append(&banner);
        column.append(&scroller);
        column.append(&status);
        column.append(&controls);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&column));
        window.set_content(Some(&toasts));

        let (events_tx, events_rx) = async_channel::unbounded();
        let (commands, commands_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("huddle")
            .enable_all()
            .build()
            .expect("a tokio runtime for the call");
        runtime.spawn(call::run(request.server_url, request.participant_token, events_tx, commands_rx));

        let huddle = Self(Rc::new(Inner {
            window,
            hooks,
            call_name: request.call_name,
            commands,
            runtime: RefCell::new(Some(runtime)),
            local_identity: RefCell::default(),
            tiles: RefCell::default(),
            grid,
            banner,
            status,
            toasts,
            mic_button,
            camera_button,
            updating: Cell::new(false),
            left: Cell::new(false),
            end_message: RefCell::default(),
            disconnected: RefCell::default(),
        }));
        let inner = &huddle.0;
        let weak = Rc::downgrade(inner);
        inner.mic_button.connect_toggled(move |b| {
            if let Some(inner) = weak.upgrade().filter(|i| !i.updating.get()) {
                let _ = inner.commands.send(Command::SetMicrophone(b.is_active()));
            }
        });
        let weak = Rc::downgrade(inner);
        inner.camera_button.connect_toggled(move |b| {
            if let Some(inner) = weak.upgrade().filter(|i| !i.updating.get()) {
                let _ = inner.commands.send(Command::SetCamera(b.is_active()));
            }
        });
        let window = inner.window.clone();
        leave.connect_clicked(move |_| window.close());
        let weak = Rc::downgrade(inner);
        inner.window.connect_close_request(move |_| {
            if let Some(inner) = weak.upgrade() {
                HuddleWindow(inner).close_request();
            }
            glib::Propagation::Proceed
        });
        let weak = Rc::downgrade(inner);
        glib::spawn_future_local(async move {
            while let Ok(event) = events_rx.recv().await {
                let Some(inner) = weak.upgrade() else { break };
                HuddleWindow(inner).handle(event);
            }
        });
        huddle
    }

    pub fn present(&self) {
        self.0.window.present();
    }

    pub fn close(&self) {
        self.0.window.close();
    }

    pub fn is(&self, other: &HuddleWindow) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    /// Blocks until the call has disconnected, so quitting Amp does not leave a ghost participant
    /// behind until the server times it out.
    pub fn wait_left(&self, timeout: Duration) {
        if let Some(receiver) = self.0.disconnected.borrow().as_ref() {
            let _ = receiver.recv_timeout(timeout);
        }
    }

    fn close_request(&self) {
        let inner = &self.0;
        if inner.left.replace(true) {
            return;
        }
        let (done, disconnected) = std::sync::mpsc::channel();
        *inner.disconnected.borrow_mut() = Some(disconnected);
        // The call task reports Left; this thread may block on `disconnected` while quitting, so
        // a watcher on the runtime signals it.
        let _ = inner.commands.send(Command::Leave);
        if let Some(runtime) = inner.runtime.borrow().as_ref() {
            let commands = inner.commands.clone();
            runtime.spawn(async move {
                commands.closed().await;
                let _ = done.send(());
            });
        }
        (inner.hooks.ended)(inner.end_message.take());
    }

    fn handle(&self, event: Event) {
        let inner = &self.0;
        match event {
            Event::Connected { identity, name } => {
                let name = if name.is_empty() { identity.clone() } else { name };
                self.tile(&identity, &format!("{name} (You)"), Some(&name));
                *inner.local_identity.borrow_mut() = Some(identity);
                inner.status.set_visible(false);
                (inner.hooks.reconcile)();
            }
            Event::Failed(message) | Event::CallEnded(Some(message)) => self.call_ended(Some(message)),
            Event::CallEnded(None) => self.call_ended(None),
            Event::ParticipantChanged { identity, name, muted } => {
                self.tile(&identity, &name, None);
                if let Some(tile) = inner.tiles.borrow().get(&identity) {
                    tile.muted.set_visible(muted);
                }
            }
            Event::ParticipantLeft(identity) => {
                let keys: Vec<String> = inner
                    .tiles
                    .borrow()
                    .keys()
                    .filter(|k| **k == identity || k.starts_with(&format!("{identity}/")))
                    .cloned()
                    .collect();
                for key in keys {
                    self.remove(&key);
                }
            }
            Event::MediaChanged { mic_available, mic_on, camera_on, problem } => {
                inner.updating.set(true);
                inner.mic_button.set_active(mic_on);
                inner.camera_button.set_active(camera_on);
                inner.updating.set(false);
                inner.mic_button.set_sensitive(mic_available);
                inner.mic_button.set_icon_name(if mic_on {
                    "audio-input-microphone-symbolic"
                } else {
                    "microphone-disabled-symbolic"
                });
                inner.mic_button.set_tooltip_text(Some(if mic_on { "Mute" } else { "Unmute" }));
                inner.camera_button.set_icon_name(if camera_on {
                    "camera-web-symbolic"
                } else {
                    "camera-disabled-symbolic"
                });
                inner.camera_button.set_tooltip_text(Some(if camera_on {
                    "Turn Camera Off"
                } else {
                    "Turn Camera On"
                }));
                if let Some(local) = inner.local_identity.borrow().as_ref()
                    && let Some(tile) = inner.tiles.borrow().get(local)
                {
                    tile.muted.set_visible(!mic_on);
                }
                if let Some(problem) = problem {
                    inner.toasts.add_toast(adw::Toast::builder().title(problem).timeout(4).build());
                }
            }
            Event::SpeakersChanged(identities) => {
                for (key, tile) in inner.tiles.borrow().iter() {
                    tile.ring.set_visible(identities.contains(key));
                }
            }
            Event::MetadataChanged(metadata) => {
                let active =
                    serde_json::from_str::<serde_json::Value>(if metadata.is_empty() { "{}" } else { &metadata })
                        .ok()
                        .and_then(|m| m.get("transcript")?.get("active").cloned())
                        .is_some_and(|a| a.as_bool().unwrap_or(!a.is_null()));
                inner.banner.set_label(&format!(
                    "This {} is being transcribed. The transcript will be in Artifacts when the call ends; \
                     speech from this Linux window isn’t included yet.",
                    inner.call_name
                ));
                inner.banner.set_visible(active);
            }
            Event::StatusChanged(text) => {
                inner.status.set_label(text.as_deref().unwrap_or(""));
                inner.status.set_visible(text.is_some());
            }
            Event::VideoStarted { key, label } => {
                if !key.ends_with("/camera") {
                    self.tile(&key, &label, None);
                }
            }
            Event::VideoFrame { key, width, height, data } => {
                if let Some(key) = self.tile_key(&key)
                    && let Some(tile) = inner.tiles.borrow().get(&key)
                {
                    tile.show_frame(width, height, data);
                }
            }
            Event::VideoEnded(key) => {
                let Some(key) = self.resolve_local(&key) else { return };
                match key.split_once('/') {
                    Some((identity, source)) if source != "camera" => {
                        let _ = identity;
                        self.remove(&key);
                    }
                    Some((identity, _)) => self.clear_video(identity),
                    None => self.clear_video(&key),
                }
            }
            Event::Left => {
                (inner.hooks.reconcile)();
                if let Some(runtime) = inner.runtime.take() {
                    runtime.shutdown_background();
                }
            }
        }
    }

    fn resolve_local(&self, key: &str) -> Option<String> {
        if key == LOCAL { self.0.local_identity.borrow().clone() } else { Some(key.to_string()) }
    }

    /// The tile that shows `key`'s frames: a participant's own tile for their camera.
    fn tile_key(&self, key: &str) -> Option<String> {
        let key = self.resolve_local(key)?;
        Some(match key.strip_suffix("/camera") {
            Some(identity) => identity.to_string(),
            None => key,
        })
    }

    fn clear_video(&self, identity: &str) {
        if let Some(tile) = self.0.tiles.borrow().get(identity) {
            tile.clear_video();
        }
    }

    /// The room went away without the user leaving: show why in the main window.
    fn call_ended(&self, message: Option<String>) {
        if !self.0.left.get() {
            *self.0.end_message.borrow_mut() = message;
            self.0.window.close();
        }
    }

    fn tile(&self, key: &str, label: &str, name: Option<&str>) {
        let mut tiles = self.0.tiles.borrow_mut();
        if !tiles.contains_key(key) {
            let tile = Tile::new(label, name);
            self.0.grid.append(&tile.root);
            tiles.insert(key.to_string(), tile);
        }
    }

    fn remove(&self, key: &str) {
        if let Some(tile) = self.0.tiles.borrow_mut().remove(key) {
            self.0.grid.remove(&tile.root);
        }
    }
}

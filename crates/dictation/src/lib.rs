//! Native microphone capture for the page's dictation (ampNativeDictation, version 1).
//!
//! WebKitGTK gives the page getUserMedia but its MediaRecorder needs GStreamer 1.24.9 or newer for
//! WebM and gst-plugins-rs for fragmented MP4 (MediaRecorderPrivateGStreamer.cpp), so on older
//! stacks the page's browser dictation records nothing. This records with GStreamer directly.
//!
//! The page sends { command, requestID, ... } and listens for amp:native-dictation events
//! { version: 1, type, requestID, ... }, as the Mac app's AVCapture recorder answers them:
//! - listSources: sources { sources: [{ deviceID, label }] }; sourcesChanged when devices change.
//! - start { deviceID, requestStartedAtEpochMs }: started, firstBuffer, firstNonzeroBuffer, level,
//!   and on stop finished { base64, mimeType, ... }; canceled after cancel; error on failure.
//! - openMicrophoneSettings: the desktop's sound input settings.
//!
//! The page plays its own start and stop cues (nativeCues false) and uploads the audio itself
//! (uploadStreaming false).

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::str::FromStr;

use base64::Engine;
use gst::prelude::*;
use serde_json::{Map, Value, json};

const MIME_TYPE: &str = "audio/webm;codecs=opus";
const LEVEL_INTERVAL_NS: u64 = 50_000_000;
const SETTINGS_APPS: [&[&str]; 3] =
    [&["pavucontrol", "--tab=4"], &["gnome-control-center", "sound"], &["systemsettings", "kcm_pulseaudio"]];

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Whether this computer can record: GStreamer with an Opus encoder and a WebM muxer.
pub fn available() -> bool {
    gst::init().is_ok() && ["opusenc", "webmmux", "level"].iter().all(|n| gst::ElementFactory::find(n).is_some())
}

type Emit = Box<dyn Fn(String)>;

struct Inner {
    /// Runs a script in the page.
    emit: Emit,
    monitor: RefCell<Option<(gst::DeviceMonitor, gst::bus::BusWatchGuard)>>,
    known: RefCell<Option<BTreeSet<String>>>,
    session: RefCell<Option<Rc<Session>>>,
}

#[derive(Clone)]
pub struct Dictation(Rc<Inner>);

impl Dictation {
    /// `run_js` runs a script in the page that dictates.
    pub fn new(run_js: impl Fn(String) + 'static) -> Self {
        let _ = gst::init();
        Self(Rc::new(Inner {
            emit: Box::new(run_js),
            monitor: RefCell::default(),
            known: RefCell::default(),
            session: RefCell::default(),
        }))
    }

    pub fn handle(&self, body: &Value) {
        let Some(request) = body.get("requestID").and_then(Value::as_str) else { return };
        let current = || self.0.session.borrow().clone().filter(|s| s.request == request);
        match body.get("command").and_then(Value::as_str) {
            Some("listSources") => {
                let sources = self.0.sources();
                self.0.emit("sources", request, json!({"sources": sources}));
            }
            Some("start") => self.start(request, body),
            Some("stop") => {
                if let Some(session) = current() {
                    session.stop();
                }
            }
            Some("cancel") => {
                if let Some(session) = current() {
                    session.cancel();
                }
            }
            Some("openMicrophoneSettings") => {
                open_microphone_settings();
            }
            _ => {}
        }
    }

    pub fn close(&self) {
        let session = self.0.session.borrow().clone();
        if let Some(session) = session {
            session.cancel();
        }
        if let Some((monitor, _watch)) = self.0.monitor.take() {
            monitor.stop();
        }
    }

    fn start(&self, request: &str, body: &Value) {
        let received = now_ms();
        let previous = self.0.session.borrow().clone();
        if let Some(session) = previous {
            session.cancel();
        }
        if !available() {
            let message = "Dictation needs GStreamer with the Opus and WebM plugins.";
            self.0.emit("error", request, json!({"message": message}));
            return;
        }
        let Some((source, label)) = self.0.source_for(body.get("deviceID").and_then(Value::as_str)) else {
            let fields = json!({"message": "No microphone was found.", "code": "microphone-unavailable"});
            self.0.emit("error", request, fields);
            return;
        };
        let requested =
            body.get("requestStartedAtEpochMs").filter(|v| v.is_number()).cloned().unwrap_or(json!(received));
        match Session::new(&self.0, request, source, label, requested, received) {
            Ok(session) => {
                *self.0.session.borrow_mut() = Some(session.clone());
                session.start();
            }
            Err(error) => {
                log::warn!("dictation pipeline could not be built: {error}");
                self.0.emit("error", request, json!({"message": "The microphone could not be opened."}));
            }
        }
    }
}

impl Inner {
    fn emit(&self, kind: &str, request: &str, fields: Value) {
        let mut detail = json!({"version": 1, "type": kind, "requestID": request});
        if let (Some(detail), Value::Object(fields)) = (detail.as_object_mut(), fields) {
            detail.extend(fields);
        }
        (self.emit)(format!("window.dispatchEvent(new CustomEvent('amp:native-dictation', {{ detail: {detail} }}))"));
    }

    fn devices(self: &Rc<Self>) -> Vec<gst::Device> {
        if self.monitor.borrow().is_none() {
            let monitor = gst::DeviceMonitor::new();
            monitor.add_filter(Some("Audio/Source"), None);
            let weak = Rc::downgrade(self);
            let watch = monitor.bus().add_watch_local(move |_, message| {
                if let Some(inner) = weak.upgrade() {
                    inner.device_message(message);
                }
                glib::ControlFlow::Continue
            });
            if monitor.start().is_err() {
                log::warn!("could not start the microphone monitor");
            }
            *self.known.borrow_mut() = None;
            match watch {
                Ok(watch) => *self.monitor.borrow_mut() = Some((monitor, watch)),
                Err(_) => return Vec::new(),
            }
        }
        let monitor = self.monitor.borrow().as_ref().map(|(m, _)| m.clone());
        monitor
            .map(|m| m.devices())
            .into_iter()
            .flatten()
            // A monitor device is what a speaker plays, not a microphone.
            .filter(|d| d.properties().and_then(|p| p.get::<String>("device.class").ok()).as_deref() != Some("monitor"))
            .collect()
    }

    fn device_message(self: &Rc<Self>, message: &gst::Message) {
        if matches!(message.view(), gst::MessageView::DeviceAdded(_) | gst::MessageView::DeviceRemoved(_)) {
            // The monitor also announces the devices it finds while starting; report real changes.
            let ids: BTreeSet<String> = self.devices().iter().map(device_id).collect();
            if self.known.borrow().as_ref() != Some(&ids) {
                *self.known.borrow_mut() = Some(ids);
                self.emit("sourcesChanged", "devices", json!({}));
            }
        }
    }

    fn sources(self: &Rc<Self>) -> Vec<Value> {
        let devices = self.devices();
        *self.known.borrow_mut() = Some(devices.iter().map(device_id).collect());
        devices.iter().map(|d| json!({"deviceID": device_id(d), "label": d.display_name().as_str()})).collect()
    }

    fn source_for(self: &Rc<Self>, device_id_: Option<&str>) -> Option<(gst::Element, String)> {
        let devices = self.devices();
        let chosen = device_id_
            .filter(|id| !id.is_empty())
            .and_then(|id| devices.iter().find(|d| device_id(d) == id))
            .or_else(|| {
                devices.iter().find(|d| d.properties().and_then(|p| p.get::<bool>("is-default").ok()).unwrap_or(false))
            });
        if let Some(device) = chosen
            && let Ok(element) = device.create_element(None)
        {
            return Some((element, device.display_name().to_string()));
        }
        ["pulsesrc", "pipewiresrc", "autoaudiosrc"]
            .iter()
            .find_map(|n| gst::ElementFactory::make(n).build().ok())
            .map(|e| (e, "Default Microphone".to_string()))
    }

    fn ended(&self, session: &Session) {
        let mut current = self.session.borrow_mut();
        if current.as_ref().is_some_and(|s| std::ptr::eq(s.as_ref(), session)) {
            *current = None;
        }
    }
}

fn device_id(device: &gst::Device) -> String {
    if device.find_property("internal-name").is_some()
        && let Ok(Some(name)) = device.property_value("internal-name").get::<Option<String>>()
        && !name.is_empty()
    {
        return name;
    }
    device.display_name().to_string()
}

struct Session {
    owner: Weak<Inner>,
    request: String,
    label: String,
    requested_at: Value,
    received_at: i64,
    first_buffer_at: Cell<Option<i64>>,
    first_signal_at: Cell<Option<i64>>,
    stop_requested_at: Cell<Option<i64>>,
    samples: Cell<u64>,
    peak: Cell<f64>,
    done: Cell<bool>,
    path: PathBuf,
    pipeline: gst::Pipeline,
    watch: RefCell<Option<gst::bus::BusWatchGuard>>,
}

impl Session {
    fn new(
        owner: &Rc<Inner>,
        request: &str,
        source: gst::Element,
        label: String,
        requested_at: Value,
        received_at: i64,
    ) -> Result<Rc<Self>, glib::BoolError> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("amp-dictation-{}-{nanos}.webm", std::process::id()));
        let pipeline = gst::Pipeline::with_name("amp-dictation");
        let make = |name: &str| gst::ElementFactory::make(name).build();
        let caps = gst::Caps::from_str("audio/x-raw,channels=1,rate=48000")?;
        let capsfilter = gst::ElementFactory::make("capsfilter").property("caps", &caps).build()?;
        let level = gst::ElementFactory::make("level")
            .property("interval", LEVEL_INTERVAL_NS)
            .property("post-messages", true)
            .build()?;
        let opus = gst::ElementFactory::make("opusenc").property("bitrate", 32000i32).build()?;
        let sink =
            gst::ElementFactory::make("filesink").property("location", path.to_string_lossy().as_ref()).build()?;
        let elements =
            [source, make("audioconvert")?, make("audioresample")?, capsfilter, level, opus, make("webmmux")?, sink];
        pipeline.add_many(&elements)?;
        gst::Element::link_many(&elements)?;

        let session = Rc::new(Self {
            owner: Rc::downgrade(owner),
            request: request.to_string(),
            label,
            requested_at,
            received_at,
            first_buffer_at: Cell::default(),
            first_signal_at: Cell::default(),
            stop_requested_at: Cell::default(),
            samples: Cell::default(),
            peak: Cell::default(),
            done: Cell::default(),
            path,
            pipeline: pipeline.clone(),
            watch: RefCell::default(),
        });
        let weak = Rc::downgrade(&session);
        let watch =
            pipeline.bus().expect("a pipeline has a bus").add_watch_local(move |_, message| match weak.upgrade() {
                Some(session) => session.message(message),
                None => glib::ControlFlow::Break,
            })?;
        *session.watch.borrow_mut() = Some(watch);
        Ok(session)
    }

    fn emit(&self, kind: &str, fields: Value) {
        if let Some(owner) = self.owner.upgrade() {
            owner.emit(kind, &self.request, fields);
        }
    }

    fn start(&self) {
        let prepared = now_ms();
        if self.pipeline.set_state(gst::State::Playing).is_err() {
            self.fail("The microphone could not be opened.");
            return;
        }
        self.emit(
            "started",
            json!({
                "nativeCues": false,
                "uploadStreaming": false,
                "requestStartedAtEpochMs": self.requested_at,
                "nativeRequestReceivedAtEpochMs": self.received_at,
                "nativePreparationStartedAtEpochMs": self.received_at,
                "nativePreparedAtEpochMs": prepared,
                "nativeRecordStartedAtEpochMs": now_ms(),
                "processing": "none",
                "deviceLabel": self.label,
            }),
        );
    }

    fn stop(&self) {
        if self.done.get() || self.stop_requested_at.get().is_some() {
            return;
        }
        self.stop_requested_at.set(Some(now_ms()));
        self.pipeline.send_event(gst::event::Eos::new());
    }

    fn cancel(&self) {
        if self.done.get() {
            return;
        }
        self.teardown(false);
        self.emit("canceled", json!({}));
    }

    fn message(&self, message: &gst::Message) -> glib::ControlFlow {
        if self.done.get() {
            return glib::ControlFlow::Break;
        }
        match message.view() {
            gst::MessageView::Element(element) => {
                if let Some(s) = element.structure().filter(|s| s.name() == "level") {
                    self.level(s);
                }
            }
            gst::MessageView::Eos(_) => {
                self.finish();
                return glib::ControlFlow::Break;
            }
            gst::MessageView::Error(error) => {
                log::warn!("dictation pipeline failed: {} ({:?})", error.error(), error.debug());
                self.fail(&error.error().to_string());
                return glib::ControlFlow::Break;
            }
            _ => {}
        }
        glib::ControlFlow::Continue
    }

    fn level(&self, structure: &gst::StructureRef) {
        let decibels: Vec<f64> = structure
            .get::<glib::ValueArray>("rms")
            .map(|a| a.iter().filter_map(|v| v.get::<f64>().ok()).collect())
            .unwrap_or_default();
        let Some(loudest) = decibels.into_iter().reduce(f64::max) else { return };
        let mut rms = 10f64.powf(loudest / 20.0).clamp(0.0, 1.0);
        if rms < 1e-6 {
            rms = 0.0;
        }
        self.samples.set(self.samples.get() + 1);
        self.peak.set(self.peak.get().max(rms));
        let now = now_ms();
        if self.first_buffer_at.get().is_none() {
            self.first_buffer_at.set(Some(now));
            self.emit("firstBuffer", json!({"firstBufferAtEpochMs": now, "rootMeanSquare": rms}));
        } else if rms > 0.0 && self.first_signal_at.get().is_none() {
            self.first_signal_at.set(Some(now));
            self.emit("firstNonzeroBuffer", json!({"firstNonzeroBufferAtEpochMs": now, "rootMeanSquare": rms}));
        } else {
            self.emit("level", json!({"rootMeanSquare": rms}));
        }
    }

    fn finish(&self) {
        self.teardown(true);
        let data = std::fs::read(&self.path);
        let _ = std::fs::remove_file(&self.path);
        let data = match data {
            Ok(d) => d,
            Err(error) => {
                self.emit("error", json!({"message": error.to_string()}));
                return;
            }
        };
        let delivered = now_ms();
        let mut fields = Map::new();
        fields.insert("base64".into(), base64::engine::general_purpose::STANDARD.encode(data).into());
        fields.insert("mimeType".into(), MIME_TYPE.into());
        fields.insert("stopRequestedAtEpochMs".into(), self.stop_requested_at.get().unwrap_or(delivered).into());
        fields.insert("finalAudioDeliveredAtEpochMs".into(), delivered.into());
        fields.insert("firstBufferAtEpochMs".into(), self.first_buffer_at.get().unwrap_or(delivered).into());
        fields.insert("firstNonzeroBufferAtEpochMs".into(), self.first_signal_at.get().into());
        fields.insert("levelSampleCount".into(), self.samples.get().into());
        fields.insert("peakRootMeanSquare".into(), self.peak.get().into());
        fields.insert("droppedStartupBufferCount".into(), 0.into());
        fields.insert("droppedFormatChangeBufferCount".into(), 0.into());
        fields.insert("stopReason".into(), Value::Null);
        self.emit("finished", Value::Object(fields));
    }

    fn fail(&self, message: &str) {
        self.teardown(false);
        self.emit("error", json!({"message": message}));
    }

    fn teardown(&self, keep_file: bool) {
        self.done.set(true);
        let _ = self.pipeline.set_state(gst::State::Null);
        if !keep_file {
            let _ = std::fs::remove_file(&self.path);
        }
        if let Some(owner) = self.owner.upgrade() {
            owner.ended(self);
        }
    }
}

pub fn open_microphone_settings() -> bool {
    for command in SETTINGS_APPS {
        if glib::find_program_in_path(command[0]).is_some() {
            return spawn_detached(command);
        }
    }
    false
}

/// Starts a program in its own session, detached from Amp's input and output.
fn spawn_detached(command: &[&str]) -> bool {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(command[0])
        .args(&command[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .is_ok()
}

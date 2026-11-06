//! One room connection. The LiveKit SDK runs on its own tokio runtime and reports to the window as
//! [`Event`]s; the window controls it with [`Command`]s.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use gst::prelude::*;
use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::prelude::{
    I420Buffer, RtcVideoSource, VideoFormatType, VideoFrame, VideoResolution, VideoRotation,
};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_stream::native::NativeVideoStream;
use tokio::sync::mpsc;

const VIDEO_WIDTH: u32 = 640;
const VIDEO_HEIGHT: u32 = 360;
const VIDEO_FPS: i32 = 24;
/// Remote frames shown per second, at most.
const FRAME_INTERVAL: Duration = Duration::from_millis(50);

/// The key of the local camera's frames.
pub const LOCAL: &str = "local";

pub enum Event {
    Connected {
        identity: String,
        name: String,
    },
    Failed(String),
    ParticipantChanged {
        identity: String,
        name: String,
        muted: bool,
    },
    ParticipantLeft(String),
    MediaChanged {
        mic_available: bool,
        mic_on: bool,
        camera_on: bool,
        problem: Option<String>,
    },
    SpeakersChanged(Vec<String>),
    MetadataChanged(String),
    StatusChanged(Option<String>),
    /// The room went away without the user leaving, with the reason to show.
    CallEnded(Option<String>),
    VideoStarted {
        key: String,
        label: String,
    },
    /// RGBA pixels of `key`: [`LOCAL`], or `identity/source`.
    VideoFrame {
        key: String,
        width: u32,
        height: u32,
        data: Vec<u8>,
    },
    VideoEnded(String),
    /// The call disconnected after a [`Command::Leave`].
    Left,
}

pub enum Command {
    SetMicrophone(bool),
    SetCamera(bool),
    Leave,
}

pub fn source_name(source: TrackSource) -> &'static str {
    match source {
        TrackSource::Camera => "camera",
        TrackSource::Microphone => "microphone",
        TrackSource::Screenshare => "screenshare",
        TrackSource::ScreenshareAudio => "screenshare-audio",
        _ => "unknown",
    }
}

fn describe(participant: &RemoteParticipant) -> Event {
    let microphone = participant.track_publications().into_values().find(|p| p.source() == TrackSource::Microphone);
    let identity = participant.identity().to_string();
    let name = participant.name();
    Event::ParticipantChanged {
        name: if name.is_empty() { identity.clone() } else { name },
        identity,
        muted: microphone.is_none_or(|m| m.is_muted()),
    }
}

/// What the window says when the call ended; None when this app left.
fn reason(name: &str) -> Option<String> {
    match name {
        "DUPLICATE_IDENTITY" => Some("You joined from another device.".into()),
        "PARTICIPANT_REMOVED" => Some("You were removed from the call.".into()),
        "ROOM_DELETED" => Some("The call ended.".into()),
        "CLIENT_INITIATED" => None,
        _ => Some("Lost connection to the call.".into()),
    }
}

/// A source element for the first camera, or None when there is none.
fn camera_element() -> Option<gst::Element> {
    gst::init().ok()?;
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Video/Source"), None);
    monitor.start().ok()?;
    let device = monitor.devices().into_iter().next();
    monitor.stop();
    device?.create_element(None).ok()
}

/// Captures RGBA frames with GStreamer into the camera track's source.
struct Camera {
    pipeline: gst::Pipeline,
}

impl Camera {
    fn new(source: NativeVideoSource, events: async_channel::Sender<Event>) -> Result<Self, String> {
        let camera = camera_element().ok_or("No camera found")?;
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "RGBA")
            .field("width", VIDEO_WIDTH as i32)
            .field("height", VIDEO_HEIGHT as i32)
            .field("framerate", gst::Fraction::new(VIDEO_FPS, 1))
            .build();
        let start = Instant::now();
        let sink = gst_app::AppSink::builder()
            .max_buffers(1)
            .drop(true)
            .callbacks(
                gst_app::AppSinkCallbacks::builder()
                    .new_sample(move |sink| {
                        let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                        let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                        let (w, h) = (VIDEO_WIDTH, VIDEO_HEIGHT);
                        if map.len() < (w * h * 4) as usize {
                            return Ok(gst::FlowSuccess::Ok);
                        }
                        let mut i420 = I420Buffer::new(w, h);
                        let (sy, su, sv) = i420.strides();
                        let (y, u, v) = i420.data_mut();
                        // libyuv's ABGR is R, G, B, A in memory.
                        yuv_helper::abgr_to_i420(map.as_slice(), w * 4, y, sy, u, su, v, sv, w as i32, h as i32);
                        let mut frame = VideoFrame::new(VideoRotation::VideoRotation0, i420);
                        frame.timestamp_us = start.elapsed().as_micros() as i64;
                        source.capture_frame(&frame);
                        let _ = events.try_send(Event::VideoFrame {
                            key: LOCAL.into(),
                            width: w,
                            height: h,
                            data: map.as_slice().to_vec(),
                        });
                        Ok(gst::FlowSuccess::Ok)
                    })
                    .build(),
            )
            .build();
        let pipeline = gst::Pipeline::new();
        let make = |name: &str| gst::ElementFactory::make(name).build().map_err(|e| e.to_string());
        let capsfilter =
            gst::ElementFactory::make("capsfilter").property("caps", &caps).build().map_err(|e| e.to_string())?;
        let elements =
            [camera, make("videoconvert")?, make("videoscale")?, make("videorate")?, capsfilter, sink.upcast()];
        pipeline.add_many(&elements).map_err(|e| e.to_string())?;
        gst::Element::link_many(&elements).map_err(|e| e.to_string())?;
        pipeline.set_state(gst::State::Playing).map_err(|e| e.to_string())?;
        Ok(Self { pipeline })
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

struct Media {
    _audio: Option<PlatformAudio>,
    mic: Option<LocalAudioTrack>,
    camera: Option<Camera>,
    camera_track: Option<LocalVideoTrack>,
    video_source: Option<NativeVideoSource>,
}

pub async fn run(
    server_url: String,
    token: String,
    events: async_channel::Sender<Event>,
    mut commands: mpsc::UnboundedReceiver<Command>,
) {
    let send = |event: Event| {
        let _ = events.try_send(event);
    };
    let mut options = RoomOptions::default();
    options.auto_subscribe = true;
    options.dynacast = true;
    let (room, mut room_events) = match Room::connect(&server_url, &token, options).await {
        Ok(connected) => connected,
        Err(error) => {
            log::warn!("huddle join failed: {error}");
            send(Event::Failed(format!("Couldn’t join: {error}")));
            // Wait for the window to leave, so it sees Left as usual.
            while let Some(command) = commands.recv().await {
                if let Command::Leave = command {
                    break;
                }
            }
            send(Event::Left);
            return;
        }
    };
    let local = room.local_participant();
    send(Event::Connected { identity: local.identity().to_string(), name: local.name() });
    let mut streams: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    for participant in room.remote_participants().values() {
        send(describe(participant));
        for publication in participant.track_publications().values() {
            if let Some(RemoteTrack::Video(track)) = publication.track() {
                watch_video(&mut streams, &events, participant, track, publication.source());
            }
        }
    }
    send(Event::MetadataChanged(room.metadata()));

    // Media problems leave the call up, as the page does ("Some media features are unavailable").
    let mut problems = Vec::new();
    let mut media = Media { _audio: None, mic: None, camera: None, camera_track: None, video_source: None };
    match PlatformAudio::new() {
        Ok(audio) => {
            let mic = LocalAudioTrack::create_audio_track("microphone", audio.rtc_source());
            let options = TrackPublishOptions { source: TrackSource::Microphone, ..Default::default() };
            match local.publish_track(LocalTrack::Audio(mic.clone()), options).await {
                Ok(_) => {
                    media.mic = Some(mic);
                    media._audio = Some(audio);
                }
                Err(error) => {
                    log::warn!("huddle microphone failed: {error}");
                    problems.push("microphone");
                }
            }
        }
        Err(error) => {
            log::warn!("huddle microphone failed: {error}");
            problems.push("microphone");
        }
    }
    if let Err(error) = start_camera(&mut media, &local, &events).await {
        log::info!("huddle camera unavailable: {error}");
        if error != "No camera found" {
            problems.push("camera");
        }
    }
    let problem = (!problems.is_empty()).then(|| format!("Your {} isn’t available.", problems.join(" and ")));
    send(media_changed(&media, problem));

    let mut room_open = true;
    loop {
        tokio::select! {
            event = room_events.recv(), if room_open => match event {
                Some(event) => handle_room_event(event, &mut streams, &events),
                None => room_open = false,
            },
            command = commands.recv() => match command {
                Some(Command::SetMicrophone(enabled)) => {
                    if let Some(mic) = &media.mic {
                        if enabled { mic.unmute() } else { mic.mute() }
                    }
                    send(media_changed(&media, None));
                }
                Some(Command::SetCamera(enabled)) => {
                    let mut problem = None;
                    if enabled && media.camera.is_none() {
                        if let Err(error) = start_camera(&mut media, &local, &events).await {
                            log::warn!("huddle camera failed: {error}");
                            problem = Some(if error == "No camera found" { "No camera found.".to_string() } else { format!("Camera failed: {error}") });
                        }
                    } else if !enabled && media.camera.is_some() {
                        media.camera = None;
                        if let Some(track) = &media.camera_track {
                            track.mute();
                        }
                        send(Event::VideoEnded(LOCAL.into()));
                    }
                    send(media_changed(&media, problem));
                }
                Some(Command::Leave) | None => break,
            },
        }
    }
    media.camera = None;
    for (_, task) in streams.drain() {
        task.abort();
    }
    if let Err(error) = room.close().await {
        log::debug!("huddle disconnect: {error}");
    }
    drop(media);
    send(Event::Left);
}

fn media_changed(media: &Media, problem: Option<String>) -> Event {
    Event::MediaChanged {
        mic_available: media.mic.is_some(),
        mic_on: media.mic.as_ref().is_some_and(|m| !m.is_muted()),
        camera_on: media.camera.is_some(),
        problem,
    }
}

async fn start_camera(
    media: &mut Media,
    local: &LocalParticipant,
    events: &async_channel::Sender<Event>,
) -> Result<(), String> {
    let source = media
        .video_source
        .get_or_insert_with(|| {
            NativeVideoSource::new(VideoResolution { width: VIDEO_WIDTH, height: VIDEO_HEIGHT }, false)
        })
        .clone();
    media.camera = Some(Camera::new(source.clone(), events.clone())?);
    match &media.camera_track {
        Some(track) => track.unmute(),
        None => {
            let track = LocalVideoTrack::create_video_track("camera", RtcVideoSource::Native(source));
            let options = TrackPublishOptions { source: TrackSource::Camera, ..Default::default() };
            if let Err(error) = local.publish_track(LocalTrack::Video(track.clone()), options).await {
                media.camera = None;
                return Err(error.to_string());
            }
            media.camera_track = Some(track);
        }
    }
    Ok(())
}

fn handle_room_event(
    event: RoomEvent,
    streams: &mut HashMap<String, tokio::task::JoinHandle<()>>,
    events: &async_channel::Sender<Event>,
) {
    let send = |event: Event| {
        let _ = events.try_send(event);
    };
    match event {
        RoomEvent::ParticipantConnected(p) => send(describe(&p)),
        RoomEvent::ParticipantDisconnected(p) => send(Event::ParticipantLeft(p.identity().to_string())),
        RoomEvent::TrackPublished { participant, .. } | RoomEvent::TrackUnpublished { participant, .. } => {
            send(describe(&participant))
        }
        RoomEvent::TrackMuted { participant: Participant::Remote(p), .. }
        | RoomEvent::TrackUnmuted { participant: Participant::Remote(p), .. } => send(describe(&p)),
        RoomEvent::TrackSubscribed { track, publication, participant } => {
            send(describe(&participant));
            if let RemoteTrack::Video(track) = track {
                watch_video(streams, events, &participant, track, publication.source());
            }
        }
        RoomEvent::TrackUnsubscribed { publication, participant, .. } => {
            stop_video(streams, events, &format!("{}/{}", participant.identity(), source_name(publication.source())));
        }
        RoomEvent::ActiveSpeakersChanged { speakers } => {
            send(Event::SpeakersChanged(speakers.iter().map(|s| s.identity().to_string()).collect()))
        }
        RoomEvent::RoomMetadataChanged { metadata, .. } => send(Event::MetadataChanged(metadata)),
        RoomEvent::Reconnecting => send(Event::StatusChanged(Some("Reconnecting…".into()))),
        RoomEvent::Reconnected => send(Event::StatusChanged(None)),
        RoomEvent::Disconnected { reason: r } => send(Event::CallEnded(reason(r.as_str_name()))),
        _ => {}
    }
}

fn watch_video(
    streams: &mut HashMap<String, tokio::task::JoinHandle<()>>,
    events: &async_channel::Sender<Event>,
    participant: &RemoteParticipant,
    track: RemoteVideoTrack,
    source: TrackSource,
) {
    let key = format!("{}/{}", participant.identity(), source_name(source));
    let name = participant.name();
    let mut label = if name.is_empty() { participant.identity().to_string() } else { name };
    if source == TrackSource::Screenshare {
        label += "’s screen";
    }
    stop_video(streams, events, &key);
    let events = events.clone();
    let task_key = key.clone();
    let task = tokio::spawn(async move {
        let mut stream = NativeVideoStream::new(track.rtc_track());
        let _ = events.try_send(Event::VideoStarted { key: task_key.clone(), label });
        let mut shown: Option<Instant> = None;
        while let Some(frame) = stream.next().await {
            if shown.is_some_and(|t| t.elapsed() < FRAME_INTERVAL) {
                continue;
            }
            shown = Some(Instant::now());
            let (width, height) = (frame.buffer.width(), frame.buffer.height());
            let mut data = vec![0u8; (width * height * 4) as usize];
            // libyuv's ABGR is R, G, B, A in memory, which is what the window shows.
            frame.buffer.to_argb(VideoFormatType::ABGR, &mut data, width * 4, width as i32, height as i32);
            let _ = events.try_send(Event::VideoFrame { key: task_key.clone(), width, height, data });
        }
        let _ = events.try_send(Event::VideoEnded(task_key));
    });
    streams.insert(key, task);
}

fn stop_video(
    streams: &mut HashMap<String, tokio::task::JoinHandle<()>>,
    events: &async_channel::Sender<Event>,
    key: &str,
) {
    if let Some(task) = streams.remove(key) {
        task.abort();
        let _ = events.try_send(Event::VideoEnded(key.to_string()));
    }
}

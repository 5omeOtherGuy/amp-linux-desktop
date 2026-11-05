"""Huddles through the LiveKit SDK, for WebKitGTK builds without WebRTC.

Distribution WebKitGTK is built with ENABLE_WEB_RTC off, so the page's livekit-client cannot
connect. The page still asks Amp for the call's token (POST /api/threads/<id>/space-token with
an empty body); scripts.HUDDLE hands that token to ampNativeHuddle and this module joins the room
natively: the microphone and speakers go through libwebrtc's audio device module (echo
cancellation included), the camera through GStreamer, and remote video into a GTK window.

The room and identity are the same ones the page would use, so others see an ordinary
participant. Not covered: screen sharing from Linux and the page's per-phrase transcription
upload; the window says when a call is being transcribed.

The SDK is the `livekit` Python package. scripts/install puts it in DATA_DIR/python; without it
the handler is not registered and the page keeps its own (failing) path.
"""

import asyncio
import json
import logging
import sys
import threading
import time

from gi.repository import Adw, Gdk, GLib, Gtk

from . import config

log = logging.getLogger(__name__)

rtc = None
Gst = None
VIDEO_WIDTH, VIDEO_HEIGHT, VIDEO_FPS = 640, 360, 24
FRAME_INTERVAL = 1 / 20  # remote frames shown per second, at most


def available():
    """Whether the LiveKit SDK can be imported."""
    global rtc
    if rtc is None:
        path = str(config.DATA_DIR / "python")
        if path not in sys.path:
            sys.path.append(path)
        try:
            from livekit import rtc as module
        except Exception as error:  # missing, or built for another Python
            log.info("huddles unavailable: %s", error)
            return False
        rtc = module
    return True


def _gst():
    global Gst
    if Gst is None:
        import gi

        gi.require_version("Gst", "1.0")
        from gi.repository import Gst as module

        module.init(None)
        Gst = module
    return Gst


def camera_element():
    """A source element for the first camera, or None when there is none."""
    gst = _gst()
    monitor = gst.DeviceMonitor()
    monitor.add_filter("Video/Source", None)
    monitor.start()
    try:
        devices = monitor.get_devices() or []
    finally:
        monitor.stop()
    return devices[0].create_element(None) if devices else None


class Camera:
    """Captures RGBA frames with GStreamer and calls on_frame(width, height, bytes) per frame."""

    def __init__(self, on_frame):
        gst = _gst()
        source = camera_element()
        if source is None:
            raise RuntimeError("No camera found")
        self.pipeline = gst.Pipeline()
        caps = gst.Caps.from_string(
            f"video/x-raw,format=RGBA,width={VIDEO_WIDTH},height={VIDEO_HEIGHT},framerate={VIDEO_FPS}/1")
        elements = [source] + [gst.ElementFactory.make(n) for n in ("videoconvert", "videoscale", "videorate")]
        capsfilter = gst.ElementFactory.make("capsfilter")
        capsfilter.set_property("caps", caps)
        sink = gst.ElementFactory.make("appsink")
        sink.set_property("emit-signals", True)
        sink.set_property("max-buffers", 1)
        sink.set_property("drop", True)
        sink.connect("new-sample", self._sample)
        elements += [capsfilter, sink]
        for element in elements:
            self.pipeline.add(element)
        for a, b in zip(elements, elements[1:]):
            a.link(b)
        self.on_frame = on_frame

    def _sample(self, sink):
        sample = sink.emit("pull-sample")
        buffer = sample.get_buffer()
        ok, info = buffer.map(Gst.MapFlags.READ)
        if ok:
            try:
                self.on_frame(VIDEO_WIDTH, VIDEO_HEIGHT, bytes(info.data))
            finally:
                buffer.unmap(info)
        return Gst.FlowReturn.OK

    def start(self):
        self.pipeline.set_state(Gst.State.PLAYING)

    def stop(self):
        self.pipeline.set_state(Gst.State.NULL)


class Call:
    """One room connection. Runs the SDK on its own asyncio loop; reports to `ui` on the GTK
    main loop through GLib.idle_add."""

    def __init__(self, ui, server_url, token):
        self.ui = ui
        self.server_url, self.token = server_url, token
        self.loop = asyncio.new_event_loop()
        self.room = None
        self.audio = None
        self.mic = None
        self.mic_source = None
        self.camera = None
        self.camera_track = None
        self.video_source = None
        self._streams = {}
        threading.Thread(target=self.loop.run_forever, name="huddle", daemon=True).start()

    def submit(self, coroutine):
        return asyncio.run_coroutine_threadsafe(coroutine, self.loop)

    def _ui(self, method, *args):
        GLib.idle_add(lambda: getattr(self.ui, method)(*args) and False)

    async def join(self, camera):
        try:
            self.room = rtc.Room(loop=self.loop)
            self._listen(self.room)
            await self.room.connect(self.server_url, self.token, rtc.RoomOptions(auto_subscribe=True, dynacast=True))
            self._ui("connected", self.room.local_participant.identity, self.room.local_participant.name)
            for participant in self.room.remote_participants.values():
                self._ui("participant_changed", self._describe(participant))
                for publication in participant.track_publications.values():
                    if publication.track and publication.kind == rtc.TrackKind.KIND_VIDEO:
                        self._watch_video(participant, publication.track, publication.source)
            self._ui("metadata_changed", self.room.metadata)
        except Exception as error:
            log.warning("huddle join failed: %s", error)
            self._ui("failed", f"Couldn’t join: {error}")
            return
        # Media problems leave the call up, as the page does ("Some media features are unavailable").
        problems = []
        try:
            self.audio = rtc.PlatformAudio()
            self.mic_source = self.audio.create_audio_source()
            self.mic = rtc.LocalAudioTrack.create_audio_track("microphone", self.mic_source)
            await self.room.local_participant.publish_track(
                self.mic, rtc.TrackPublishOptions(source=rtc.TrackSource.SOURCE_MICROPHONE))
        except Exception as error:
            log.warning("huddle microphone failed: %s", error)
            problems.append("microphone")
        if camera:
            try:
                await self._start_camera()
            except Exception as error:
                log.info("huddle camera unavailable: %s", error)
                if not isinstance(error, RuntimeError):
                    problems.append("camera")
        self._ui("media_changed", self.mic is not None and not self.mic.muted, self.camera is not None,
                 "Your " + " and ".join(problems) + " isn’t available." if problems else None)

    async def _start_camera(self):
        if self.video_source is None:
            self.video_source = rtc.VideoSource(VIDEO_WIDTH, VIDEO_HEIGHT)
        self.camera = Camera(self._camera_frame)
        self.camera.start()
        if self.camera_track is None:
            self.camera_track = rtc.LocalVideoTrack.create_video_track("camera", self.video_source)
            await self.room.local_participant.publish_track(
                self.camera_track, rtc.TrackPublishOptions(source=rtc.TrackSource.SOURCE_CAMERA))
        else:
            self.camera_track.unmute()

    def _camera_frame(self, width, height, data):
        frame = rtc.VideoFrame(width, height, rtc.VideoBufferType.RGBA, data)
        self.video_source.capture_frame(frame, timestamp_us=int(time.monotonic() * 1e6))
        self._ui("video_frame", "local", width, height, width * 4, data)

    async def set_microphone(self, enabled):
        if self.mic:
            self.mic.unmute() if enabled else self.mic.mute()
        self._ui("media_changed", self.mic is not None and not self.mic.muted, self.camera is not None, None)

    async def set_camera(self, enabled):
        error = None
        if enabled and self.camera is None:
            try:
                await self._start_camera()
            except Exception as e:
                error = "No camera found." if isinstance(e, RuntimeError) else f"Camera failed: {e}"
        elif not enabled and self.camera is not None:
            self.camera.stop()
            self.camera = None
            self.camera_track.mute()
            self._ui("video_ended", "local")
        self._ui("media_changed", self.mic is not None and not self.mic.muted, self.camera is not None, error)

    async def leave(self):
        if self.camera:
            self.camera.stop()
            self.camera = None
        for task in self._streams.values():
            task.cancel()
        if self.room:
            try:
                await self.room.disconnect()
            except Exception as error:
                log.debug("huddle disconnect: %s", error)
        for resource in (self.mic_source, self.audio):
            if resource:
                resource.close()
        self.loop.call_soon(self.loop.stop)

    # ---- room events -------------------------------------------------------------------------

    def _describe(self, participant):
        microphone = next((p for p in participant.track_publications.values()
                           if p.source == rtc.TrackSource.SOURCE_MICROPHONE), None)
        return {"identity": participant.identity, "name": participant.name or participant.identity,
                "muted": microphone is None or microphone.muted}

    def _listen(self, room):
        room.on("participant_connected", lambda p: self._ui("participant_changed", self._describe(p)))
        room.on("participant_disconnected", lambda p: self._ui("participant_left", p.identity))
        def changed(participant):
            if participant is not room.local_participant:
                self._ui("participant_changed", self._describe(participant))

        # The SDK passes (publication, participant) for these and (participant, publication) for mutes.
        for event in ("track_published", "track_unpublished"):
            room.on(event, lambda _publication, participant: changed(participant))
        for event in ("track_muted", "track_unmuted"):
            room.on(event, lambda participant, _publication: changed(participant))
        room.on("track_subscribed", self._subscribed)
        room.on("track_unsubscribed", lambda track, pub, p: self._stop_video(f"{p.identity}/{pub.source}"))
        room.on("active_speakers_changed", lambda speakers: self._ui(
            "speakers_changed", [s.identity for s in speakers]))
        room.on("room_metadata_changed", lambda _old, new: self._ui("metadata_changed", new))
        room.on("reconnecting", lambda: self._ui("status_changed", "Reconnecting…"))
        room.on("reconnected", lambda: self._ui("status_changed", None))
        room.on("disconnected", lambda reason: self._ui("call_ended", _reason(reason)))

    def _subscribed(self, track, publication, participant):
        self._ui("participant_changed", self._describe(participant))
        if track.kind == rtc.TrackKind.KIND_VIDEO:
            self._watch_video(participant, track, publication.source)

    def _watch_video(self, participant, track, source):
        key = f"{participant.identity}/{source}"
        label = participant.name or participant.identity
        if source == rtc.TrackSource.SOURCE_SCREENSHARE:
            label += "’s screen"
        self._stop_video(key)
        self._streams[key] = self.loop.create_task(self._render(key, label, track))

    async def _render(self, key, label, track):
        stream = rtc.VideoStream(track, loop=self.loop, format=rtc.VideoBufferType.RGBA)
        self._ui("video_started", key, label)
        shown = 0.0
        try:
            async for event in stream:
                now = time.monotonic()
                if now - shown < FRAME_INTERVAL:
                    continue
                shown = now
                frame = event.frame
                self._ui("video_frame", key, frame.width, frame.height, frame.width * 4, bytes(frame.data))
        finally:
            await stream.aclose()
            self._ui("video_ended", key)

    def _stop_video(self, key):
        task = self._streams.pop(key, None)
        if task:
            task.cancel()


def _reason(reason):
    name = rtc.DisconnectReason.Name(reason) if isinstance(reason, int) else str(reason)
    return {
        "DUPLICATE_IDENTITY": "You joined from another device.",
        "PARTICIPANT_REMOVED": "You were removed from the call.",
        "ROOM_DELETED": "The call ended.",
        "CLIENT_INITIATED": None,
    }.get(name, "Lost connection to the call.")


class Tile(Gtk.Overlay):
    def __init__(self, label, name=None):
        super().__init__()
        self.add_css_class("huddle-tile")
        self.set_size_request(320, 180)
        self.avatar = Adw.Avatar(size=72, text=name or label, show_initials=True, halign=Gtk.Align.CENTER,
                                 valign=Gtk.Align.CENTER)
        self.picture = Gtk.Picture(content_fit=Gtk.ContentFit.COVER, can_shrink=True, visible=False)
        self.set_child(Gtk.Box(hexpand=True, vexpand=True))
        self.add_overlay(self.picture)
        self.add_overlay(self.avatar)
        caption = Gtk.Box(spacing=4, halign=Gtk.Align.START, valign=Gtk.Align.END, margin_start=8, margin_bottom=8)
        caption.add_css_class("huddle-caption")
        self.muted = Gtk.Image(icon_name="microphone-disabled-symbolic", visible=False)
        caption.append(self.muted)
        caption.append(Gtk.Label(label=label, ellipsize=3))
        self.add_overlay(caption)
        self.ring = Gtk.Box(visible=False, can_target=False)
        self.ring.add_css_class("huddle-ring")
        self.add_overlay(self.ring)

    def show_frame(self, width, height, stride, data):
        texture = Gdk.MemoryTexture.new(width, height, Gdk.MemoryFormat.R8G8B8A8, GLib.Bytes.new(data), stride)
        self.picture.set_paintable(texture)
        self.picture.set_visible(True)
        self.avatar.set_visible(False)

    def clear_video(self):
        self.picture.set_paintable(None)
        self.picture.set_visible(False)
        self.avatar.set_visible(True)

    def set_speaking(self, speaking):
        self.ring.set_visible(speaking)


class HuddleWindow(Adw.Window):
    """The call window: a tile per participant (and per shared screen), and mute, camera and leave
    buttons."""

    def __init__(self, app, thread_id, title, call_name, server_url, token):
        super().__init__(application=app, default_width=760, default_height=520, title=call_name)
        self.call_name = call_name
        self.app, self.thread_id = app, thread_id
        self.local_identity = None
        self.tiles = {}
        self._left = False
        self._end_message = None
        self._leaving = None

        header = Adw.HeaderBar()
        self.heading = Adw.WindowTitle(title=call_name, subtitle=title or "")
        header.set_title_widget(self.heading)
        self.banner = Gtk.Label(wrap=True, visible=False, margin_top=6, margin_bottom=6)
        self.banner.add_css_class("huddle-banner")
        self.grid = Gtk.FlowBox(selection_mode=Gtk.SelectionMode.NONE, homogeneous=True, max_children_per_line=3,
                                min_children_per_line=1, row_spacing=8, column_spacing=8, valign=Gtk.Align.CENTER,
                                margin_start=12, margin_end=12, margin_top=12, margin_bottom=12)
        scroller = Gtk.ScrolledWindow(vexpand=True, child=self.grid, hscrollbar_policy=Gtk.PolicyType.NEVER)

        self.mic_button = Gtk.ToggleButton(icon_name="audio-input-microphone-symbolic", tooltip_text="Mute",
                                           sensitive=False, active=True)
        self.camera_button = Gtk.ToggleButton(icon_name="camera-web-symbolic", tooltip_text="Turn Camera On")
        leave = Gtk.Button(label="Leave")
        leave.add_css_class("destructive-action")
        leave.add_css_class("pill")
        for button in (self.mic_button, self.camera_button):
            button.add_css_class("circular")
            button.add_css_class("huddle-control")
        self._mic_handler = self.mic_button.connect("toggled", lambda b: self.call.submit(self.call.set_microphone(b.get_active())))
        self._camera_handler = self.camera_button.connect("toggled", lambda b: self.call.submit(self.call.set_camera(b.get_active())))
        leave.connect("clicked", lambda *_: self.close())
        controls = Gtk.Box(spacing=12, halign=Gtk.Align.CENTER, margin_top=6, margin_bottom=12)
        for widget in (self.mic_button, self.camera_button, leave):
            controls.append(widget)
        self.status = Gtk.Label(label="Joining…", margin_bottom=4)
        self.status.add_css_class("dim-label")

        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        for widget in (header, self.banner, scroller, self.status, controls):
            box.append(widget)
        self.toasts = Adw.ToastOverlay(child=box)
        self.set_content(self.toasts)
        self.connect("close-request", self._close_request)

        self.call = Call(self, server_url, token)
        self.call.submit(self.call.join(camera=True))

    # ---- from the call -----------------------------------------------------------------------

    def connected(self, identity, name):
        self.local_identity = identity
        self._tile(identity, f"{name or identity} (You)", name or identity)
        self.status.set_visible(False)
        self.app.huddle_reconcile(self.thread_id)

    def failed(self, message):
        self.call_ended(message)

    def call_ended(self, message):
        """The room went away without the user leaving: show why in the main window."""
        if not self._left:
            self._end_message = message
            self.close()

    def participant_changed(self, info):
        tile = self._tile(info["identity"], info["name"])
        tile.muted.set_visible(info["muted"])

    def participant_left(self, identity):
        for key in [k for k in self.tiles if k == identity or k.startswith(identity + "/")]:
            self._remove(key)

    def media_changed(self, mic_on, camera_on, problem):
        for button, handler, active in ((self.mic_button, self._mic_handler, mic_on),
                                        (self.camera_button, self._camera_handler, camera_on)):
            button.handler_block(handler)
            button.set_active(active)
            button.handler_unblock(handler)
        self.mic_button.set_sensitive(self.call.mic is not None)
        self.mic_button.set_icon_name("audio-input-microphone-symbolic" if mic_on else "microphone-disabled-symbolic")
        self.mic_button.set_tooltip_text("Mute" if mic_on else "Unmute")
        self.camera_button.set_icon_name("camera-web-symbolic" if camera_on else "camera-disabled-symbolic")
        self.camera_button.set_tooltip_text("Turn Camera Off" if camera_on else "Turn Camera On")
        if self.local_identity in self.tiles:
            self.tiles[self.local_identity].muted.set_visible(not mic_on)
        if problem:
            self.toasts.add_toast(Adw.Toast(title=problem, timeout=4))

    def speakers_changed(self, identities):
        for key, tile in self.tiles.items():
            tile.set_speaking(key in identities)

    def metadata_changed(self, metadata):
        try:
            transcript = (json.loads(metadata or "{}") or {}).get("transcript") or {}
        except ValueError:
            transcript = {}
        self.banner.set_label(f"This {self.call_name} is being transcribed. The transcript will be in Artifacts when the "
                              "call ends; speech from this Linux window isn’t included yet.")
        self.banner.set_visible(bool(transcript.get("active")))

    def status_changed(self, text):
        self.status.set_label(text or "")
        self.status.set_visible(bool(text))

    def video_started(self, key, label):
        if not key.endswith(f"/{rtc.TrackSource.SOURCE_CAMERA}"):
            self._tile(key, label)

    def video_frame(self, key, width, height, stride, data):
        key = self.local_identity if key == "local" else key
        identity, _, source = key.partition("/")
        if source == str(rtc.TrackSource.SOURCE_CAMERA):
            key = identity
        tile = self.tiles.get(key)
        if tile:
            tile.show_frame(width, height, stride, data)

    def video_ended(self, key):
        key = self.local_identity if key == "local" else key
        identity, _, source = key.partition("/")
        if source == str(rtc.TrackSource.SOURCE_CAMERA) or not source:
            tile = self.tiles.get(identity)
            if tile:
                tile.clear_video()
        else:
            self._remove(key)

    # ---- tiles -------------------------------------------------------------------------------

    def _tile(self, key, label, name=None):
        tile = self.tiles.get(key)
        if tile is None:
            tile = self.tiles[key] = Tile(label, name)
            self.grid.append(tile)
        return tile

    def _remove(self, key):
        tile = self.tiles.pop(key, None)
        if tile:
            self.grid.remove(tile.get_parent())

    def _close_request(self, *_):
        if not self._left:
            self._left = True
            left = self._leaving = self.call.submit(self.call.leave())
            # The server updates the thread's live state once the participant is gone.
            left.add_done_callback(lambda _f: GLib.idle_add(lambda: self.app.huddle_reconcile(self.thread_id) and False))
            self.app.huddle_ended(self, self._end_message)
        return False

    def wait_left(self, timeout):
        """Blocks until the call has disconnected, so quitting Amp does not leave a ghost
        participant behind until the server times it out."""
        if self._leaving:
            try:
                self._leaving.result(timeout)
            except Exception as error:
                log.debug("huddle leave on quit: %s", error)

"""Native microphone capture for the page's dictation (ampNativeDictation, version 1).

WebKitGTK gives the page getUserMedia but its MediaRecorder needs GStreamer 1.24.9 or newer for
WebM and gst-plugins-rs for fragmented MP4 (MediaRecorderPrivateGStreamer.cpp), so on older
stacks the page's browser dictation records nothing. This records with GStreamer directly.

The page sends { command, requestID, ... } and listens for amp:native-dictation events
{ version: 1, type, requestID, ... }, as the Mac app's AVCapture recorder answers them:
- listSources: sources { sources: [{ deviceID, label }] }; sourcesChanged when devices change.
- start { deviceID, requestStartedAtEpochMs }: started, firstBuffer, firstNonzeroBuffer, level,
  and on stop finished { base64, mimeType, ... }; canceled after cancel; error on failure.
- openMicrophoneSettings: the desktop's sound input settings.
The page plays its own start and stop cues (nativeCues false) and uploads the audio itself
(uploadStreaming false).
"""

import base64
import json
import logging
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

import gi

from gi.repository import GLib

try:
    gi.require_version("Gst", "1.0")
    from gi.repository import Gst
except (ValueError, ImportError):  # no GStreamer introspection data: the page keeps its own path
    Gst = None

log = logging.getLogger(__name__)

MIME_TYPE = "audio/webm;codecs=opus"
LEVEL_INTERVAL_NS = 50_000_000
SETTINGS_APPS = (["pavucontrol", "--tab=4"], ["gnome-control-center", "sound"], ["systemsettings", "kcm_pulseaudio"])

_initialized = False


def _init():
    global _initialized
    if not _initialized:
        Gst.init(None)
        _initialized = True


def _now_ms():
    return int(time.time() * 1000)


def available():
    """Whether this computer can record: GStreamer with an Opus encoder and a WebM muxer."""
    if Gst is None:
        return False
    try:
        _init()
    except Exception:
        return False
    return all(Gst.ElementFactory.find(name) for name in ("opusenc", "webmmux", "level"))


class Dictation:
    def __init__(self, run_js):
        _init()
        self._run_js = run_js
        self._monitor = None
        self._known = None
        self._session = None

    # ---- page messages -----------------------------------------------------------------------

    def handle(self, body):
        if not isinstance(body, dict) or not isinstance(body.get("requestID"), str):
            return
        command, request = body.get("command"), body["requestID"]
        if command == "listSources":
            self._emit("sources", request, sources=self._sources())
        elif command == "start":
            self._start(request, body)
        elif command == "stop":
            if self._session and self._session.request == request:
                self._session.stop()
        elif command == "cancel":
            if self._session and self._session.request == request:
                self._session.cancel()
        elif command == "openMicrophoneSettings":
            open_microphone_settings()

    def close(self):
        if self._session:
            self._session.cancel()
        if self._monitor:
            self._monitor.stop()
            self._monitor = None

    def _emit(self, kind, request, **fields):
        detail = {"version": 1, "type": kind, "requestID": request, **fields}
        self._run_js(f"window.dispatchEvent(new CustomEvent('amp:native-dictation', {{ detail: {json.dumps(detail)} }}))")

    # ---- devices -----------------------------------------------------------------------------

    def _devices(self):
        if self._monitor is None:
            self._monitor = Gst.DeviceMonitor()
            self._monitor.add_filter("Audio/Source", None)
            bus = self._monitor.get_bus()
            bus.add_watch(GLib.PRIORITY_DEFAULT, self._device_message)
            self._monitor.start()
            self._known = None
        devices = []
        for device in self._monitor.get_devices():
            props = device.get_properties()
            if props and props.get_string("device.class") == "monitor":
                continue  # what a speaker plays, not a microphone
            devices.append(device)
        return devices

    def _device_message(self, _bus, message):
        if message.type in (Gst.MessageType.DEVICE_ADDED, Gst.MessageType.DEVICE_REMOVED):
            # The monitor also announces the devices it finds while starting; report real changes.
            ids = {self._device_id(d) for d in self._devices()}
            if ids != self._known:
                self._known = ids
                self._emit("sourcesChanged", "devices")
        return True

    @staticmethod
    def _device_id(device):
        try:
            return device.get_property("internal-name") or device.get_display_name()
        except TypeError:
            return device.get_display_name()

    def _sources(self):
        devices = self._devices()
        self._known = {self._device_id(d) for d in devices}
        return [{"deviceID": self._device_id(d), "label": d.get_display_name()} for d in devices]

    def _source_for(self, device_id):
        devices = self._devices()
        chosen = next((d for d in devices if self._device_id(d) == device_id), None) if device_id else None
        if chosen is None:
            chosen = next((d for d in devices if d.get_properties() and d.get_properties().get_value("is-default")), None)
        if chosen is not None:
            return chosen.create_element(None), chosen.get_display_name()
        for name in ("pulsesrc", "pipewiresrc", "autoaudiosrc"):
            if Gst.ElementFactory.find(name):
                return Gst.ElementFactory.make(name, None), "Default Microphone"
        return None, None

    # ---- capture -----------------------------------------------------------------------------

    def _start(self, request, body):
        received = _now_ms()
        if self._session:
            self._session.cancel()
        if not available():
            self._emit("error", request, message="Dictation needs GStreamer with the Opus and WebM plugins.")
            return
        source, label = self._source_for(body.get("deviceID"))
        if source is None:
            self._emit("error", request, message="No microphone was found.", code="microphone-unavailable")
            return
        requested = body.get("requestStartedAtEpochMs")
        self._session = _Session(
            self,
            request,
            source,
            label,
            requested if isinstance(requested, (int, float)) else received,
            received,
        )
        self._session.start()

    def _ended(self, session):
        if self._session is session:
            self._session = None


class _Session:
    def __init__(self, owner, request, source, label, requested_at, received_at):
        self.owner = owner
        self.request = request
        self.label = label
        self.requested_at = requested_at
        self.received_at = received_at
        self.first_buffer_at = None
        self.first_signal_at = None
        self.stop_requested_at = None
        self.samples = 0
        self.peak = 0.0
        self.done = False
        fd, path = tempfile.mkstemp(prefix="amp-dictation-", suffix=".webm")
        GLib.close(fd)
        self.path = Path(path)

        self.pipeline = Gst.Pipeline.new("amp-dictation")
        caps = Gst.Caps.from_string("audio/x-raw,channels=1,rate=48000")
        elements = [
            source,
            Gst.ElementFactory.make("audioconvert", None),
            Gst.ElementFactory.make("audioresample", None),
            Gst.ElementFactory.make("capsfilter", None),
            Gst.ElementFactory.make("level", None),
            Gst.ElementFactory.make("opusenc", None),
            Gst.ElementFactory.make("webmmux", None),
            Gst.ElementFactory.make("filesink", None),
        ]
        elements[3].set_property("caps", caps)
        elements[4].set_property("interval", LEVEL_INTERVAL_NS)
        elements[4].set_property("post-messages", True)
        elements[5].set_property("bitrate", 32000)
        elements[7].set_property("location", str(self.path))
        for element in elements:
            self.pipeline.add(element)
        for a, b in zip(elements, elements[1:]):
            a.link(b)
        bus = self.pipeline.get_bus()
        self._watch = bus.add_watch(GLib.PRIORITY_DEFAULT, self._message)

    def emit(self, kind, **fields):
        self.owner._emit(kind, self.request, **fields)

    def start(self):
        prepared = _now_ms()
        if self.pipeline.set_state(Gst.State.PLAYING) == Gst.StateChangeReturn.FAILURE:
            self._fail("The microphone could not be opened.")
            return
        self.emit(
            "started",
            nativeCues=False,
            uploadStreaming=False,
            requestStartedAtEpochMs=self.requested_at,
            nativeRequestReceivedAtEpochMs=self.received_at,
            nativePreparationStartedAtEpochMs=self.received_at,
            nativePreparedAtEpochMs=prepared,
            nativeRecordStartedAtEpochMs=_now_ms(),
            processing="none",
            deviceLabel=self.label,
        )

    def stop(self):
        if self.done or self.stop_requested_at is not None:
            return
        self.stop_requested_at = _now_ms()
        self.pipeline.send_event(Gst.Event.new_eos())

    def cancel(self):
        if self.done:
            return
        self._teardown()
        self.emit("canceled")

    def _message(self, _bus, message):
        if self.done:
            return False
        if message.type == Gst.MessageType.ELEMENT and message.get_structure().get_name() == "level":
            self._level(message.get_structure())
        elif message.type == Gst.MessageType.EOS:
            self._finish()
            return False
        elif message.type == Gst.MessageType.ERROR:
            error, debug = message.parse_error()
            log.warning("dictation pipeline failed: %s (%s)", error.message, debug)
            self._fail(error.message)
            return False
        return True

    def _level(self, structure):
        ok, values = structure.get_array("rms") if hasattr(structure, "get_array") else (False, None)
        if ok and values is not None:
            decibels = [values.get_nth(i) for i in range(values.n_values)]
        else:
            decibels = list(structure.get_value("rms") or [])
        if not decibels:
            return
        rms = max(0.0, min(1.0, 10 ** (max(decibels) / 20)))
        if rms < 1e-6:
            rms = 0.0
        self.samples += 1
        self.peak = max(self.peak, rms)
        now = _now_ms()
        if self.first_buffer_at is None:
            self.first_buffer_at = now
            self.emit("firstBuffer", firstBufferAtEpochMs=now, rootMeanSquare=rms)
        elif rms > 0 and self.first_signal_at is None:
            self.first_signal_at = now
            self.emit("firstNonzeroBuffer", firstNonzeroBufferAtEpochMs=now, rootMeanSquare=rms)
        else:
            self.emit("level", rootMeanSquare=rms)

    def _finish(self):
        self._teardown(keep_file=True)
        try:
            data = self.path.read_bytes()
        finally:
            self.path.unlink(missing_ok=True)
        delivered = _now_ms()
        self.emit(
            "finished",
            base64=base64.b64encode(data).decode(),
            mimeType=MIME_TYPE,
            stopRequestedAtEpochMs=self.stop_requested_at or delivered,
            finalAudioDeliveredAtEpochMs=delivered,
            firstBufferAtEpochMs=self.first_buffer_at or delivered,
            firstNonzeroBufferAtEpochMs=self.first_signal_at,
            levelSampleCount=self.samples,
            peakRootMeanSquare=self.peak,
            droppedStartupBufferCount=0,
            droppedFormatChangeBufferCount=0,
            stopReason=None,
        )

    def _fail(self, message):
        self._teardown()
        self.emit("error", message=message)

    def _teardown(self, keep_file=False):
        self.done = True
        self.pipeline.set_state(Gst.State.NULL)
        if not keep_file:
            self.path.unlink(missing_ok=True)
        self.owner._ended(self)


def open_microphone_settings():
    for command in SETTINGS_APPS:
        if shutil.which(command[0]):
            subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL, start_new_session=True)
            return True
    return False

"""Files the page hands to the app: opening and revealing local files, saving downloads, and
reading pasted images.

Saving follows the Mac app (NativeSaveFileMessageHandler): files go straight to the Downloads
folder under a unique name, one streamed save at a time, written to a temporary file first.
"""

import base64
import binascii
import os
import tempfile
import uuid
from pathlib import Path
from urllib.parse import unquote, urlsplit

from gi.repository import Gdk, Gio, GLib

IMAGE_TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp", "image/heic", "image/tiff", "image/bmp"]
FILE_LIST_TYPES = ["text/uri-list", "x-special/gnome-copied-files"]
MAX_PASTE_BYTES = 64 * 1024 * 1024


def _local_file(uri):
    if not isinstance(uri, str):
        raise ValueError("expected a file URI")
    parts = urlsplit(uri)
    if parts.scheme != "file" or parts.netloc not in ("", "localhost"):
        raise ValueError("only local file URIs can be opened")
    path = Path(unquote(parts.path))
    if not path.exists():
        raise FileNotFoundError(f"{path} does not exist")
    return Gio.File.new_for_path(str(path))


def _launch_context():
    display = Gdk.Display.get_default()
    return display.get_app_launch_context() if display else None


def open_in_editor(uri):
    """Opens a text file in the app the desktop uses for its type, or for plain text."""
    file = _local_file(uri)
    info = file.query_info(Gio.FILE_ATTRIBUTE_STANDARD_CONTENT_TYPE, Gio.FileQueryInfoFlags.NONE, None)
    content_type = info.get_content_type() or "text/plain"
    app = Gio.AppInfo.get_default_for_type(content_type, False)
    if app is None or not Gio.content_type_is_a(content_type, "text/plain"):
        app = Gio.AppInfo.get_default_for_type("text/plain", False) or app
    if app is None:
        raise RuntimeError("no editor is set up for text files")
    app.launch([file], _launch_context())


def show_in_file_manager(uri, done, failed):
    """Selects the file in the file manager through org.freedesktop.FileManager1, or opens its
    folder when no file manager provides that interface."""
    try:
        file = _local_file(uri)
    except Exception as error:
        failed(str(error))
        return

    def open_folder():
        try:
            Gio.AppInfo.launch_default_for_uri(file.get_parent().get_uri(), _launch_context())
            done(None)
        except GLib.Error as error:
            failed(error.message)

    def shown(bus, result):
        try:
            bus.call_finish(result)
            done(None)
        except GLib.Error:
            open_folder()

    try:
        bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    except GLib.Error:
        open_folder()
        return
    bus.call(
        "org.freedesktop.FileManager1",
        "/org/freedesktop/FileManager1",
        "org.freedesktop.FileManager1",
        "ShowItems",
        GLib.Variant("(ass)", ([file.get_uri()], "")),
        None,
        Gio.DBusCallFlags.NONE,
        5000,
        None,
        shown,
    )


# ---- saving --------------------------------------------------------------------------------


def _downloads():
    folder = GLib.get_user_special_dir(GLib.UserDirectory.DIRECTORY_DOWNLOAD) or str(Path.home() / "Downloads")
    path = Path(folder)
    path.mkdir(parents=True, exist_ok=True)
    return path


def _target(folder, name):
    name = Path(str(name or "")).name.strip() or "download"
    target = folder / name
    stem, suffix = target.stem, target.suffix
    for n in range(1, 1000):
        if not target.exists():
            return target
        target = folder / f"{stem} ({n}){suffix}"
    raise RuntimeError(f"too many files named {name}")


def _finished(app, path):
    notification = Gio.Notification.new("Download Finished")
    notification.set_body(path.name)
    app.send_notification(None, notification)


def save_file(app, body):
    if not isinstance(body, dict) or not isinstance(body.get("base64"), str):
        raise ValueError("invalid save request")
    data = base64.b64decode(body["base64"], validate=True)
    target = _target(_downloads(), body.get("fileName"))
    with open(target, "xb") as out:
        out.write(data)
    _finished(app, target)


class _Transfer:
    def __init__(self, name, size):
        self.id = str(uuid.uuid4())
        self.name = name
        self.size = size
        self.written = 0
        self.folder = _downloads()
        fd, path = tempfile.mkstemp(dir=self.folder, prefix=".amp-download-", suffix=".part")
        umask = os.umask(0)
        os.umask(umask)
        os.fchmod(fd, 0o666 & ~umask)  # mkstemp makes it private; a download is not
        self.file = os.fdopen(fd, "wb")
        self.path = Path(path)

    def discard(self):
        self.file.close()
        self.path.unlink(missing_ok=True)


_active = None


def save_stream(app, body):
    """Streams a save in chunks: begin returns a transfer ID, then append, finish, or cancel."""
    global _active
    action = body.get("action") if isinstance(body, dict) else None
    if action == "begin":
        size = body.get("size")
        if not isinstance(size, int) or size < 0 or not isinstance(body.get("fileName"), str):
            raise ValueError("invalid save request")
        if _active is not None:
            raise RuntimeError("another save is already active")
        try:
            _active = _Transfer(body["fileName"], size)
        except OSError as error:
            raise RuntimeError(f"could not create temporary file: {error}") from error
        return _active.id

    transfer = _active
    if transfer is None or body.get("transferID") != transfer.id:
        raise ValueError("invalid save request")
    if action == "append":
        if body.get("offset") != transfer.written:
            raise ValueError("save chunks are out of order")
        try:
            data = base64.b64decode(body.get("base64") or "", validate=True)
        except (binascii.Error, TypeError) as error:
            raise ValueError("invalid save chunk") from error
        if transfer.written + len(data) > transfer.size:
            raise ValueError("invalid save chunk")
        transfer.file.write(data)
        transfer.written += len(data)
        return None
    if action == "finish":
        _active = None
        if transfer.written != transfer.size:
            transfer.discard()
            raise RuntimeError("saved file size does not match")
        transfer.file.close()
        try:
            target = _target(transfer.folder, transfer.name)
            os.link(transfer.path, target)  # fails instead of replacing a file created meanwhile
        except OSError as error:
            transfer.discard()
            raise RuntimeError(f"could not save file: {error}") from error
        transfer.path.unlink(missing_ok=True)
        _finished(app, target)
        return None
    if action == "cancel":
        _active = None
        transfer.discard()
        return None
    raise ValueError("invalid save request")


# ---- pasteboard ----------------------------------------------------------------------------


def _read_all(clipboard, mime_types, done):
    """Reads the first offered type of mime_types from the clipboard: done(bytes, mime) or
    done(None, None)."""

    def spliced(out, result, mime):
        try:
            out.splice_finish(result)
            done(out.steal_as_bytes().get_data(), mime)
        except GLib.Error:
            done(None, None)

    def opened(cb, result):
        try:
            stream, mime = cb.read_finish(result)
        except GLib.Error:
            done(None, None)
            return
        out = Gio.MemoryOutputStream.new_resizable()
        flags = Gio.OutputStreamSpliceFlags.CLOSE_SOURCE | Gio.OutputStreamSpliceFlags.CLOSE_TARGET
        out.splice_async(stream, flags, GLib.PRIORITY_DEFAULT, None, spliced, mime)

    clipboard.read_async(mime_types, GLib.PRIORITY_DEFAULT, None, opened)


def _image_files(uri_list):
    files, total = [], 0
    for line in uri_list.splitlines():
        line = line.strip()
        if not line.startswith("file://"):
            continue  # comments, and the copy/cut verb of x-special/gnome-copied-files
        path = Path(unquote(urlsplit(line).path))
        mime = Gio.content_type_get_mime_type(Gio.content_type_guess(path.name, None)[0]) or ""
        if not mime.startswith("image/") or not path.is_file():
            continue
        total += path.stat().st_size
        if total > MAX_PASTE_BYTES:
            break
        files.append({"name": path.name, "mimeType": mime, "dataBase64": base64.b64encode(path.read_bytes()).decode()})
    return files


def read_pasteboard_images(clipboard, done, failed):
    """Replies { version: 1, files: [{ name, mimeType, dataBase64 }] } with the images on the
    clipboard: copied image files first, then image data."""
    formats = clipboard.get_formats()

    def reply(files):
        done({"version": 1, "files": files})

    def read_image():
        offered = [t for t in IMAGE_TYPES if formats.contain_mime_type(t)]
        if not offered:
            reply([])
            return

        def got(data, mime):
            if not data or len(data) > MAX_PASTE_BYTES:
                reply([])
                return
            extension = (mime or "image/png").split("/")[1].split("+")[0]
            reply([{"name": f"image.{extension}", "mimeType": mime, "dataBase64": base64.b64encode(data).decode()}])

        _read_all(clipboard, offered, got)

    if not any(formats.contain_mime_type(t) for t in FILE_LIST_TYPES):
        read_image()
        return

    def got_list(data, _mime):
        try:
            files = _image_files(data.decode("utf-8", "replace")) if data else []
        except OSError as error:
            failed(str(error))
            return
        if files:
            reply(files)
        else:
            read_image()

    _read_all(clipboard, FILE_LIST_TYPES, got_list)

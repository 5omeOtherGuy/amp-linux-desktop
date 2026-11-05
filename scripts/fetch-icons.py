#!/usr/bin/env python3
"""Extracts the Amp app icons from Amp's own published macOS disk image into the user's data dir.

The icons are Amp's artwork, so the repository does not carry them; this script takes them from
https://static.ampcode.com/mac/latest.dmg at install time. It needs 7z (p7zip or 7zip) to open
the disk image.
"""

import shutil
import struct
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from amp_native import config  # noqa: E402

DMG_URL = "https://static.ampcode.com/mac/latest.dmg"
VARIANTS = {"dark": "AppIcon.icns", "oled": "AppIconBlack.icns", "light": "AppIconLight.icns", "puck": "AppIconPuck.icns"}


def largest_png(icns):
    data = icns.read_bytes()
    best, offset = b"", 8
    while offset + 8 <= len(data):
        kind, length = struct.unpack(">4sI", data[offset:offset + 8])
        chunk = data[offset + 8:offset + length]
        if chunk.startswith(b"\x89PNG") and len(chunk) > len(best):
            best = chunk
        offset += max(length, 8)
    return best


def main():
    sevenzip = shutil.which("7z") or shutil.which("7zz")
    if not sevenzip:
        sys.exit("fetch-icons: install 7z (p7zip) to extract the icons from Amp's disk image")
    config.ICON_DIR.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        dmg = Path(tmp) / "amp.dmg"
        urllib.request.urlretrieve(DMG_URL, dmg)
        subprocess.run([sevenzip, "x", "-y", f"-o{tmp}/x", str(dmg), "Amp.app/Contents/Resources/*.icns"],
                       check=True, stdout=subprocess.DEVNULL)
        resources = Path(tmp) / "x/Amp.app/Contents/Resources"
        for key, name in VARIANTS.items():
            png = largest_png(resources / name) if (resources / name).exists() else b""
            if png:
                (config.ICON_DIR / f"{key}.png").write_bytes(png)
                print(f"fetch-icons: {key}.png")
    if (config.ICON_DIR / "dark.png").exists():
        shutil.copyfile(config.ICON_DIR / "dark.png", config.ICON_DIR / "app.png")


if __name__ == "__main__":
    main()

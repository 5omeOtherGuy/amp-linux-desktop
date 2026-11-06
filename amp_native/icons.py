"""Extracts the Amp app icons from Amp's own published macOS disk image.

The icons are Amp's artwork, so neither the repository nor a prebuilt package carries them. They
come from https://static.ampcode.com/mac/latest.dmg: scripts/install and the Arch package extract
them at install or build time, and the app fetches them on first start when neither did (the .deb
and .rpm). It needs 7z (the 7zip package) to open the disk image.

    python3 -m amp_native.icons [--dmg PATH] [--out DIR]

--dmg reads a disk image already downloaded instead (the Arch package pins one), and --out writes
the icons to DIR instead of the user's data dir.
"""

import argparse
import shutil
import struct
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

from . import config

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


def fetch(out=config.ICON_DIR, dmg=None):
    """Writes the icon variants and app.png to out; returns the written names. Raises on failure."""
    sevenzip = shutil.which("7z") or shutil.which("7zz")
    if not sevenzip:
        raise RuntimeError("install 7z (7zip) to extract the icons from Amp's disk image")
    out.mkdir(parents=True, exist_ok=True)
    written = []
    with tempfile.TemporaryDirectory() as tmp:
        if dmg is None:
            dmg = Path(tmp) / "amp.dmg"
            urllib.request.urlretrieve(DMG_URL, dmg)
        subprocess.run([sevenzip, "x", "-y", f"-o{tmp}/x", str(dmg), "Amp.app/Contents/Resources/*.icns"],
                       check=True, stdout=subprocess.DEVNULL)
        resources = Path(tmp) / "x/Amp.app/Contents/Resources"
        for key, name in VARIANTS.items():
            png = largest_png(resources / name) if (resources / name).exists() else b""
            if png:
                (out / f"{key}.png").write_bytes(png)
                written.append(f"{key}.png")
    if not written:
        raise RuntimeError("Amp's disk image had none of the expected icons")
    if (out / "dark.png").exists():
        shutil.copyfile(out / "dark.png", out / "app.png")
    return written


def main():
    parser = argparse.ArgumentParser(description="Extract Amp's app icons from its macOS disk image.")
    parser.add_argument("--dmg", type=Path, help="a downloaded disk image (default: download the latest)")
    parser.add_argument("--out", type=Path, default=config.ICON_DIR, help="output directory")
    args = parser.parse_args()
    try:
        for name in fetch(args.out, args.dmg):
            print(f"icons: {name}")
    except (RuntimeError, OSError, subprocess.CalledProcessError) as error:
        sys.exit(f"icons: {error}")


if __name__ == "__main__":
    main()

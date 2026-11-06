#!/usr/bin/env python3
"""Extracts the Amp app icons from Amp's own published macOS disk image into the user's data dir.

The icons are Amp's artwork, so the repository does not carry them; this script takes them from
https://static.ampcode.com/mac/latest.dmg at install time. It needs 7z (the 7zip package) to open
the disk image.

    fetch-icons.py [--dmg PATH] [--out DIR]

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
    parser = argparse.ArgumentParser(description="Extract Amp's app icons from its macOS disk image.")
    parser.add_argument("--dmg", type=Path, help="a downloaded disk image (default: download the latest)")
    parser.add_argument("--out", type=Path, default=config.ICON_DIR, help="output directory")
    args = parser.parse_args()
    sevenzip = shutil.which("7z") or shutil.which("7zz")
    if not sevenzip:
        sys.exit("fetch-icons: install 7z (7zip) to extract the icons from Amp's disk image")
    args.out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        dmg = args.dmg or Path(tmp) / "amp.dmg"
        if args.dmg is None:
            urllib.request.urlretrieve(DMG_URL, dmg)
        subprocess.run([sevenzip, "x", "-y", f"-o{tmp}/x", str(dmg), "Amp.app/Contents/Resources/*.icns"],
                       check=True, stdout=subprocess.DEVNULL)
        resources = Path(tmp) / "x/Amp.app/Contents/Resources"
        for key, name in VARIANTS.items():
            png = largest_png(resources / name) if (resources / name).exists() else b""
            if png:
                (args.out / f"{key}.png").write_bytes(png)
                print(f"fetch-icons: {key}.png")
    if (args.out / "dark.png").exists():
        shutil.copyfile(args.out / "dark.png", args.out / "app.png")


if __name__ == "__main__":
    main()

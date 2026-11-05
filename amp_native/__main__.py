import logging
import os
import sys

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
gi.require_version("WebKit", "6.0")
gi.require_version("JavaScriptCore", "6.0")

from . import config  # noqa: E402


def main():
    # GTK 4.16+ defaults to its Vulkan renderer, which shows WebKit's frames late: on an RTX 3060
    # (driver 615, GTK 4.24, WebKitGTK 2.54) keypress-to-screen in the composer was 135 ms median
    # with Vulkan and 50 ms with GL (Chromium: 36 ms). GSK_RENDERER set by the user still wins.
    os.environ.setdefault("GSK_RENDERER", "gl")
    # WebKit picks the NVIDIA decoders for the Desktop tab's WebCodecs video, but they cannot reach
    # the GPU inside its web process sandbox, and WebKit does not fall back. Demote them so the
    # software decoders (gst-libav for H.264) are used. The web process inherits this.
    nvidia = ",".join(f"nv{codec}dec:NONE" for codec in ("h264", "h265", "vp8", "vp9", "av1"))
    ranks = os.environ.get("GST_PLUGIN_FEATURE_RANK")
    os.environ["GST_PLUGIN_FEATURE_RANK"] = f"{ranks},{nvidia}" if ranks else nvidia
    config.CACHE_DIR.mkdir(parents=True, exist_ok=True)
    logging.basicConfig(
        filename=config.LOG_FILE,
        level=logging.DEBUG if os.environ.get("AMP_NATIVE_DEBUG") else logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )
    from .app import AmpApplication

    return AmpApplication().run(sys.argv)


if __name__ == "__main__":
    sys.exit(main())

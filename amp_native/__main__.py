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

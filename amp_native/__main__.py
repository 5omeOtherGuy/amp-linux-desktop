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

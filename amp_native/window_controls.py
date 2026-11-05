"""The desktop's own window controls, placed over the page's title strip.

The page leaves room at the left of its title strip for window controls (the Mac shell's
--amp-macos-window-controls-left-inset). This puts GTK's controls there and at the right, in the
order the desktop sets in gtk-decoration-layout, so they look and sit the way every other app on
that desktop does. A tiling desktop with an empty layout gets no controls and no gap.
"""

from gi.repository import Gtk

STRIP_HEIGHT = 36
EDGE = 6


class WindowControls:
    def __init__(self, on_change):
        self._on_change = on_change
        self.start = self._controls(Gtk.PackType.START, Gtk.Align.START)
        self.end = self._controls(Gtk.PackType.END, Gtk.Align.END)

    def _controls(self, side, halign):
        controls = Gtk.WindowControls(side=side, halign=halign, valign=Gtk.Align.START)
        controls.add_css_class("amp-window-controls")
        controls.set_size_request(-1, STRIP_HEIGHT)
        controls.set_margin_start(EDGE)
        controls.set_margin_end(EDGE)
        controls.connect("notify::empty", lambda *_: self._on_change())
        controls.connect("notify::decoration-layout", lambda *_: self._on_change())
        return controls

    def set_visible(self, visible):
        self.start.set_visible(visible)
        self.end.set_visible(visible)
        self._on_change()

    def _width(self, controls):
        if not controls.get_visible() or controls.get_empty():
            return 0
        natural = controls.measure(Gtk.Orientation.HORIZONTAL, -1)[1]
        return natural + 2 * EDGE

    def insets(self):
        """Pixels the page should keep free at its left and right edges."""
        return self._width(self.start), self._width(self.end)

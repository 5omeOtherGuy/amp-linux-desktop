//! The desktop's own window controls, placed over the page's title strip.
//!
//! The page leaves room at the left of its title strip for window controls (the Mac shell's
//! --amp-macos-window-controls-left-inset). This puts GTK's controls there and at the right, in the
//! order the desktop sets in gtk-decoration-layout, so they look and sit the way every other app on
//! that desktop does. A tiling desktop with an empty layout gets no controls and no gap.

use gtk::prelude::*;

const STRIP_HEIGHT: i32 = 36;
const EDGE: i32 = 6;

pub struct WindowControls {
    pub start: gtk::WindowControls,
    pub end: gtk::WindowControls,
}

impl WindowControls {
    /// `on_change` runs when the controls' width may have changed.
    pub fn new(on_change: impl Fn() + Clone + 'static) -> Self {
        let controls = |side, halign| {
            let controls = gtk::WindowControls::builder().side(side).halign(halign).valign(gtk::Align::Start).build();
            controls.add_css_class("amp-window-controls");
            controls.set_size_request(-1, STRIP_HEIGHT);
            controls.set_margin_start(EDGE);
            controls.set_margin_end(EDGE);
            let changed = on_change.clone();
            controls.connect_empty_notify(move |_| changed());
            let changed = on_change.clone();
            controls.connect_decoration_layout_notify(move |_| changed());
            controls
        };
        Self {
            start: controls(gtk::PackType::Start, gtk::Align::Start),
            end: controls(gtk::PackType::End, gtk::Align::End),
        }
    }

    pub fn set_visible(&self, visible: bool) {
        self.start.set_visible(visible);
        self.end.set_visible(visible);
    }

    fn width(controls: &gtk::WindowControls) -> i32 {
        if !controls.is_visible() || controls.is_empty() {
            return 0;
        }
        controls.measure(gtk::Orientation::Horizontal, -1).1 + 2 * EDGE
    }

    /// Pixels the page should keep free at its left and right edges.
    pub fn insets(&self) -> (i32, i32) {
        (Self::width(&self.start), Self::width(&self.end))
    }
}

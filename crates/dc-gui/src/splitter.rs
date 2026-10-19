//! Divider between the two panels (`MainSplitter` of the original): drag it
//! to change the ratio, which is shown in percent while dragging; a double
//! click sets 50:50. The ratio, not the pixel position, stays when the
//! window is resized.

use std::cell::Cell;
use std::rc::Rc;

use dc_i18n::{fill, gettext};
use gtk::glib;
use gtk::prelude::*;

/// How far from the handle a press still counts as on the handle (pixels).
const HANDLE_TOLERANCE: f64 = 3.0;

/// Share of the left panel in hundredths of a percent, `0..=10_000`
/// (5000 = 50.00 %). An integer rather than a float: it is stored and
/// compared exactly, cannot be NaN, and a hundredth of a percent is below
/// half a pixel up to 8K, so pixel -> ratio -> pixel is exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ratio(u16);

impl Ratio {
    const FULL: i64 = 10_000;
    const HALF: Self = Self(5_000);

    fn from_percent(percent: u8) -> Self {
        Self(u16::from(percent.min(100)) * 100)
    }

    /// Ratio of `position` in `shared_width`, rounded to the nearest
    /// hundredth; `None` before layout.
    fn of(position: i32, shared_width: i32) -> Option<Self> {
        if shared_width <= 0 {
            return None;
        }
        let (position, width) = (i64::from(position.clamp(0, shared_width)), i64::from(shared_width));
        let hundredths = (2 * position * Self::FULL + width) / (2 * width);
        u16::try_from(hundredths).ok().map(Self)
    }

    /// Divider position in whole pixels, rounded to the nearest; `None`
    /// before layout.
    ///
    /// Stable by construction: the position is always computed afresh from
    /// the remembered ratio (never adjusted by a delta), and the ratio is
    /// only taken from a pixel position when the user drags. Resizing the
    /// window back and forth always lands on the same pixel.
    fn position(self, shared_width: i32) -> Option<i32> {
        if shared_width <= 0 {
            return None;
        }
        let width = i64::from(shared_width);
        let position = (2 * i64::from(self.0) * width + Self::FULL) / (2 * Self::FULL);
        i32::try_from(position).ok()
    }

    /// "42.50" for showing as a percentage.
    fn percent_text(self) -> String {
        format!("{}.{:02}", self.0 / 100, self.0 % 100)
    }
}

pub struct Splitter {
    paned: gtk::Paned,
    /// Share of the left panel of the width both panels share
    /// (`FMainSplitterPos` of the original).
    ratio: Cell<Ratio>,
    /// Set while the position is changed by us, not by the user.
    adjusting: Cell<bool>,
    hint: gtk::Popover,
    hint_label: gtk::Label,
    dragging: Cell<bool>,
}

impl Splitter {
    pub fn new(left: &impl IsA<gtk::Widget>, right: &impl IsA<gtk::Widget>) -> Rc<Self> {
        let paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(left)
            .end_child(right)
            .resize_start_child(true)
            .resize_end_child(true)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .wide_handle(true)
            .vexpand(true)
            .build();
        let hint_label = gtk::Label::new(None);
        let hint = gtk::Popover::builder().child(&hint_label).autohide(false).has_arrow(false).build();
        hint.set_parent(&paned);

        let splitter = Rc::new(Self {
            paned,
            ratio: Cell::new(Ratio::HALF),
            adjusting: Cell::new(false),
            hint,
            hint_label,
            dragging: Cell::new(false),
        });
        splitter.connect_signals();
        splitter
    }

    pub fn widget(&self) -> &gtk::Paned {
        &self.paned
    }

    /// `cm_PanelsSplitterPerPos`: the left panel gets `percent` of the width.
    pub fn set_percent(&self, percent: u8) {
        self.ratio.set(Ratio::from_percent(percent));
        self.apply();
    }

    fn connect_signals(self: &Rc<Self>) {
        // A new size (first layout, window resized): keep the ratio. The
        // notification comes during layout, where a new position does not
        // take effect, hence right after it.
        let weak = Rc::downgrade(self);
        self.paned.connect_notify_local(Some("max-position"), move |_, _| {
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                if let Some(splitter) = weak.upgrade() {
                    splitter.apply();
                }
            });
        });

        // Moved by the user: remember the ratio and show it. GTK moves the
        // divider by itself while laying out; only a drag is the user's ratio.
        let weak = Rc::downgrade(self);
        self.paned.connect_notify_local(Some("position"), move |paned, _| {
            let Some(splitter) = weak.upgrade() else { return };
            if splitter.adjusting.get() || !splitter.dragging.get() {
                return;
            }
            if let Some(ratio) = Ratio::of(paned.position(), splitter.shared_width()) {
                splitter.ratio.set(ratio);
            }
            splitter.show_hint();
        });

        // The paned handles the drag itself; this only follows it.
        let drag = gtk::GestureDrag::new();
        drag.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        drag.connect_drag_begin(move |_, x, _| {
            if let Some(splitter) = weak.upgrade()
                && splitter.on_handle(x)
            {
                splitter.dragging.set(true);
            }
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_end(move |_, _, _| {
            if let Some(splitter) = weak.upgrade() {
                splitter.dragging.set(false);
                splitter.hint.popdown();
            }
        });
        self.paned.add_controller(drag);

        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |_, presses, x, _| {
            if let Some(splitter) = weak.upgrade()
                && presses == 2
                && splitter.on_handle(x)
            {
                splitter.set_percent(50);
            }
        });
        self.paned.add_controller(click);
    }

    /// Puts the divider where the remembered ratio says, within what the
    /// minimum widths of both panels allow.
    fn apply(&self) {
        let Some(position) = self.ratio.get().position(self.shared_width()) else { return };
        let (min, max) = (self.paned.min_position(), self.paned.max_position());
        self.adjusting.set(true);
        self.paned.set_position(position.clamp(min, max.max(min)));
        self.adjusting.set(false);
    }

    /// Width of the handle: what the two panels do not get.
    fn handle_width(&self) -> i32 {
        let child_width = |child: Option<gtk::Widget>| child.map_or(0, |child| child.width());
        (self.paned.width() - child_width(self.paned.start_child()) - child_width(self.paned.end_child())).max(0)
    }

    /// Width the two panels share (`max-position` is not it: it leaves room
    /// for the minimum width of the right panel).
    fn shared_width(&self) -> i32 {
        self.paned.width() - self.handle_width()
    }

    /// Whether `x` (paned coordinates) is on the divider, not in a panel.
    fn on_handle(&self, x: f64) -> bool {
        let start = f64::from(self.paned.position());
        let end = start + f64::from(self.handle_width());
        x >= start - HANDLE_TOLERANCE && x <= end + HANDLE_TOLERANCE
    }

    /// "42.50%" next to the divider, as the original shows while dragging.
    fn show_hint(&self) {
        let percent = self.ratio.get().percent_text();
        self.hint_label.set_text(&fill(&gettext("{percent}%"), &[("percent", &percent)]));
        let rect = gtk::gdk::Rectangle::new(self.paned.position(), self.paned.height() / 2, 1, 1);
        self.hint.set_pointing_to(Some(&rect));
        if !self.hint.is_visible() {
            self.hint.popup();
        }
    }
}

impl Drop for Splitter {
    fn drop(&mut self) {
        self.hint.unparent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_and_position_convert_both_ways() {
        assert_eq!(Ratio::HALF.position(1190), Some(595));
        assert_eq!(Ratio::from_percent(30).position(1000), Some(300));
        assert_eq!(Ratio::of(300, 1000), Some(Ratio(3000)));
        assert_eq!(Ratio::of(1, 3), Some(Ratio(3333)));
        assert_eq!(Ratio::from_percent(200), Ratio(10_000));
        assert_eq!(Ratio::HALF.position(0), None);
        assert_eq!(Ratio::of(10, 0), None);
        assert_eq!(Ratio(4250).percent_text(), "42.50");
        assert_eq!(Ratio(5).percent_text(), "0.05");
    }

    #[test]
    fn dragged_pixel_survives_the_round_trip_exactly_up_to_8k() {
        for width in 1..=7680 {
            for position in 0..=width {
                let ratio = Ratio::of(position, width).unwrap();
                assert_eq!(ratio.position(width), Some(position), "width {width}, position {position}");
            }
        }
    }

    #[test]
    fn resizing_back_and_forth_does_not_drift() {
        let ratio = Ratio::of(437, 1190).unwrap();
        for width in (300..3000).step_by(7).chain((300..3000).rev().step_by(11)) {
            let _ = ratio.position(width);
        }
        assert_eq!(ratio.position(1190), Some(437));
    }

    #[test]
    fn same_share_on_any_window_width() {
        let ratio = Ratio::of(270, 900).unwrap();
        assert_eq!(ratio, Ratio(3000));
        for width in [600, 1190, 1904] {
            let position = ratio.position(width).unwrap();
            assert_eq!(Ratio::of(position, width).map(|r| (i32::from(r.0) - 3000).abs() <= 1), Some(true), "{width}");
        }
    }
}

//! Layout that puts the children in a row and wraps them onto more lines
//! when the row does not fit, like the drive buttons of the original.
//!
//! `AdwWrapBox` does the same, but only from libadwaita 1.7.

use std::ops::Range;

use gtk::glib;
use gtk::prelude::*;

/// Space between children, in both directions.
const SPACING: i32 = 2;

mod imp {
    use super::{Range, SPACING, glib, lines};
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    #[derive(Debug, Default)]
    pub struct WrapLayout;

    #[glib::object_subclass]
    impl ObjectSubclass for WrapLayout {
        const NAME: &'static str = "DcWrapLayout";
        type Type = super::WrapLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for WrapLayout {}

    impl LayoutManagerImpl for WrapLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, widget: &gtk::Widget, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let children = children(widget);
            if orientation == gtk::Orientation::Horizontal {
                // At least the widest child, at best everything in one line.
                let minimum = children.iter().map(|child| child.measure(orientation, -1).0).max().unwrap_or(0);
                return (minimum, line_width(&natural_widths(&children)), -1, -1);
            }
            let height = |width: i32| layout(&children, width).last().map_or(0, |line| line.y + line.height);
            if for_size >= 0 {
                let height = height(for_size);
                (height, height, -1, -1)
            } else {
                // Without a width as in one line; the real width comes later and
                // the narrowest would make the window's minimum height too big.
                let (_, natural, _, _) = self.measure(widget, gtk::Orientation::Horizontal, -1);
                let height = height(natural);
                (height, height, -1, -1)
            }
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, _height: i32, _baseline: i32) {
            let children = children(widget);
            let widths = natural_widths(&children);
            for line in layout(&children, width) {
                let mut x = 0;
                for index in line.children {
                    let child_width = widths[index].min(width);
                    children[index].size_allocate(&gtk::Allocation::new(x, line.y, child_width, line.height), -1);
                    x += child_width + SPACING;
                }
            }
        }
    }

    pub struct Line {
        pub children: Range<usize>,
        pub y: i32,
        pub height: i32,
    }

    fn children(widget: &gtk::Widget) -> Vec<gtk::Widget> {
        std::iter::successors(widget.first_child(), gtk::Widget::next_sibling)
            .filter(gtk::Widget::should_layout)
            .collect()
    }

    fn natural_widths(children: &[gtk::Widget]) -> Vec<i32> {
        children.iter().map(|child| child.measure(gtk::Orientation::Horizontal, -1).1).collect()
    }

    fn line_width(widths: &[i32]) -> i32 {
        let gaps = widths.len().saturating_sub(1);
        widths.iter().sum::<i32>() + SPACING * i32::try_from(gaps).unwrap_or(i32::MAX)
    }

    /// The lines at `width`, each as tall as its tallest child.
    fn layout(children: &[gtk::Widget], width: i32) -> Vec<Line> {
        let widths = natural_widths(children);
        let mut y = 0;
        lines(&widths, width)
            .into_iter()
            .map(|range| {
                let height = children[range.clone()]
                    .iter()
                    .zip(&widths[range.clone()])
                    .map(|(child, &child_width)| child.measure(gtk::Orientation::Vertical, child_width.min(width)).1)
                    .max()
                    .unwrap_or(0);
                let line = Line { children: range, y, height };
                y += height + SPACING;
                line
            })
            .collect()
    }
}

glib::wrapper! {
    pub struct WrapLayout(ObjectSubclass<imp::WrapLayout>) @extends gtk::LayoutManager;
}

impl WrapLayout {
    /// A box whose children wrap onto more lines.
    pub fn new_box() -> gtk::Box {
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        container.set_layout_manager(Some(glib::Object::new::<Self>()));
        container
    }
}

/// Splits children of `widths` into lines no wider than `available`; every
/// line has at least one child, even one wider than that.
fn lines(widths: &[i32], available: i32) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let (mut start, mut x) = (0, 0);
    for (index, &width) in widths.iter().enumerate() {
        if index > start && x + SPACING + width > available {
            lines.push(start..index);
            (start, x) = (index, 0);
        }
        x += if index > start { SPACING + width } else { width };
    }
    if start < widths.len() {
        lines.push(start..widths.len());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::lines;

    #[test]
    fn everything_fits_in_one_line() {
        assert_eq!(lines(&[10, 10, 10], 34), vec![0..3]);
    }

    #[test]
    fn wraps_when_the_next_child_does_not_fit() {
        assert_eq!(lines(&[10, 10, 10], 33), [0..2, 2..3]);
        assert_eq!(lines(&[10, 10, 10], 10), [0..1, 1..2, 2..3]);
    }

    #[test]
    fn a_too_wide_child_gets_its_own_line() {
        assert_eq!(lines(&[10, 50, 10], 30), [0..1, 1..2, 2..3]);
    }

    #[test]
    fn no_children_no_lines() {
        assert!(lines(&[], 100).is_empty());
    }
}

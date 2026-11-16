//! The "Times" page: the times of modification and access, to set (as
//! `touch`). Birth and the last change of metadata are kept by the kernel,
//! they cannot be set.

use std::rc::Rc;
use std::time::SystemTime;

use dc_i18n::gettext;
use gtk::prelude::*;
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};

use crate::date_time::{DateField, TimeField};

/// One time: whether it changes, the day, the time of day.
struct Row {
    check: gtk::CheckButton,
    date: DateField,
    time: TimeField,
}

impl Row {
    fn new(title: &str, initial: SystemTime) -> Self {
        let row = Self { check: gtk::CheckButton::with_mnemonic(title), date: DateField::new(), time: TimeField::new() };
        let zoned = Timestamp::try_from(initial).unwrap_or(Timestamp::UNIX_EPOCH).to_zoned(TimeZone::system());
        row.set(&zoned);
        row
    }

    fn set(&self, zoned: &Zoned) {
        self.date.set_date(zoned.date());
        self.time.set_time(zoned.time());
    }

    fn value(&self) -> Option<SystemTime> {
        if !self.check.is_active() {
            return None;
        }
        let zoned = self.date.date().to_datetime(self.time.time()).to_zoned(TimeZone::system()).ok()?;
        Some(SystemTime::from(zoned.timestamp()))
    }
}

pub(super) struct Times {
    pub page: gtk::Box,
    modified: Row,
    accessed: Row,
}

impl Times {
    pub fn new(modified: SystemTime, accessed: SystemTime) -> Rc<Self> {
        let times = Rc::new(Self {
            page: gtk::Box::new(gtk::Orientation::Vertical, 12),
            modified: Row::new(&gettext("_Modified:"), modified),
            accessed: Row::new(&gettext("_Accessed:"), accessed),
        });
        let grid = gtk::Grid::builder().row_spacing(8).column_spacing(12).build();
        for (index, row) in [&times.modified, &times.accessed].into_iter().enumerate() {
            let index = i32::try_from(index).unwrap_or(0);
            grid.attach(&row.check, 0, index, 1, 1);
            grid.attach(&row.date.button, 1, index, 1, 1);
            grid.attach(&row.time.row, 2, index, 1, 1);
            let now = gtk::Button::with_label(&gettext("Now"));
            grid.attach(&now, 3, index, 1, 1);
            let check = row.check.downgrade();
            let times_weak = Rc::downgrade(&times);
            now.connect_clicked(move |_| {
                let Some(this) = times_weak.upgrade() else { return };
                let row = if index == 0 { &this.modified } else { &this.accessed };
                row.set(&Zoned::now());
                if let Some(check) = check.upgrade() {
                    check.set_active(true);
                }
            });
            // A value changed by hand means it is to be set.
            let check = row.check.downgrade();
            let on = move || {
                if let Some(check) = check.upgrade() {
                    check.set_active(true);
                }
            };
            let on_date = on.clone();
            row.date.calendar.connect_day_selected(move |_| on_date());
            row.time.connect_changed(on);
        }
        times.page.append(&grid);
        // The values shown at first are the items' own: not a change.
        times.modified.check.set_active(false);
        times.accessed.check.set_active(false);
        let note = gtk::Label::builder()
            .label(gettext("The time of birth and of the last change of metadata are kept by the kernel and cannot be set."))
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label", "caption"])
            .build();
        times.page.append(&note);
        times
    }

    pub fn connect_changed(&self, changed: impl Fn() + Clone + 'static) {
        for row in [&self.modified, &self.accessed] {
            let changed = changed.clone();
            row.check.connect_toggled(move |_| changed());
        }
    }

    /// The new times of modification and access, those to set.
    pub fn changes(&self) -> (Option<SystemTime>, Option<SystemTime>) {
        (self.modified.value(), self.accessed.value())
    }
}

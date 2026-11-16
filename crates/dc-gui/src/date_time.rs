//! A day picked from a calendar under a button and a time of day typed
//! as `hh:mm:ss`: the date and time fields of the window to find files and
//! of the properties of files.

use dc_i18n::{fill, gettext};
use gtk::prelude::*;
use jiff::civil::{Date, Time};

/// A day picked from a calendar under a button.
pub struct DateField {
    pub button: gtk::MenuButton,
    pub calendar: gtk::Calendar,
}

impl DateField {
    pub fn new() -> Self {
        let calendar = gtk::Calendar::new();
        let popover = gtk::Popover::builder().child(&calendar).build();
        let button = gtk::MenuButton::builder().popover(&popover).always_show_arrow(true).valign(gtk::Align::Center).build();
        let this = Self { button, calendar };
        this.show_label();
        let (button, popover_weak) = (this.button.downgrade(), popover.downgrade());
        this.calendar.connect_day_selected(move |calendar| {
            if let Some(button) = button.upgrade() {
                button.set_label(&date_label(calendar_date(calendar)));
            }
            if let Some(popover) = popover_weak.upgrade() {
                popover.popdown();
            }
        });
        this
    }

    fn show_label(&self) {
        self.button.set_label(&date_label(calendar_date(&self.calendar)));
    }

    pub fn date(&self) -> Date {
        calendar_date(&self.calendar)
    }

    pub fn set_date(&self, date: Date) {
        // Day 1 first: the 31st does not fit every month on the way.
        self.calendar.set_day(1);
        self.calendar.set_year(i32::from(date.year()));
        self.calendar.set_month(i32::from(date.month()) - 1);
        self.calendar.set_day(i32::from(date.day()));
        self.show_label();
    }
}

fn calendar_date(calendar: &gtk::Calendar) -> Date {
    let day = calendar.date();
    let part = |value: i32| i8::try_from(value).unwrap_or(1);
    Date::new(i16::try_from(day.year()).unwrap_or(2000), part(day.month()), part(day.day_of_month()))
        .unwrap_or(jiff::civil::date(2000, 1, 1))
}

fn date_label(date: Date) -> String {
    fill(&gettext("{day}. {month}. {year}"), &[("day", &date.day()), ("month", &date.month()), ("year", &date.year())])
}

/// A time of day typed as `hh:mm` or `hh:mm:ss`.
pub struct TimeField {
    pub row: gtk::Entry,
}

impl TimeField {
    pub fn new() -> Self {
        let row = gtk::Entry::builder().width_chars(8).max_width_chars(8).placeholder_text("hh:mm:ss").valign(gtk::Align::Center).build();
        // A time that cannot be read shows as an error until it can.
        row.connect_changed(|entry| {
            if parse_time(&entry.text()).is_some() {
                entry.remove_css_class("error");
            } else {
                entry.add_css_class("error");
            }
        });
        Self { row }
    }

    /// Midnight for a time that cannot be read (shown as an error).
    pub fn time(&self) -> Time {
        parse_time(&self.row.text()).unwrap_or(Time::midnight())
    }

    pub fn set_time(&self, time: Time) {
        self.row.set_text(&format!("{:02}:{:02}:{:02}", time.hour(), time.minute(), time.second()));
    }

    pub fn connect_changed(&self, changed: impl Fn() + 'static) {
        self.row.connect_changed(move |_| changed());
    }
}

/// `h:m` or `h:m:s`, 24 hours.
fn parse_time(text: &str) -> Option<Time> {
    let parts: Vec<i8> = text.trim().split(':').map(|part| part.trim().parse().ok()).collect::<Option<_>>()?;
    match parts.as_slice() {
        [hour, minute] => Time::new(*hour, *minute, 0, 0).ok(),
        [hour, minute, second] => Time::new(*hour, *minute, *second, 0).ok(),
        _ => None,
    }
}


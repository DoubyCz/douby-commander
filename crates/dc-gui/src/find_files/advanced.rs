//! The "Advanced" page: modification time, size, types and permissions,
//! duplicate files.

use dc_core::find::{Age, Attributes, Duplicates, FindQuery, SizeUnit, TimeUnit};
use dc_i18n::{fill, gettext};
use gtk::prelude::*;
use gtk::glib;
use jiff::civil::{Date, Time};

use super::{check, section};

/// A day picked from a calendar under a button.
struct DateField {
    button: gtk::MenuButton,
    calendar: gtk::Calendar,
}

impl DateField {
    fn new() -> Self {
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

    fn date(&self) -> Date {
        calendar_date(&self.calendar)
    }

    fn set_date(&self, date: Date) {
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
struct TimeField {
    row: gtk::Entry,
}

impl TimeField {
    fn new() -> Self {
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
    fn time(&self) -> Time {
        parse_time(&self.row.text()).unwrap_or(Time::midnight())
    }

    fn set_time(&self, time: Time) {
        self.row.set_text(&format!("{:02}:{:02}:{:02}", time.hour(), time.minute(), time.second()));
    }

    fn connect_changed(&self, changed: impl Fn() + 'static) {
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

pub(super) struct Advanced {
    pub page: gtk::Box,
    date_from_check: gtk::CheckButton,
    date_from: DateField,
    time_from_check: gtk::CheckButton,
    time_from: TimeField,
    date_to_check: gtk::CheckButton,
    date_to: DateField,
    time_to_check: gtk::CheckButton,
    time_to: TimeField,
    age_check: gtk::CheckButton,
    age: gtk::SpinButton,
    age_unit: gtk::DropDown,
    age_titles: gtk::StringList,
    size_from_check: gtk::CheckButton,
    size_from: gtk::SpinButton,
    size_to_check: gtk::CheckButton,
    size_to: gtk::SpinButton,
    size_unit: gtk::DropDown,
    attributes: gtk::Entry,
    duplicates: gtk::CheckButton,
    same_name: gtk::CheckButton,
    same_size: gtk::CheckButton,
    same_hash: gtk::CheckButton,
    same_content: gtk::CheckButton,
}

fn number(max: f64) -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(0.0, max, 1.0);
    spin.set_digits(0);
    spin
}

impl Advanced {
    pub fn new() -> Self {
        let age_titles = gtk::StringList::new(&[]);
        let units: Vec<String> = SizeUnit::ALL.iter().map(|unit| unit.title()).collect();
        let this = Self {
            page: gtk::Box::new(gtk::Orientation::Vertical, 10),
            date_from_check: check(&gettext("_Date from:")),
            date_from: DateField::new(),
            time_from_check: check(&gettext("_Time from:")),
            time_from: TimeField::new(),
            date_to_check: check(&gettext("Dat_e to:")),
            date_to: DateField::new(),
            time_to_check: check(&gettext("Ti_me to:")),
            time_to: TimeField::new(),
            age_check: check(&gettext("N_ot older than:")),
            age: number(1e6),
            age_unit: gtk::DropDown::builder().model(&age_titles).build(),
            age_titles,
            size_from_check: check(&gettext("S_ize from:")),
            size_from: number(1e15),
            size_to_check: check(&gettext("Si_ze to:")),
            size_to: number(1e15),
            size_unit: gtk::DropDown::from_strings(&units.iter().map(String::as_str).collect::<Vec<_>>()),
            attributes: gtk::Entry::builder().hexpand(true).activates_default(true).build(),
            duplicates: check(&gettext("Find du_plicate files:")),
            same_name: check(&gettext("same name")),
            same_size: check(&gettext("same size")),
            same_hash: check(&gettext("same content (hash)")),
            same_content: check(&gettext("same content (byte by byte)")),
        };
        this.age.set_range(1.0, 1e6);
        this.update_age_titles();
        this.lay_out();
        this
    }

    fn lay_out(&self) {
        let time = gtk::Grid::builder().row_spacing(6).column_spacing(12).build();
        time.attach(&self.date_from_check, 0, 0, 1, 1);
        time.attach(&self.date_from.button, 1, 0, 1, 1);
        time.attach(&self.time_from_check, 2, 0, 1, 1);
        time.attach(&self.time_from.row, 3, 0, 1, 1);
        time.attach(&self.date_to_check, 0, 1, 1, 1);
        time.attach(&self.date_to.button, 1, 1, 1, 1);
        time.attach(&self.time_to_check, 2, 1, 1, 1);
        time.attach(&self.time_to.row, 3, 1, 1, 1);
        time.attach(&self.age_check, 0, 2, 1, 1);
        let age = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        age.append(&self.age);
        age.append(&self.age_unit);
        time.attach(&age, 1, 2, 3, 1);
        time.attach(&hint(&gettext("A date alone is the whole day, a time alone the time of any day.")), 0, 3, 4, 1);

        let size = gtk::Grid::builder().row_spacing(6).column_spacing(12).build();
        size.attach(&self.size_from_check, 0, 0, 1, 1);
        size.attach(&self.size_from, 1, 0, 1, 1);
        size.attach(&self.size_to_check, 2, 0, 1, 1);
        size.attach(&self.size_to, 3, 0, 1, 1);
        size.attach(&self.size_unit, 4, 0, 1, 1);
        size.attach(&hint(&gettext("Directories have no size, with a size given only files are found.")), 0, 1, 5, 1);

        let attributes = gtk::Grid::builder().row_spacing(6).column_spacing(6).build();
        let label = gtk::Label::builder()
            .label(gettext("Attri_butes"))
            .use_underline(true)
            .mnemonic_widget(&self.attributes)
            .xalign(0.0)
            .build();
        attributes.attach(&label, 0, 0, 1, 1);
        attributes.attach(&self.attributes, 1, 0, 1, 1);
        attributes.attach(&self.attribute_menu(), 2, 0, 1, 1);
        attributes.attach(
            &hint(&gettext("For example d+ directories, r+ux+ executable files, !l+ no links, d+|l+ directories or links, 644+ octal bits.")),
            0,
            1,
            3,
            1,
        );

        let duplicates = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        duplicates.append(&self.duplicates);
        for check in [&self.same_name, &self.same_size, &self.same_hash, &self.same_content] {
            duplicates.append(check);
        }

        self.page.append(&section(&gettext("Modified"), &time));
        self.page.append(&section(&gettext("Size"), &size));
        self.page.append(&section(&gettext("Types and permissions"), &attributes));
        self.page.append(&section(&gettext("Duplicates"), &duplicates));
    }

    /// "Add": every word with what it means; a click writes it as wanted
    /// (`+`) or not wanted (`-`).
    fn attribute_menu(&self) -> gtk::MenuButton {
        let grid = gtk::Grid::builder().row_spacing(2).column_spacing(8).margin_top(4).margin_bottom(4).build();
        let popover = gtk::Popover::builder().child(&grid).build();
        for (row, (word, meaning)) in Attributes::words().into_iter().enumerate() {
            let row = i32::try_from(row).unwrap_or(0);
            grid.attach(&gtk::Label::builder().label(word).xalign(0.0).css_classes(["monospace"]).build(), 0, row, 1, 1);
            grid.attach(&gtk::Label::builder().label(&meaning).xalign(0.0).hexpand(true).build(), 1, row, 1, 1);
            for (column, (sign, tooltip)) in [("+", gettext("must have")), ("-", gettext("must not have"))].into_iter().enumerate() {
                let button = gtk::Button::builder().label(sign).tooltip_text(tooltip).build();
                button.add_css_class("flat");
                let written = format!("{word}{sign}");
                button.connect_clicked(glib::clone!(
                    #[weak(rename_to = entry)]
                    self.attributes,
                    #[weak]
                    popover,
                    move |_| {
                        entry.set_text(&format!("{}{written}", entry.text()));
                        popover.popdown();
                    }
                ));
                grid.attach(&button, 2 + i32::try_from(column).unwrap_or(0), row, 1, 1);
            }
        }
        gtk::MenuButton::builder().label(gettext("_Add")).use_underline(true).popover(&popover).build()
    }

    /// Units after the number in their right form ("1 day", "5 days").
    fn update_age_titles(&self) {
        let count = u32::try_from(self.age.value_as_int()).unwrap_or(1);
        let selected = self.age_unit.selected();
        let titles: Vec<String> = TimeUnit::ALL.iter().map(|unit| unit.title(count)).collect();
        let old = self.age_titles.n_items();
        self.age_titles.splice(0, old, &titles.iter().map(String::as_str).collect::<Vec<_>>());
        self.age_unit.set_selected(if old == 0 { 3 } else { selected });
    }

    /// A changed value checks its box (as the original); `changed` after
    /// every change that enables other fields.
    pub fn connect(self: &std::rc::Rc<Self>) {
        let checks_on = |check: &gtk::CheckButton| {
            let check = check.downgrade();
            move || {
                if let Some(check) = check.upgrade() {
                    check.set_active(true);
                }
            }
        };
        let on = checks_on(&self.date_from_check);
        self.date_from.calendar.connect_day_selected(move |_| on());
        let on = checks_on(&self.date_to_check);
        self.date_to.calendar.connect_day_selected(move |_| on());
        self.time_from.connect_changed(checks_on(&self.time_from_check));
        self.time_to.connect_changed(checks_on(&self.time_to_check));
        let on = checks_on(&self.size_from_check);
        self.size_from.connect_value_changed(move |_| on());
        let on = checks_on(&self.size_to_check);
        self.size_to.connect_value_changed(move |_| on());
        let weak = std::rc::Rc::downgrade(self);
        self.age.connect_value_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.age_check.set_active(true);
                this.update_age_titles();
            }
        });
        for check in [&self.age_check, &self.duplicates] {
            let weak = std::rc::Rc::downgrade(self);
            check.connect_toggled(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.update();
                }
            });
        }
    }

    fn update(&self) {
        let dates = !self.age_check.is_active();
        for widget in [
            self.date_from_check.upcast_ref::<gtk::Widget>(),
            self.date_from.button.upcast_ref(),
            self.time_from_check.upcast_ref(),
            self.time_from.row.upcast_ref(),
            self.date_to_check.upcast_ref(),
            self.date_to.button.upcast_ref(),
            self.time_to_check.upcast_ref(),
            self.time_to.row.upcast_ref(),
        ] {
            widget.set_sensitive(dates);
        }
        let duplicates = self.duplicates.is_active();
        for check in [&self.same_name, &self.same_size, &self.same_hash, &self.same_content] {
            check.set_sensitive(duplicates);
        }
    }

    pub fn fill(&self, query: &mut FindQuery) {
        let when = |check: &gtk::CheckButton| check.is_active();
        query.date_from = when(&self.date_from_check).then(|| self.date_from.date());
        query.time_from = when(&self.time_from_check).then(|| self.time_from.time());
        query.date_to = when(&self.date_to_check).then(|| self.date_to.date());
        query.time_to = when(&self.time_to_check).then(|| self.time_to.time());
        query.age = when(&self.age_check).then(|| Age {
            amount: u32::try_from(self.age.value_as_int()).unwrap_or(1),
            unit: TimeUnit::ALL.get(self.age_unit.selected() as usize).copied().unwrap_or(TimeUnit::Day),
        });
        query.size_from = when(&self.size_from_check).then(|| spin_value(&self.size_from));
        query.size_to = when(&self.size_to_check).then(|| spin_value(&self.size_to));
        query.size_unit = SizeUnit::ALL.get(self.size_unit.selected() as usize).copied().unwrap_or(SizeUnit::Kilo);
        self.attributes.text().trim().clone_into(&mut query.attributes);
        query.duplicates = when(&self.duplicates).then(|| Duplicates {
            name: self.same_name.is_active(),
            size: self.same_size.is_active(),
            hash: self.same_hash.is_active(),
            content: self.same_content.is_active(),
        });
    }

    /// Shows `query`; fields it leaves out show now (dates, times) or the
    /// original's defaults (1 day, 0 to 10 KiB).
    pub fn load(&self, query: &FindQuery) {
        let now = jiff::Zoned::now();
        self.date_from.set_date(query.date_from.unwrap_or(now.date()));
        self.date_to.set_date(query.date_to.unwrap_or(now.date()));
        self.time_from.set_time(query.time_from.unwrap_or(now.time()));
        self.time_to.set_time(query.time_to.unwrap_or(now.time()));
        let age = query.age.unwrap_or(Age { amount: 1, unit: TimeUnit::Day });
        self.age.set_value(f64::from(age.amount));
        let unit = TimeUnit::ALL.iter().position(|unit| *unit == age.unit).unwrap_or(3);
        self.age_unit.set_selected(u32::try_from(unit).unwrap_or(3));
        #[expect(clippy::cast_precision_loss, reason = "sizes typed by hand, far below 2^52")]
        let set = |spin: &gtk::SpinButton, value: u64| spin.set_value(value as f64);
        set(&self.size_from, query.size_from.unwrap_or(0));
        set(&self.size_to, query.size_to.unwrap_or(10));
        let unit = SizeUnit::ALL.iter().position(|unit| *unit == query.size_unit).unwrap_or(1);
        self.size_unit.set_selected(u32::try_from(unit).unwrap_or(1));
        self.attributes.set_text(&query.attributes);
        let duplicates = query.duplicates.unwrap_or(Duplicates { name: true, ..Duplicates::default() });
        self.same_name.set_active(duplicates.name);
        self.same_size.set_active(duplicates.size);
        self.same_hash.set_active(duplicates.hash);
        self.same_content.set_active(duplicates.content);
        // The boxes last: setting the values checks them.
        self.date_from_check.set_active(query.date_from.is_some());
        self.time_from_check.set_active(query.time_from.is_some());
        self.date_to_check.set_active(query.date_to.is_some());
        self.time_to_check.set_active(query.time_to.is_some());
        self.age_check.set_active(query.age.is_some());
        self.size_from_check.set_active(query.size_from.is_some());
        self.size_to_check.set_active(query.size_to.is_some());
        self.duplicates.set_active(query.duplicates.is_some());
        self.update();
    }
}

#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a whole, positive number of the spin button")]
fn spin_value(spin: &gtk::SpinButton) -> u64 {
    spin.value().round() as u64
}

/// A small line explaining the fields above it.
fn hint(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).wrap(true).css_classes(["dim-label", "caption"]).build()
}

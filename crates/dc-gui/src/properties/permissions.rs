//! The "Permissions" page: owner and group, read, write and execute for
//! each of them and the others, the special bits; in octal and as `ls -l`
//! writes them. With several items a check of a bit some of them have is
//! neither on nor off, and stays so unless clicked.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use dc_core::properties::{self, Account, ModeChange};
use dc_i18n::{fill, gettext};
use gtk::prelude::*;

/// The bits in the order of the grid: rows owner, group, others; columns
/// read, write, execute; then setuid, setgid, sticky.
const BITS: [u32; 12] = [0o400, 0o200, 0o100, 0o040, 0o020, 0o010, 0o004, 0o002, 0o001, 0o4000, 0o2000, 0o1000];

/// One bit: its check and what it was / is now (`None`: mixed, unchanged).
struct Bit {
    mask: u32,
    check: gtk::CheckButton,
    initial: Option<bool>,
    state: Cell<Option<bool>>,
}

/// An owner or a group: the list, what it was (`None`: mixed).
struct Choice {
    drop_down: gtk::DropDown,
    accounts: Vec<Account>,
    /// A first entry "(various)" when the items differ.
    mixed: bool,
    initial: u32,
}

impl Choice {
    fn new(accounts: Vec<Account>, ids: &[u32]) -> Self {
        let first = ids.first().copied().unwrap_or(0);
        let mixed = ids.iter().any(|id| *id != first);
        let mut accounts = accounts;
        // An id without a name (a file of a removed user) is offered as a number.
        if !mixed && !accounts.iter().any(|account| account.id == first) {
            accounts.insert(0, Account { id: first, name: first.to_string() });
        }
        let mut titles: Vec<String> = Vec::new();
        if mixed {
            titles.push(gettext("(various)"));
        }
        titles.extend(accounts.iter().map(|account| fill(&gettext("{name} ({id})"), &[("name", &account.name), ("id", &account.id)])));
        let drop_down = gtk::DropDown::from_strings(&titles.iter().map(String::as_str).collect::<Vec<_>>());
        drop_down.set_enable_search(true);
        drop_down.set_expression(Some(gtk::PropertyExpression::new(gtk::StringObject::static_type(), None::<gtk::Expression>, "string")));
        let selected = if mixed { 0 } else { accounts.iter().position(|account| account.id == first).unwrap_or(0) };
        drop_down.set_selected(u32::try_from(selected).unwrap_or(0));
        Self { drop_down, accounts, mixed, initial: first }
    }

    /// The account chosen, when it is another than before.
    fn changed(&self) -> Option<u32> {
        let index = self.drop_down.selected() as usize;
        let index = if self.mixed { index.checked_sub(1)? } else { index };
        let id = self.accounts.get(index)?.id;
        (self.mixed || id != self.initial).then_some(id)
    }
}

pub(super) struct Permissions {
    pub page: gtk::Box,
    bits: Vec<Rc<Bit>>,
    owner: Choice,
    group: Choice,
    octal: gtk::Entry,
    symbolic: gtk::Label,
    /// Execute only where present (`chmod X`), for the contents of directories.
    pub execute_where_present: gtk::CheckButton,
    /// The type of the first item, for the `ls -l` text.
    type_char: char,
    updating: Cell<bool>,
    changed: RefCell<Option<Box<dyn Fn()>>>,
}

impl Permissions {
    /// The page for items with `modes`, owned by `uids` and `gids`.
    pub fn new(modes: &[u32], uids: &[u32], gids: &[u32], any_dir: bool) -> Rc<Self> {
        let bits = BITS
            .iter()
            .map(|&bit| {
                let initial = properties::bit_state(modes, bit);
                let check = gtk::CheckButton::builder().active(initial == Some(true)).inconsistent(initial.is_none()).build();
                Rc::new(Bit { mask: bit, check, initial, state: Cell::new(initial) })
            })
            .collect();
        let type_char = modes.first().map_or('-', |mode| format_type(*mode));
        let page = Rc::new(Self {
            page: gtk::Box::new(gtk::Orientation::Vertical, 14),
            bits,
            owner: Choice::new(properties::accounts("passwd"), uids),
            group: Choice::new(properties::accounts("group"), gids),
            octal: gtk::Entry::builder().width_chars(6).max_width_chars(6).build(),
            symbolic: gtk::Label::builder().xalign(0.0).selectable(true).css_classes(["monospace"]).build(),
            execute_where_present: gtk::CheckButton::builder()
                .label(gettext("In the contents of directories execute only where it is already (as chmod X)"))
                .active(true)
                .visible(any_dir)
                .build(),
            type_char,
            updating: Cell::new(false),
            changed: RefCell::default(),
        });
        page.lay_out(any_dir);
        page.connect();
        page.show_mode();
        page
    }

    fn lay_out(&self, any_dir: bool) {
        let accounts = gtk::Grid::builder().row_spacing(6).column_spacing(12).build();
        for (row, (title, choice)) in [(gettext("_Owner:"), &self.owner), (gettext("_Group:"), &self.group)].into_iter().enumerate() {
            let row = i32::try_from(row).unwrap_or(0);
            let label = gtk::Label::builder().label(title).use_underline(true).mnemonic_widget(&choice.drop_down).xalign(0.0).build();
            accounts.attach(&label, 0, row, 1, 1);
            choice.drop_down.set_hexpand(true);
            accounts.attach(&choice.drop_down, 1, row, 1, 1);
        }
        self.page.append(&accounts);

        let grid = gtk::Grid::builder().row_spacing(6).column_spacing(18).build();
        let execute = if any_dir { gettext("Execute / enter") } else { gettext("Execute") };
        for (column, title) in [gettext("Read"), gettext("Write"), execute].into_iter().enumerate() {
            let label = gtk::Label::builder().label(title).css_classes(["heading"]).build();
            grid.attach(&label, i32::try_from(column).unwrap_or(0) + 1, 0, 1, 1);
        }
        for (row, title) in [gettext("Owner"), gettext("Group"), gettext("Others")].into_iter().enumerate() {
            let row = i32::try_from(row).unwrap_or(0) + 1;
            grid.attach(&gtk::Label::builder().label(title).xalign(0.0).build(), 0, row, 1, 1);
            for column in 0..3 {
                let index = usize::try_from((row - 1) * 3 + column).unwrap_or(0);
                let check = &self.bits[index].check;
                check.set_halign(gtk::Align::Center);
                grid.attach(check, column + 1, row, 1, 1);
            }
        }
        self.page.append(&grid);

        let special = gtk::Box::new(gtk::Orientation::Horizontal, 18);
        for (index, (title, tooltip)) in [
            ("Set UID", gettext("A program runs with the rights of its owner")),
            ("Set GID", gettext("A program runs with the rights of its group; in a directory new files get its group")),
            ("Sticky", gettext("In a directory only the owner of a file may delete or rename it (as /tmp)")),
        ]
        .into_iter()
        .enumerate()
        {
            let check = &self.bits[9 + index].check;
            check.set_label(Some(title));
            check.set_tooltip_text(Some(&tooltip));
            special.append(check);
        }
        self.page.append(&special);

        let written = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let octal_label =
            gtk::Label::builder().label(gettext("O_ctal:")).use_underline(true).mnemonic_widget(&self.octal).build();
        written.append(&octal_label);
        written.append(&self.octal);
        written.append(&self.symbolic);
        self.page.append(&written);
        self.page.append(&self.execute_where_present);
    }

    fn connect(self: &Rc<Self>) {
        for bit in &self.bits {
            let (weak, check, bit) = (Rc::downgrade(self), bit.check.clone(), Rc::clone(bit));
            check.connect_toggled(move |check| {
                let Some(this) = weak.upgrade() else { return };
                if this.updating.get() {
                    return;
                }
                check.set_inconsistent(false);
                bit.state.set(Some(check.is_active()));
                this.show_mode();
                this.notify();
            });
        }
        let weak = Rc::downgrade(self);
        self.octal.connect_changed(move |entry| {
            let Some(this) = weak.upgrade() else { return };
            if this.updating.get() {
                return;
            }
            let Some(mode) = properties::parse_octal(&entry.text()) else {
                entry.add_css_class("error");
                return;
            };
            entry.remove_css_class("error");
            this.updating.set(true);
            for bit in &this.bits {
                let on = mode & bit.mask != 0;
                bit.state.set(Some(on));
                bit.check.set_inconsistent(false);
                bit.check.set_active(on);
            }
            this.updating.set(false);
            this.symbolic.set_text(&this.symbolic_text());
            this.notify();
        });
        for choice in [&self.owner, &self.group] {
            let weak = Rc::downgrade(self);
            choice.drop_down.connect_selected_notify(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.notify();
                }
            });
        }
    }

    pub fn connect_changed(&self, changed: impl Fn() + 'static) {
        *self.changed.borrow_mut() = Some(Box::new(changed));
    }

    fn notify(&self) {
        if let Some(changed) = &*self.changed.borrow() {
            changed();
        }
    }

    /// The bits as now, `None` when some are mixed.
    fn mode(&self) -> Option<u32> {
        self.bits.iter().try_fold(0, |mode, bit| bit.state.get().map(|on| if on { mode | bit.mask } else { mode }))
    }

    fn symbolic_text(&self) -> String {
        let mut text = String::from(self.type_char);
        for (index, bit) in self.bits.iter().take(9).enumerate() {
            let letter = ['r', 'w', 'x'][index % 3];
            let special = match index {
                2 => Some((9, 's')),
                5 => Some((10, 's')),
                8 => Some((11, 't')),
                _ => None,
            };
            let on = |bit: &Bit| bit.state.get();
            let ch = match (on(bit), special.map(|(index, letter)| (on(&self.bits[index]), letter))) {
                (None, _) | (_, Some((None, _))) => '?',
                (Some(true), Some((Some(true), letter))) => letter,
                (Some(false), Some((Some(true), letter))) => letter.to_ascii_uppercase(),
                (Some(true), _) => letter,
                (Some(false), _) => '-',
            };
            text.push(ch);
        }
        text
    }

    /// The octal field and the `ls -l` text after a change of the checks.
    fn show_mode(&self) {
        self.updating.set(true);
        if let Some(mode) = self.mode() {
            self.octal.set_text(&properties::octal(mode));
            self.octal.set_placeholder_text(None);
        } else {
            self.octal.set_text("");
            self.octal.set_placeholder_text(Some(&gettext("various")));
        }
        self.octal.remove_css_class("error");
        self.updating.set(false);
        self.symbolic.set_text(&self.symbolic_text());
    }

    /// What changes: the bits that differ from before, the owner and group
    /// chosen anew.
    pub fn changes(&self) -> (ModeChange, Option<u32>, Option<u32>) {
        let mut change = ModeChange { execute_only_where_present: self.execute_where_present.is_active(), ..ModeChange::default() };
        for bit in &self.bits {
            match bit.state.get() {
                Some(true) if bit.initial != Some(true) => change.set |= bit.mask,
                Some(false) if bit.initial != Some(false) => change.clear |= bit.mask,
                _ => {}
            }
        }
        (change, self.owner.changed(), self.group.changed())
    }
}

/// The first letter of `ls -l` for the type in `mode`.
fn format_type(mode: u32) -> char {
    match mode & 0o170_000 {
        0o040_000 => 'd',
        0o120_000 => 'l',
        0o020_000 => 'c',
        0o060_000 => 'b',
        0o010_000 => 'p',
        0o140_000 => 's',
        _ => '-',
    }
}

//! Keyboard shortcuts and the keymap that turns them into commands.
//!
//! Shortcuts use the text form of the original (`Ctrl+PgUp`, `Shift+F8`,
//! `Num+`, `BkSp`), see `platform/ukeyboard.pas`, so its `shortcuts.scf`
//! can be read later. Modifiers are written in the original's Linux order:
//! `Ctrl`, `Super`, `Shift`, `Alt`. The original calls the Super key
//! `WinKey`, that spelling is accepted when parsing but never written.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use dc_i18n::{fill, gettext};

use crate::command::{Command, Direction, RangeStep, TrashChoice};
use crate::sort::SortColumn;

/// A key without modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// Letter, digit or symbol. Letters are kept in upper case.
    Char(char),
    /// Function key F1 to F24.
    F(u8),
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Insert,
    Delete,
    Enter,
    Tab,
    Backspace,
    Space,
    Escape,
    NumPlus,
    NumMinus,
    NumMultiply,
    NumDivide,
}

/// Named keys and their text form.
const KEY_NAMES: [(Key, &str); 19] = [
    (Key::Up, "Up"),
    (Key::Down, "Down"),
    (Key::Left, "Left"),
    (Key::Right, "Right"),
    (Key::PageUp, "PgUp"),
    (Key::PageDown, "PgDn"),
    (Key::Home, "Home"),
    (Key::End, "End"),
    (Key::Insert, "Ins"),
    (Key::Delete, "Del"),
    (Key::Enter, "Enter"),
    (Key::Tab, "Tab"),
    (Key::Backspace, "BkSp"),
    (Key::Space, "Space"),
    (Key::Escape, "Esc"),
    (Key::NumPlus, "Num+"),
    (Key::NumMinus, "Num-"),
    (Key::NumMultiply, "Num*"),
    (Key::NumDivide, "Num/"),
];

impl Key {
    /// A character key, letters normalised to upper case.
    #[must_use]
    pub fn char(ch: char) -> Self {
        Self::Char(ch.to_uppercase().next().unwrap_or(ch))
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Char(ch) => write!(f, "{ch}"),
            Self::F(n) => write!(f, "F{n}"),
            named => {
                let (_, name) = KEY_NAMES.iter().find(|(key, _)| key == named).expect("every named key has a name");
                f.write_str(name)
            }
        }
    }
}

impl FromStr for Key {
    type Err = ParseShortcutError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((key, _)) = KEY_NAMES.iter().find(|(_, name)| name.eq_ignore_ascii_case(s)) {
            return Ok(*key);
        }
        if let Some(n) = s.strip_prefix(['F', 'f']).and_then(|n| n.parse::<u8>().ok())
            && (1..=24).contains(&n)
        {
            return Ok(Self::F(n));
        }
        let mut chars = s.chars();
        match (chars.next(), chars.next()) {
            (Some(ch), None) => Ok(Self::char(ch)),
            _ => Err(ParseShortcutError(s.to_owned())),
        }
    }
}

/// Modifier keys held with a shortcut. Lock keys (Caps, Num) never count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[expect(clippy::struct_excessive_bools, reason = "four independent keys, held or not; a bit set would only hide that")]
pub struct Modifiers {
    pub ctrl: bool,
    pub super_key: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Modifiers {
    pub const NONE: Self = Self { ctrl: false, super_key: false, shift: false, alt: false };
    pub const SHIFT: Self = Self { shift: true, ..Self::NONE };
    pub const CTRL: Self = Self { ctrl: true, ..Self::NONE };

    #[must_use]
    pub fn is_empty(self) -> bool {
        self == Self::NONE
    }
}

/// A key with modifiers, e.g. `Ctrl+PgUp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl Shortcut {
    #[must_use]
    pub const fn new(modifiers: Modifiers, key: Key) -> Self {
        Self { modifiers, key }
    }

    #[must_use]
    pub const fn plain(key: Key) -> Self {
        Self::new(Modifiers::NONE, key)
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.modifiers;
        for (held, text) in [(m.ctrl, "Ctrl+"), (m.super_key, "Super+"), (m.shift, "Shift+"), (m.alt, "Alt+")] {
            if held {
                f.write_str(text)?;
            }
        }
        write!(f, "{}", self.key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseShortcutError(pub String);

impl fmt::Display for ParseShortcutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&fill(&gettext("Invalid shortcut {shortcut}"), &[("shortcut", &self.0)]))
    }
}

impl std::error::Error for ParseShortcutError {}

impl FromStr for Shortcut {
    type Err = ParseShortcutError;

    /// Accepts modifiers in any order. The key itself may contain `+`
    /// (`Num+`, `Ctrl+Num+`), so modifiers are taken from the front.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut modifiers = Modifiers::NONE;
        let mut rest = s.trim();
        while let Some((prefix, tail)) = rest.split_once('+') {
            let flag = match prefix.to_ascii_lowercase().as_str() {
                "ctrl" => &mut modifiers.ctrl,
                "shift" => &mut modifiers.shift,
                "alt" => &mut modifiers.alt,
                "super" | "winkey" | "meta" => &mut modifiers.super_key,
                _ => break,
            };
            *flag = true;
            rest = tail;
        }
        let key = rest.parse().map_err(|_| ParseShortcutError(s.to_owned()))?;
        Ok(Self { modifiers, key })
    }
}

/// Shortcut to command table.
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    bindings: HashMap<Shortcut, Command>,
}

impl Keymap {
    /// Binds `shortcut` to `command`, replacing an earlier binding of it.
    pub fn bind(&mut self, shortcut: Shortcut, command: Command) {
        self.bindings.insert(shortcut, command);
    }

    #[must_use]
    pub fn command(&self, shortcut: &Shortcut) -> Option<Command> {
        self.bindings.get(shortcut).copied()
    }

    /// Shortcuts bound to `command`, sorted by their text for stable output.
    #[must_use]
    pub fn shortcuts(&self, command: Command) -> Vec<Shortcut> {
        let mut found: Vec<_> =
            self.bindings.iter().filter(|(_, bound)| **bound == command).map(|(shortcut, _)| *shortcut).collect();
        found.sort_by_key(ToString::to_string);
        found
    }

    /// Default bindings: those of the original for commands it has
    /// (`LoadDefaultHotkeyBindings` in `uglobs.pas`) plus the keys it
    /// handles directly in the file view.
    #[must_use]
    pub fn with_defaults() -> Self {
        let shift = |key| Shortcut::new(Modifiers::SHIFT, key);
        let ctrl = |key| Shortcut::new(Modifiers::CTRL, key);
        let alt = |key| Shortcut::new(Modifiers { alt: true, ..Modifiers::NONE }, key);
        let defaults = [
            (Shortcut::plain(Key::Enter), Command::Open),
            (Shortcut::plain(Key::Backspace), Command::ChangeDirToParent),
            (Shortcut::new(Modifiers::CTRL, Key::PageUp), Command::ChangeDirToParent),
            // Deliberately unlike the original (history back and forward there):
            // Alt+Left up as Backspace, Alt+Right into a directory as Enter.
            (alt(Key::Left), Command::ChangeDirToParent),
            (alt(Key::Right), Command::EnterDir),
            (Shortcut::plain(Key::Tab), Command::FocusSwap),
            // Deliberately unlike the original (`cm_FocusTreeView` there):
            // Shift+Tab is "back" where Tab is "forward", and with two panels
            // both mean the other one. Keep it even once there is a tree view.
            (shift(Key::Tab), Command::FocusSwap),
            (Shortcut::plain(Key::Space), Command::ToggleMark),
            (Shortcut::plain(Key::Insert), Command::ToggleMarkMoveDown),
            (shift(Key::Up), Command::MarkMove(Direction::Up)),
            (shift(Key::Down), Command::MarkMove(Direction::Down)),
            (shift(Key::Left), Command::MarkMove(Direction::Left)),
            (shift(Key::Right), Command::MarkMove(Direction::Right)),
            (shift(Key::PageUp), Command::MarkRange(RangeStep::PageUp)),
            (shift(Key::PageDown), Command::MarkRange(RangeStep::PageDown)),
            (shift(Key::Home), Command::MarkRange(RangeStep::First)),
            (shift(Key::End), Command::MarkRange(RangeStep::Last)),
            (Shortcut::plain(Key::F(3)), Command::View),
            (Shortcut::plain(Key::F(4)), Command::Edit),
            (Shortcut::plain(Key::F(5)), Command::Copy),
            (shift(Key::F(5)), Command::CopySamePanel),
            (Shortcut::plain(Key::F(6)), Command::Rename),
            (Shortcut::plain(Key::F(7)), Command::MakeDir),
            (Shortcut::plain(Key::F(9)), Command::RunTerm),
            (Shortcut::plain(Key::F(8)), Command::Delete { trash: TrashChoice::Setting }),
            (Shortcut::plain(Key::Delete), Command::Delete { trash: TrashChoice::Setting }),
            (shift(Key::F(8)), Command::Delete { trash: TrashChoice::ReverseSetting }),
            (shift(Key::Delete), Command::Delete { trash: TrashChoice::ReverseSetting }),
            (Shortcut::new(Modifiers { alt: true, ..Modifiers::NONE }, Key::Char('X')), Command::Exit),
            (ctrl(Key::Char('R')), Command::Refresh),
            (ctrl(Key::F(1)), Command::BriefView),
            (ctrl(Key::F(2)), Command::ColumnsView),
            (ctrl(Key::F(3)), Command::SortBy(SortColumn::Name)),
            (ctrl(Key::F(4)), Command::SortBy(SortColumn::Extension)),
            (ctrl(Key::F(5)), Command::SortBy(SortColumn::Modified)),
            (ctrl(Key::F(6)), Command::SortBy(SortColumn::Size)),
            (Shortcut::new(Modifiers { ctrl: true, shift: true, ..Modifiers::NONE }, Key::F(1)), Command::ThumbnailsView),
            (ctrl(Key::Char('A')), Command::MarkMarkAll),
            (ctrl(Key::NumPlus), Command::MarkMarkAll),
            (ctrl(Key::NumMinus), Command::MarkUnmarkAll),
            (Shortcut::plain(Key::NumMultiply), Command::MarkInvert),
            (ctrl(Key::Char('\\')), Command::ChangeDirToRoot),
            (Shortcut::new(Modifiers { ctrl: true, shift: true, ..Modifiers::NONE }, Key::Home), Command::ChangeDirToHome),
        ];
        let mut keymap = Self::default();
        for (shortcut, command) in defaults {
            keymap.bind(shortcut, command);
        }
        keymap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Shortcut {
        s.parse().unwrap()
    }

    #[test]
    fn parses_texts_of_the_original() {
        assert_eq!(parse("Ctrl+PgUp"), Shortcut::new(Modifiers::CTRL, Key::PageUp));
        assert_eq!(parse("Shift+F8"), Shortcut::new(Modifiers::SHIFT, Key::F(8)));
        assert_eq!(parse("Num+"), Shortcut::plain(Key::NumPlus));
        assert_eq!(parse("Ctrl+Num+"), Shortcut::new(Modifiers::CTRL, Key::NumPlus));
        assert_eq!(parse("BkSp"), Shortcut::plain(Key::Backspace));
        assert_eq!(parse("Ctrl+a"), Shortcut::new(Modifiers::CTRL, Key::Char('A')));
        assert_eq!(parse("Ctrl+\\"), Shortcut::new(Modifiers::CTRL, Key::Char('\\')));
        assert_eq!(parse("Ctrl++"), Shortcut::new(Modifiers::CTRL, Key::Char('+')));
        assert!("Ctrl+Nonsense".parse::<Shortcut>().is_err());
        assert!("F25".parse::<Shortcut>().is_err());
    }

    #[test]
    fn modifiers_in_any_order_print_in_the_original_order() {
        let shortcut = parse("Alt+Shift+Ctrl+Enter");
        assert_eq!(shortcut.to_string(), "Ctrl+Shift+Alt+Enter");
        assert_eq!(parse("Super+Tab").to_string(), "Super+Tab");
        // Spelling of the original's shortcut files.
        assert_eq!(parse("WinKey+Tab").to_string(), "Super+Tab");
    }

    #[test]
    fn display_round_trips() {
        for text in ["Ctrl+PgUp", "Shift+Num-", "F12", "Ins", "Ctrl+Shift+A", "Alt+Del", "Num*"] {
            assert_eq!(parse(text).to_string(), text);
        }
    }

    #[test]
    fn defaults_cover_every_command() {
        let keymap = Keymap::with_defaults();
        // Like in the original, these are reached by buttons only.
        let without_shortcut = [
            Command::LeftEqualRight,
            Command::RightEqualLeft,
            Command::PanelsSplitterPerPos { percent: 50 },
            // Menu only, as in the original.
            Command::SortBy(SortColumn::Attributes),
            Command::ReverseOrder,
        ];
        for command in Command::ALL.into_iter().filter(|command| !without_shortcut.contains(command)) {
            assert!(!keymap.shortcuts(command).is_empty(), "{command} has no default shortcut");
        }
        assert_eq!(keymap.command(&parse("Num*")), Some(Command::MarkInvert));
        assert_eq!(keymap.command(&parse("Ctrl+Shift+Home")), Some(Command::ChangeDirToHome));
        assert_eq!(keymap.command(&parse("Ctrl+PgUp")), Some(Command::ChangeDirToParent));
        assert_eq!(keymap.command(&parse("Shift+PgDn")), Some(Command::MarkRange(RangeStep::PageDown)));
        assert_eq!(keymap.command(&parse("PgDn")), None);
        let parent: Vec<_> = keymap.shortcuts(Command::ChangeDirToParent).iter().map(ToString::to_string).collect();
        assert_eq!(parent, ["Alt+Left", "BkSp", "Ctrl+PgUp"]);
        assert_eq!(keymap.command(&parse("Alt+Right")), Some(Command::EnterDir));
        assert_eq!(
            keymap.command(&parse("Shift+F8")),
            Some(Command::Delete { trash: TrashChoice::ReverseSetting })
        );
        assert_eq!(keymap.command(&parse("Alt+X")), Some(Command::Exit));
    }
}

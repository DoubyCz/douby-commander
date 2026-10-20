//! Commands: everything the user can trigger by a key, and later by a
//! button, a menu item or the command line.
//!
//! Commands that exist in the original keep its `cm_*` name, so shortcut
//! files and habits carry over. The original handles a few keys directly in
//! the file view (Insert, Space, Shift+arrows, Shift+PgUp/PgDn/Home/End)
//! instead of through commands; here they are commands too, with names of
//! our own, so every key goes through one keymap and can be rebound.

use std::fmt;
use std::str::FromStr;

use dc_i18n::{fill, gettext};

use crate::sort::SortColumn;

/// Arrow directions, as on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// Cursor jumps that extend a range selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RangeStep {
    PageUp,
    PageDown,
    First,
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    /// Enter: open the directory or the file under the cursor.
    Open,
    ChangeDirToParent,
    /// Alt+Right: into the directory under the cursor, as Enter; on `..` and
    /// on files nothing. Our own command (the original has history there).
    EnterDir,
    /// Tab: make the other panel active.
    FocusSwap,
    /// Space: toggle the mark of the item under the cursor.
    ToggleMark,
    /// Insert: toggle the mark and move to the next item.
    ToggleMarkMoveDown,
    /// Shift+arrows: mark from the cursor and move there, by an item, or
    /// in the list and icons views across a column or row.
    MarkMove(Direction),
    /// Shift+PgUp/PgDn/Home/End: mark a range from the starting point.
    MarkRange(RangeStep),
    /// F3: open the file under the cursor in the terminal text editor
    /// (`$VISUAL`, `$EDITOR`); the original has its own viewer here.
    View,
    /// F4: open the file under the cursor in the desktop's text editor
    /// (default application for `text/plain`), whatever its type.
    Edit,
    /// F5: copy marked items, or the one under the cursor, to the other panel.
    Copy,
    /// Shift+F5: copy within the active panel, under another name.
    CopySamePanel,
    /// F6: move or rename (`cm_Rename` in the original).
    Rename,
    /// Shift+F6, F2: rename the item under the cursor in place.
    RenameOnly,
    /// F7: create a directory in the active panel.
    MakeDir,
    /// F9: the terminal in the directory of the active panel.
    RunTerm,
    /// F8, Del: delete marked items, or the one under the cursor.
    Delete { trash: TrashChoice },
    /// Alt+X: close the application.
    Exit,
    /// Ctrl+R: list the active panel again.
    Refresh,
    /// Ctrl+A: mark everything except `..`, directories too.
    MarkMarkAll,
    MarkUnmarkAll,
    MarkInvert,
    ChangeDirToRoot,
    ChangeDirToHome,
    /// Ctrl+F1: the active panel shows names only, in columns ("List").
    BriefView,
    /// Ctrl+F2: the active panel shows all columns ("Details").
    ColumnsView,
    /// Ctrl+Shift+F1: the active panel shows icons and previews ("Icons").
    ThumbnailsView,
    /// Ctrl+F3 to Ctrl+F6: sort the active panel, as a click on the column
    /// header; again the same reverses.
    SortBy(SortColumn),
    /// Reverse the sort direction of the active panel.
    ReverseOrder,
    /// Left panel shows the directory of the right one (`<` above the left panel).
    LeftEqualRight,
    /// Right panel shows the directory of the left one (`>` above the right panel).
    RightEqualLeft,
    /// Divider between the panels to `percent` of the width (double click: 50).
    PanelsSplitterPerPos { percent: u8 },
}

/// Parameter `trashcan` of `cm_Delete`: whether to use the trash follows
/// the "use trash" option, or the opposite of it (Shift+F8, Shift+Del).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum TrashChoice {
    #[default]
    Setting,
    ReverseSetting,
}

impl TrashChoice {
    /// Whether to delete into the trash, given the "use trash" option.
    #[must_use]
    pub fn use_trash(self, use_trash_option: bool) -> bool {
        match self {
            Self::Setting => use_trash_option,
            Self::ReverseSetting => !use_trash_option,
        }
    }
}

impl Command {
    /// All commands with default parameters, for listings and for parsing.
    pub const ALL: [Self; 42] = [
        Self::Open,
        Self::ChangeDirToParent,
        Self::EnterDir,
        Self::FocusSwap,
        Self::ToggleMark,
        Self::ToggleMarkMoveDown,
        Self::MarkMove(Direction::Up),
        Self::MarkMove(Direction::Down),
        Self::MarkMove(Direction::Left),
        Self::MarkMove(Direction::Right),
        Self::MarkRange(RangeStep::PageUp),
        Self::MarkRange(RangeStep::PageDown),
        Self::MarkRange(RangeStep::First),
        Self::MarkRange(RangeStep::Last),
        Self::View,
        Self::Edit,
        Self::Copy,
        Self::CopySamePanel,
        Self::Rename,
        Self::RenameOnly,
        Self::MakeDir,
        Self::RunTerm,
        Self::Delete { trash: TrashChoice::Setting },
        Self::Exit,
        Self::Refresh,
        Self::MarkMarkAll,
        Self::MarkUnmarkAll,
        Self::MarkInvert,
        Self::ChangeDirToRoot,
        Self::ChangeDirToHome,
        Self::BriefView,
        Self::ColumnsView,
        Self::ThumbnailsView,
        Self::SortBy(SortColumn::Name),
        Self::SortBy(SortColumn::Extension),
        Self::SortBy(SortColumn::Size),
        Self::SortBy(SortColumn::Modified),
        Self::SortBy(SortColumn::Attributes),
        Self::ReverseOrder,
        Self::LeftEqualRight,
        Self::RightEqualLeft,
        Self::PanelsSplitterPerPos { percent: 50 },
    ];

    /// Name as used in shortcut files and on the command line, without parameters.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Open => "cm_Open",
            Self::ChangeDirToParent => "cm_ChangeDirToParent",
            Self::EnterDir => "cm_EnterDir",
            Self::FocusSwap => "cm_FocusSwap",
            Self::ToggleMark => "cm_ToggleMark",
            Self::ToggleMarkMoveDown => "cm_ToggleMarkMoveDown",
            Self::MarkMove(Direction::Up) => "cm_MarkMoveUp",
            Self::MarkMove(Direction::Down) => "cm_MarkMoveDown",
            Self::MarkMove(Direction::Left) => "cm_MarkMoveLeft",
            Self::MarkMove(Direction::Right) => "cm_MarkMoveRight",
            Self::MarkRange(RangeStep::PageUp) => "cm_MarkRangePageUp",
            Self::MarkRange(RangeStep::PageDown) => "cm_MarkRangePageDown",
            Self::MarkRange(RangeStep::First) => "cm_MarkRangeToFirst",
            Self::MarkRange(RangeStep::Last) => "cm_MarkRangeToLast",
            Self::View => "cm_View",
            Self::Edit => "cm_Edit",
            Self::Copy => "cm_Copy",
            Self::CopySamePanel => "cm_CopySamePanel",
            Self::Rename => "cm_Rename",
            Self::RenameOnly => "cm_RenameOnly",
            Self::MakeDir => "cm_MakeDir",
            Self::RunTerm => "cm_RunTerm",
            Self::Delete { .. } => "cm_Delete",
            Self::Exit => "cm_Exit",
            Self::Refresh => "cm_Refresh",
            Self::MarkMarkAll => "cm_MarkMarkAll",
            Self::MarkUnmarkAll => "cm_MarkUnmarkAll",
            Self::MarkInvert => "cm_MarkInvert",
            Self::ChangeDirToRoot => "cm_ChangeDirToRoot",
            Self::ChangeDirToHome => "cm_ChangeDirToHome",
            Self::BriefView => "cm_BriefView",
            Self::ColumnsView => "cm_ColumnsView",
            Self::ThumbnailsView => "cm_ThumbnailsView",
            Self::SortBy(SortColumn::Name) => "cm_SortByName",
            Self::SortBy(SortColumn::Extension) => "cm_SortByExt",
            Self::SortBy(SortColumn::Size) => "cm_SortBySize",
            Self::SortBy(SortColumn::Modified) => "cm_SortByDate",
            Self::SortBy(SortColumn::Attributes) => "cm_SortByAttr",
            Self::ReverseOrder => "cm_ReverseOrder",
            Self::LeftEqualRight => "cm_LeftEqualRight",
            Self::RightEqualLeft => "cm_RightEqualLeft",
            Self::PanelsSplitterPerPos { .. } => "cm_PanelsSplitterPerPos",
        }
    }

    /// Parameters that differ from the defaults, in the `key=value` form of
    /// the original.
    #[must_use]
    pub fn params(self) -> Vec<String> {
        match self {
            Self::Delete { trash: TrashChoice::ReverseSetting } => vec!["trashcan=reversesetting".to_owned()],
            Self::PanelsSplitterPerPos { percent } if percent != 50 => vec![format!("splitpct={percent}")],
            _ => Vec::new(),
        }
    }

    fn with_param(self, param: &str) -> Option<Self> {
        let (key, value) = param.split_once('=')?;
        match (self, key.to_ascii_lowercase().as_str(), value.to_ascii_lowercase().as_str()) {
            (Self::Delete { .. }, "trashcan", "setting") => Some(Self::Delete { trash: TrashChoice::Setting }),
            (Self::Delete { .. }, "trashcan", "reversesetting") => {
                Some(Self::Delete { trash: TrashChoice::ReverseSetting })
            }
            (Self::PanelsSplitterPerPos { .. }, "splitpct", value) => {
                let percent = value.parse::<u8>().ok().filter(|percent| *percent <= 100)?;
                Some(Self::PanelsSplitterPerPos { percent })
            }
            _ => None,
        }
    }
}

/// `cm_Delete trashcan=reversesetting`
impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())?;
        for param in self.params() {
            write!(f, " {param}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseCommandError {
    UnknownCommand(String),
    UnknownParam { command: &'static str, param: String },
}

impl fmt::Display for ParseCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::UnknownCommand(name) => fill(&gettext("Unknown command {command}"), &[("command", name)]),
            Self::UnknownParam { command, param } => fill(
                &gettext("Command {command} does not accept parameter {param}"),
                &[("command", command), ("param", param)],
            ),
        };
        f.write_str(&text)
    }
}

impl std::error::Error for ParseCommandError {}

impl FromStr for Command {
    type Err = ParseCommandError;

    /// A name followed by `key=value` parameters. Names and parameters are
    /// case insensitive, as in the original.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split_whitespace();
        let name = parts.next().unwrap_or_default();
        let base = Self::ALL
            .into_iter()
            .find(|command| command.name().eq_ignore_ascii_case(name))
            .ok_or_else(|| ParseCommandError::UnknownCommand(name.to_owned()))?;
        parts.try_fold(base, |command, param| {
            command.with_param(param).ok_or_else(|| ParseCommandError::UnknownParam {
                command: base.name(),
                param: param.to_owned(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn names_are_unique_and_text_round_trips() {
        let names: HashSet<_> = Command::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), Command::ALL.len());

        let reverse = Command::Delete { trash: TrashChoice::ReverseSetting };
        for command in Command::ALL.into_iter().chain([reverse]) {
            assert_eq!(command.to_string().parse::<Command>(), Ok(command));
        }
        assert_eq!(reverse.to_string(), "cm_Delete trashcan=reversesetting");

        let split = Command::PanelsSplitterPerPos { percent: 30 };
        assert_eq!(split.to_string(), "cm_PanelsSplitterPerPos splitpct=30");
        assert_eq!(split.to_string().parse::<Command>(), Ok(split));
        assert!("cm_PanelsSplitterPerPos splitpct=150".parse::<Command>().is_err());
    }

    #[test]
    fn parsing_ignores_case_and_rejects_unknown() {
        assert_eq!("CM_OPEN".parse::<Command>(), Ok(Command::Open));
        assert_eq!(
            "cm_delete TrashCan=ReverseSetting".parse::<Command>(),
            Ok(Command::Delete { trash: TrashChoice::ReverseSetting })
        );
        assert_eq!("cm_Nonsense".parse::<Command>(), Err(ParseCommandError::UnknownCommand("cm_Nonsense".into())));
        assert_eq!(
            "cm_Open trashcan=setting".parse::<Command>(),
            Err(ParseCommandError::UnknownParam { command: "cm_Open", param: "trashcan=setting".into() })
        );
    }

    #[test]
    fn trash_choice_follows_or_reverses_the_option() {
        assert!(TrashChoice::Setting.use_trash(true));
        assert!(!TrashChoice::ReverseSetting.use_trash(true));
        assert!(TrashChoice::ReverseSetting.use_trash(false));
    }
}

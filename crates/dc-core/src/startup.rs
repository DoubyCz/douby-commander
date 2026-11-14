//! The command line of the application: which panel shows which path, and
//! which panel is active. As the original (`ucmdlineparams.pas`):
//!
//! - `doubycmd PATH`: the active panel shows `PATH`,
//! - `doubycmd LEFT RIGHT`: both panels,
//! - `-L`/`--left PATH`, `-R`/`--right PATH`: one panel,
//! - `-P`/`--active-panel L|R`: which panel is active.
//!
//! A file shows its directory with the cursor on it, a path that does not
//! exist the deepest directory on the way that does (as the original, no
//! questions at start).

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use dc_i18n::{fill, gettext};

use crate::path_input::{Destination, destination};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    /// Index of the panel, 0 left, 1 right.
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Right => 1,
        }
    }
}

/// What the command line asks for; `T` is a path as the caller has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Startup<T> {
    pub left: Option<T>,
    pub right: Option<T>,
    pub active: Option<Side>,
    /// For the active panel (the one given, else the one active already),
    /// after `left` and `right`.
    pub active_path: Option<T>,
}

impl<T> Startup<T> {
    /// Nothing asked: the panels stay as they are (or start at home).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.left.is_none() && self.right.is_none() && self.active.is_none() && self.active_path.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupError {
    /// `-P` with something else than `L` or `R`.
    ActivePanel(String),
    /// More than two paths without an option.
    TooManyPaths(usize),
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::ActivePanel(value) => fill(
                &gettext("Active panel must be L (left) or R (right), not {value}."),
                &[("value", value)],
            ),
            Self::TooManyPaths(count) => fill(
                &gettext("At most two paths (left and right panel), not {count}."),
                &[("count", count)],
            ),
        };
        f.write_str(&text)
    }
}

impl std::error::Error for StartupError {}

/// Puts the parts of a command line together: `-L`, `-R`, `-P` as given
/// (`L`/`R` in any case) and `paths`, the arguments that are no options.
///
/// # Errors
/// An unknown active panel, more than two paths.
pub fn arrange<T>(
    left: Option<T>,
    right: Option<T>,
    active: Option<&str>,
    paths: Vec<T>,
) -> Result<Startup<T>, StartupError> {
    let active = match active.map(str::trim) {
        None => None,
        Some(value) if value.eq_ignore_ascii_case("l") => Some(Side::Left),
        Some(value) if value.eq_ignore_ascii_case("r") => Some(Side::Right),
        Some(value) => return Err(StartupError::ActivePanel(value.to_owned())),
    };
    let count = paths.len();
    let mut paths = paths.into_iter();
    let (first, second) = (paths.next(), paths.next());
    let startup = match (first, second) {
        (None, _) => Startup { left, right, active, active_path: None },
        (Some(only), None) => Startup { left, right, active, active_path: Some(only) },
        // Two paths are the panels; an option given too wins.
        (Some(first), Some(second)) if count == 2 => {
            Startup { left: left.or(Some(first)), right: right.or(Some(second)), active, active_path: None }
        }
        _ => return Err(StartupError::TooManyPaths(count)),
    };
    Ok(startup)
}

/// Where a panel goes for the absolute `path` of the command line: the
/// directory to show, and the name to put the cursor on.
#[must_use]
pub fn place(path: &Path) -> (PathBuf, Option<OsString>) {
    match destination(path) {
        Destination::Directory => (path.to_owned(), None),
        Destination::File { dir, name } => (dir, Some(name)),
        Destination::Missing { existing, .. } => (existing, None),
        // A file on the way: there, with the cursor on it.
        Destination::Blocked(file) => match (file.parent(), file.file_name()) {
            (Some(dir), Some(name)) => (dir.to_owned(), Some(name.to_owned())),
            _ => (PathBuf::from("/"), None),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn one_path_is_for_the_active_panel_two_for_both() {
        let none: Vec<&str> = Vec::new();
        assert!(arrange(None, None, None, none.clone()).unwrap().is_empty());
        assert_eq!(
            arrange(None, None, None, vec!["a"]).unwrap(),
            Startup { left: None, right: None, active: None, active_path: Some("a") }
        );
        assert_eq!(
            arrange(None, None, None, vec!["a", "b"]).unwrap(),
            Startup { left: Some("a"), right: Some("b"), active: None, active_path: None }
        );
        assert_eq!(
            arrange(None, Some("r"), Some("r"), vec!["a", "b"]).unwrap(),
            Startup { left: Some("a"), right: Some("r"), active: Some(Side::Right), active_path: None }
        );
        assert_eq!(
            arrange(Some("l"), None, Some(" L "), none).unwrap(),
            Startup { left: Some("l"), right: None, active: Some(Side::Left), active_path: None }
        );
    }

    #[test]
    fn refuses_what_it_does_not_understand() {
        assert_eq!(arrange(None, None, Some("x"), vec!["a"]), Err(StartupError::ActivePanel("x".to_owned())));
        assert_eq!(arrange(None, None, None, vec!["a", "b", "c"]), Err(StartupError::TooManyPaths(3)));
    }

    #[test]
    fn goes_to_the_directory_the_path_leads_to() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("dir/file.txt"), b"").unwrap();
        assert_eq!(place(&root.join("dir")), (root.join("dir"), None));
        assert_eq!(place(&root.join("dir/file.txt")), (root.join("dir"), Some(OsString::from("file.txt"))));
        assert_eq!(place(&root.join("dir/a/b")), (root.join("dir"), None));
        assert_eq!(place(&root.join("dir/file.txt/x")), (root.join("dir"), Some(OsString::from("file.txt"))));
    }
}

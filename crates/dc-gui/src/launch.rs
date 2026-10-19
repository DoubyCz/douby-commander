//! Starting text editors: the terminal one (F3) and the desktop's (F4).
//!
//! The terminal editor is the user's choice as `sensible-editor` of Debian finds it:
//! `$VISUAL`, `$EDITOR`, then `sensible-editor` where the system has it,
//! then common editors. The terminal is the default one of the desktop
//! (`xdg-terminal-exec`, the proposed freedesktop default terminal spec that
//! Omarchy and others ship), else `$TERMINAL` or Debian's
//! `x-terminal-emulator`, else a known terminal.
//!
//! `editor` itself is not used: on Debian it is the alternatives link, but
//! on Arch `/usr/bin/editor` belongs to an unrelated game.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

/// Editors tried when the user has chosen none.
const FALLBACK_EDITORS: [&str; 4] = ["sensible-editor", "nano", "vim", "vi"];

/// Terminals tried without a default terminal; all take `-e command args`.
const FALLBACK_TERMINALS: [&str; 7] = ["x-terminal-emulator", "foot", "kitty", "alacritty", "ghostty", "konsole", "xterm"];

#[derive(Debug)]
pub enum LaunchError {
    NoEditor,
    NoTerminal,
    NoDesktopEditor,
    Spawn(io::Error),
    Launch(glib::Error),
}

/// Opens `file` in the desktop's text editor: the default application for
/// `text/plain` (`xdg-mime query default text/plain`), also for files of
/// other types, e.g. scripts or configuration.
///
/// # Errors
/// When no application is set for text, or it does not start.
pub fn edit_in_desktop_editor(file: &Path) -> Result<(), LaunchError> {
    let app = gio::AppInfo::default_for_type("text/plain", false).ok_or(LaunchError::NoDesktopEditor)?;
    let context = gdk::Display::default().map(|display| display.app_launch_context());
    app.launch(&[gio::File::for_path(file)], context.as_ref()).map_err(LaunchError::Launch)
}

/// Opens `file` in the text editor, in a terminal, in the file's directory.
///
/// # Errors
/// When there is no editor or terminal, or the terminal does not start.
pub fn edit_in_terminal(file: &Path) -> Result<(), LaunchError> {
    let editor = editor(|name| std::env::var_os(name), |program| find_program(program).is_some())
        .ok_or(LaunchError::NoEditor)?;
    let dir = file.parent().unwrap_or(Path::new("/"));
    // The editor setting may carry arguments (`code -w`), so the shell
    // splits it; the file goes as a separate, untouched argument.
    let editor_args: [&std::ffi::OsStr; 4] =
        ["sh".as_ref(), "-c".as_ref(), r#"exec $DC_EDITOR "$1""#.as_ref(), "sh".as_ref()];

    let mut command = if find_program("xdg-terminal-exec").is_some() {
        let mut command = Command::new("xdg-terminal-exec");
        command.arg(format!("--dir={}", dir.display())).arg("--");
        command
    } else {
        let terminal = std::env::var_os("TERMINAL")
            .filter(|terminal| !terminal.is_empty())
            .into_iter()
            .chain(FALLBACK_TERMINALS.iter().map(OsString::from))
            .find(|terminal| find_program(terminal).is_some())
            .ok_or(LaunchError::NoTerminal)?;
        let mut command = Command::new(terminal);
        command.arg("-e");
        command
    };
    command
        .args(editor_args)
        .arg(file)
        .env("DC_EDITOR", editor)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(LaunchError::Spawn)?;
    // Reaped when it ends, so that no zombie stays behind.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// The editor command line as `sensible-editor` would choose it.
fn editor(var: impl Fn(&str) -> Option<OsString>, exists: impl Fn(&str) -> bool) -> Option<OsString> {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|name| var(name))
        .find(|value| !value.is_empty())
        .or_else(|| FALLBACK_EDITORS.iter().find(|program| exists(program)).map(OsString::from))
}

/// `program` on `$PATH` (or a path), if it is an executable file.
fn find_program(program: impl AsRef<std::ffi::OsStr>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let program = Path::new(program.as_ref());
    let executable = |path: &Path| path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    if program.components().count() > 1 {
        return executable(program).then(|| program.to_owned());
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).map(|dir| dir.join(program)).find(|path| executable(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_like_sensible_editor() {
        let none = |_: &str| None;
        let env = |values: &'static [(&'static str, &'static str)]| {
            move |name: &str| values.iter().find(|(key, _)| *key == name).map(|(_, value)| OsString::from(value))
        };
        assert_eq!(editor(env(&[("VISUAL", "nvim"), ("EDITOR", "nano")]), |_| false), Some("nvim".into()));
        assert_eq!(editor(env(&[("VISUAL", ""), ("EDITOR", "code -w")]), |_| false), Some("code -w".into()));
        assert_eq!(editor(none, |program| program == "vim"), Some("vim".into()));
        assert_eq!(editor(none, |program| program == "editor"), None, "`editor` is not trusted");
    }

    #[test]
    fn finds_programs_on_the_path() {
        assert!(find_program("sh").is_some());
        assert!(find_program("surely-no-such-program-here").is_none());
    }
}

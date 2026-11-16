//! Starting programs of the desktop: its terminal (Shift+F9).
//!
//! The terminal is the default one of the desktop (`xdg-terminal-exec`, the
//! proposed freedesktop default terminal spec that Omarchy and others ship),
//! else `$TERMINAL` or Debian's `x-terminal-emulator`, else a known terminal.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// Terminals tried without a default terminal; all take `-e command args`.
const FALLBACK_TERMINALS: [&str; 7] = ["x-terminal-emulator", "foot", "kitty", "alacritty", "ghostty", "konsole", "xterm"];

#[derive(Debug)]
pub enum LaunchError {
    NoTerminal,
    Spawn(io::Error),
}

/// `cm_RunTerm`: the desktop's terminal in `dir`.
///
/// # Errors
/// When there is no terminal or it does not start.
pub fn open_terminal(dir: &Path) -> Result<(), LaunchError> {
    let mut command = if find_program("xdg-terminal-exec").is_some() {
        let mut command = Command::new("xdg-terminal-exec");
        command.arg(format!("--dir={}", dir.display()));
        command
    } else {
        let terminal = std::env::var_os("TERMINAL")
            .filter(|terminal| !terminal.is_empty())
            .into_iter()
            .chain(FALLBACK_TERMINALS.iter().map(OsString::from))
            .find(|terminal| find_program(terminal).is_some())
            .ok_or(LaunchError::NoTerminal)?;
        Command::new(terminal)
    };
    command.current_dir(dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = command.spawn().map_err(LaunchError::Spawn)?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

pub use dc_core::program::find as find_program;

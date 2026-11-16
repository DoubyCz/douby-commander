//! Programs of the system: finding them on `$PATH` and running them
//! without a shell, with a time limit. What the application does not read
//! itself, the tools of the distribution do (the quick view, the search in
//! documents); a missing tool is a hint what to install.

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};


/// `program` on `$PATH` (or a path), if it is an executable file.
#[must_use]
pub fn find(program: impl AsRef<OsStr>) -> Option<PathBuf> {
    let program = Path::new(program.as_ref());
    let executable = |path: &Path| path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    if program.components().count() > 1 {
        return executable(program).then(|| program.to_owned());
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).map(|dir| dir.join(program)).find(|path| executable(path))
}

/// Runs `program` without a shell, `input` on its standard input; its
/// standard output (at most `max_output` bytes), unless it did not start,
/// ran out of `timeout` or was cancelled.
#[must_use]
pub fn run(
    program: &Path,
    args: &[OsString],
    input: Option<&[u8]>,
    timeout: Duration,
    max_output: usize,
    cancel: &AtomicBool,
) -> Option<Vec<u8>> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        // Small, it fits the pipe; a program that does not read it fails.
        stdin.write_all(input).ok();
    }
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        stdout.by_ref().take(max_output as u64).read_to_end(&mut output).map(|_| output)
    });
    let deadline = Instant::now() + timeout;
    let told = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            // All that is read is read: the rest is not needed.
            Ok(None) if reader.is_finished() => {
                child.kill().ok();
                child.wait().ok();
                break true;
            }
            Ok(None) if Instant::now() < deadline && !cancel.load(Ordering::Relaxed) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                child.kill().ok();
                child.wait().ok();
                break false;
            }
        }
    };
    let output = reader.join().ok()?.ok()?;
    told.then_some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_programs_on_the_path() {
        assert!(find("sh").is_some());
        assert!(find("surely-no-such-program-here").is_none());
    }

    #[test]
    fn runs_without_a_shell() {
        let sh = find("sh").unwrap();
        let args = [OsString::from("-c"), OsString::from("cat; echo ' world'")];
        let output = run(&sh, &args, Some(b"hello"), Duration::from_secs(5), 1024, &AtomicBool::new(false));
        assert_eq!(output.as_deref(), Some(&b"hello world\n"[..]));
    }
}

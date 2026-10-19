//! Text shown in panel columns and the status line.

use std::time::{Duration, SystemTime};

use dc_i18n::{fill, gettext, pgettext};
use dc_vfs::EntryKind;
use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::{FileItem, SelectionInfo};

/// File size in the default "float" format of Double Commander:
/// bytes below 1 KiB, otherwise one decimal place and a unit letter.
#[must_use]
pub fn size(bytes: u64) -> String {
    let units = [(1_u64 << 40, 'T'), (1 << 30, 'G'), (1 << 20, 'M'), (1 << 10, 'K')];
    for (base, unit) in units {
        if bytes >= base {
            #[expect(clippy::cast_precision_loss, reason = "the value is rounded to one decimal place anyway")]
            let value = bytes as f64 / base as f64;
            return format!("{value:.1} {}", unit_letter(unit));
        }
    }
    group_thousands(bytes)
}

/// Translatable unit letters, as `rsLegacyDisplaySizeSingleLetter*` of the original.
fn unit_letter(unit: char) -> String {
    match unit {
        'T' => pgettext("size unit, tera", "T"),
        'G' => pgettext("size unit, giga", "G"),
        'M' => pgettext("size unit, mega", "M"),
        _ => pgettext("size unit, kilo", "K"),
    }
}

/// The Size column: `<DIR>`, `<LNK>` for links to directories, or the
/// size; a directory with its calculated size shows that.
#[must_use]
pub fn size_column(item: &FileItem) -> String {
    if let Some(calculated) = item.calculated_size() {
        return size(calculated);
    }
    match item.entry().kind {
        EntryKind::Directory => pgettext("size column", "<DIR>"),
        EntryKind::Symlink { to_directory: true } => pgettext("size column", "<LNK>"),
        _ => size(item.entry().size),
    }
}

/// The Name column: directories in square brackets.
#[must_use]
pub fn name_column(item: &FileItem) -> String {
    if item.is_directory_like() { format!("[{}]", item.name_no_ext()) } else { item.name_no_ext().to_owned() }
}

/// Modification time in the local time zone.
#[must_use]
pub fn modified(time: Option<SystemTime>, tz: &TimeZone) -> String {
    time.and_then(|time| Timestamp::try_from(time).ok())
        .map(|ts| ts.to_zoned(tz.clone()).strftime("%d.%m.%Y %H:%M:%S").to_string())
        .unwrap_or_default()
}

/// Unix permissions as `ls -l` shows them, e.g. `drwxr-xr-x`.
#[must_use]
pub fn attributes(mode: u32, kind: EntryKind) -> String {
    let type_char = match kind {
        EntryKind::Directory => 'd',
        EntryKind::Symlink { .. } => 'l',
        EntryKind::File => '-',
        EntryKind::Other => match mode & 0o170_000 {
            0o060_000 => 'b',
            0o020_000 => 'c',
            0o010_000 => 'p',
            0o140_000 => 's',
            _ => '?',
        },
    };

    // (read, write, execute bit, special bit, special char when executable / not executable)
    let triplets = [
        (0o400, 0o200, 0o100, 0o4000, ('s', 'S')),
        (0o040, 0o020, 0o010, 0o2000, ('s', 'S')),
        (0o004, 0o002, 0o001, 0o1000, ('t', 'T')),
    ];

    let mut out = String::with_capacity(10);
    out.push(type_char);
    for (read, write, exec, special, (special_x, special_no_x)) in triplets {
        out.push(if mode & read != 0 { 'r' } else { '-' });
        out.push(if mode & write != 0 { 'w' } else { '-' });
        out.push(match (mode & special != 0, mode & exec != 0) {
            (true, true) => special_x,
            (true, false) => special_no_x,
            (false, true) => 'x',
            (false, false) => '-',
        });
    }
    out
}

/// Status line under the panel.
#[must_use]
pub fn selection_info(info: &SelectionInfo) -> String {
    // rsMsgSelectedInfo of the original.
    fill(
        &gettext("Selected: {selected_size} of {total_size}, files: {selected_files} of {total_files}, folders: {selected_dirs} of {total_dirs}"),
        &[
            ("selected_size", &size(info.selected_bytes)),
            ("total_size", &size(info.total_bytes)),
            ("selected_files", &info.selected_files),
            ("total_files", &info.total_files),
            ("selected_dirs", &info.selected_dirs),
            ("total_dirs", &info.total_dirs),
        ],
    )
}

/// A count with grouped thousands, e.g. `12 345` (`IntToStrTS` of the original).
#[must_use]
pub fn count(value: u64) -> String {
    group_thousands(value)
}

/// Remaining time of an operation, `HH:MM:SS` (hours may exceed 24).
#[must_use]
pub fn duration(time: Duration) -> String {
    let seconds = time.as_secs();
    format!("{:02}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60)
}

fn group_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(0), "0");
        assert_eq!(size(1023), "1 023");
        assert_eq!(size(1024), "1.0 K");
        assert_eq!(size(1536), "1.5 K");
        assert_eq!(size(5 * 1024 * 1024 + 300_000), "5.3 M");
        assert_eq!(size(3 << 40), "3.0 T");
    }

    #[test]
    fn permission_strings() {
        assert_eq!(attributes(0o040_755, EntryKind::Directory), "drwxr-xr-x");
        assert_eq!(attributes(0o100_644, EntryKind::File), "-rw-r--r--");
        assert_eq!(attributes(0o104_755, EntryKind::File), "-rwsr-xr-x");
        assert_eq!(attributes(0o041_777, EntryKind::Directory), "drwxrwxrwt");
        assert_eq!(attributes(0o120_777, EntryKind::Symlink { to_directory: false }), "lrwxrwxrwx");
        assert_eq!(attributes(0o020_620, EntryKind::Other), "crw--w----");
    }

    #[test]
    fn modified_in_given_time_zone() {
        let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_760_000_000);
        assert_eq!(modified(Some(time), &TimeZone::UTC), "09.10.2025 08:53:20");
        assert_eq!(modified(None, &TimeZone::UTC), "");
    }

    #[test]
    fn counts_and_durations() {
        assert_eq!(count(1_234_567), "1 234 567");
        assert_eq!(duration(Duration::from_secs(59)), "00:00:59");
        assert_eq!(duration(Duration::from_secs(3 * 3600 + 25 * 60 + 7)), "03:25:07");
        assert_eq!(duration(Duration::from_hours(30)), "30:00:00");
    }
}

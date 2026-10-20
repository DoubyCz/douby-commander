//! New names of the multi-rename tool (`cm_MultiRename`, `fmultirename.pas`
//! of the original): masks for the name and the extension with
//! placeholders in `[]`, then find and replace, then the letter case.
//!
//! The placeholders are those of the original, so its masks carry over:
//!
//! | Mask | Gives |
//! |---|---|
//! | `[N]`, `[N3]`, `[N-2]`, `[N2:5]`, `[N2,3]` | name without extension, or its characters: the 3rd, the 2nd from the end, 2nd to 5th, 3 from the 2nd |
//! | `[E]`, `[E1:2]`, ... | extension, the same way |
//! | `[A]`, `[A1:9]`, ... | whole path with extension, `/` replaced by `.` |
//! | `[P]`, `[P2]`, `[P-1]` | a directory of the path, from the root or (negative) from the file |
//! | `[C]`, `[C12]` | counter, its start in the mask overrides the counter's one |
//! | `[G]` | a random GUID |
//! | `[Y]` `[YYYY]` `[M]` `[MM]` `[MMM]` `[MMMM]` `[D]` `[DD]` `[DDD]` `[DDDD]` `[h]` `[hh]` `[n]` `[nn]` `[s]` `[ss]` | date and time of the last change, also combined (`[YYYY-MM-DD]`) |
//!
//! Anything else in `[]` is taken as a date and time format, as there.

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use dc_i18n::{NC_, pgettext};
use jiff::Timestamp;
use jiff::tz::TimeZone;

/// A file to rename, as the masks see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameFile {
    pub dir: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub modified: Option<Timestamp>,
}

impl RenameFile {
    /// Name and extension as the panel shows them: a directory has no
    /// extension, nor a dot file.
    fn split(&self) -> (&str, &str) {
        if self.is_dir { (&self.name, "") } else { split_extension(&self.name) }
    }
}

/// Letter case of the name or of the extension (`cbNameMaskStyle`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CaseStyle {
    #[default]
    Keep,
    Upper,
    Lower,
    /// First letter uppercase, the rest lowercase.
    FirstUpper,
    /// First letter of every word uppercase, the rest lowercase.
    WordsUpper,
}

impl CaseStyle {
    pub const ALL: [Self; 5] = [Self::Keep, Self::Upper, Self::Lower, Self::FirstUpper, Self::WordsUpper];

    /// `rsMulRenFileNameStyleList` of the original.
    #[must_use]
    pub fn title(self) -> String {
        let (context, text) = match self {
            Self::Keep => NC_("letter case", "No change"),
            Self::Upper => NC_("letter case", "UPPERCASE"),
            Self::Lower => NC_("letter case", "lowercase"),
            Self::FirstUpper => NC_("letter case", "First char uppercase"),
            Self::WordsUpper => NC_("letter case", "First Char Of Every Word Uppercase"),
        };
        pgettext(context, text)
    }

    fn apply(self, text: &str) -> String {
        match self {
            Self::Keep => text.to_owned(),
            Self::Upper => text.to_uppercase(),
            Self::Lower => text.to_lowercase(),
            Self::FirstUpper => {
                let lower = text.to_lowercase();
                match lower.char_indices().find(|(_, ch)| ch.is_alphabetic()) {
                    Some((at, ch)) => format!("{}{}{}", &lower[..at], ch.to_uppercase(), &lower[at + ch.len_utf8()..]),
                    None => lower,
                }
            }
            Self::WordsUpper => {
                let mut out = String::with_capacity(text.len());
                let mut word_start = true;
                for ch in text.to_lowercase().chars() {
                    if word_start {
                        out.extend(ch.to_uppercase());
                    } else {
                        out.push(ch);
                    }
                    word_start = !ch.is_alphabetic();
                }
                out
            }
        }
    }
}

/// Find and replace, applied after the masks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[expect(clippy::struct_excessive_bools, reason = "the check boxes of the dialog, each on its own")]
pub struct FindReplace {
    /// Without regular expressions several texts at once, split by `|`,
    /// each replaced by the text at the same place in `replace`.
    pub find: String,
    pub replace: String,
    pub regex: bool,
    pub case_sensitive: bool,
    /// Also in the extension.
    pub in_extension: bool,
    /// `$1` and the like in `replace` stand for groups of the expression.
    pub substitution: bool,
    /// Only the first match in each name.
    pub only_first: bool,
}

/// The counter of `[C]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counter {
    pub start: i64,
    pub step: i64,
    /// Digits at least, zeros in front.
    pub width: usize,
}

impl Default for Counter {
    fn default() -> Self {
        Self { start: 1, step: 1, width: 1 }
    }
}

/// Everything that makes the new names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameMask {
    pub name: String,
    pub extension: String,
    pub name_case: CaseStyle,
    pub extension_case: CaseStyle,
    pub find_replace: FindReplace,
    pub counter: Counter,
}

impl Default for RenameMask {
    fn default() -> Self {
        Self {
            name: "[N]".to_owned(),
            extension: "[E]".to_owned(),
            name_case: CaseStyle::Keep,
            extension_case: CaseStyle::Keep,
            // The original replaces in the extension too by default.
            find_replace: FindReplace { in_extension: true, ..FindReplace::default() },
            counter: Counter::default(),
        }
    }
}

/// The find expression is not a valid regular expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadExpression(pub String);

impl RenameMask {
    /// Checks the find expression once for all names.
    ///
    /// # Errors
    /// When it is a regular expression that does not compile.
    pub fn compile(&self) -> Result<Compiled<'_>, BadExpression> {
        let replacer = match &self.find_replace {
            fr if fr.find.is_empty() => Replacer::Nothing,
            fr if fr.regex => Replacer::Regex(build_regex(&fr.find, fr.case_sensitive)?, numbered_groups(&fr.replace)),
            fr => {
                let replacements: Vec<&str> = fr.replace.split('|').collect();
                let pairs = fr
                    .find
                    .split('|')
                    .enumerate()
                    .filter(|(_, find)| !find.is_empty())
                    .map(|(index, find)| {
                        let replace = replacements.get(index).or(replacements.last()).copied().unwrap_or_default();
                        Ok((build_regex(&regex::escape(find), fr.case_sensitive)?, replace.to_owned()))
                    })
                    .collect::<Result<_, BadExpression>>()?;
                Replacer::Plain(pairs)
            }
        };
        Ok(Compiled { mask: self, replacer })
    }
}

fn build_regex(pattern: &str, case_sensitive: bool) -> Result<regex::Regex, BadExpression> {
    regex::RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|err| BadExpression(err.to_string()))
}

/// `$1` as the original's expressions read it: the group's number only,
/// `$2_x` is group 2 and `_x` (the regex crate would look for a group
/// named `2_x`).
fn numbered_groups(replace: &str) -> String {
    let mut out = String::with_capacity(replace.len());
    let mut chars = replace.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' && chars.peek().is_some_and(char::is_ascii_digit) {
            out.push_str("${");
            while let Some(digit) = chars.next_if(char::is_ascii_digit) {
                out.push(digit);
            }
            out.push('}');
        } else {
            out.push(ch);
        }
    }
    out
}

#[derive(Debug)]
enum Replacer {
    Nothing,
    /// The expression and the replacement as the regex crate reads it.
    Regex(regex::Regex, String),
    Plain(Vec<(regex::Regex, String)>),
}

/// A mask ready to make names.
#[derive(Debug)]
pub struct Compiled<'a> {
    mask: &'a RenameMask,
    replacer: Replacer,
}

impl Compiled<'_> {
    /// The new name of `file`, the `index`th one of the list (for the counter).
    #[must_use]
    pub fn new_name(&self, file: &RenameFile, index: usize, tz: &TimeZone) -> String {
        let values = Values { file, index, counter: self.mask.counter, tz };
        let name = expand(&self.mask.name, &values);
        let extension = expand(&self.mask.extension, &values);
        let mut result = if extension.is_empty() { name } else { format!("{name}.{extension}") };

        let fr = &self.mask.find_replace;
        if !matches!(self.replacer, Replacer::Nothing) {
            let (base, ext) = if fr.in_extension { (result.as_str(), "") } else { split_with_dot(&result) };
            let replaced = self.replace(base);
            result = format!("{replaced}{ext}");
        }

        let (base, ext) = split_with_dot(&result);
        format!("{}{}", self.mask.name_case.apply(base), self.mask.extension_case.apply(ext))
    }

    fn replace(&self, text: &str) -> String {
        let fr = &self.mask.find_replace;
        let limit = usize::from(fr.only_first);
        match &self.replacer {
            Replacer::Nothing => text.to_owned(),
            Replacer::Regex(regex, groups) if fr.substitution => regex.replacen(text, limit, groups.as_str()).into_owned(),
            Replacer::Regex(regex, _) => regex.replacen(text, limit, regex::NoExpand(&fr.replace)).into_owned(),
            Replacer::Plain(pairs) => pairs.iter().fold(text.to_owned(), |text, (find, replace)| {
                find.replacen(&text, limit, regex::NoExpand(replace)).into_owned()
            }),
        }
    }
}

/// Splits at the dot of the extension, the dot stays with it.
fn split_with_dot(name: &str) -> (&str, &str) {
    let (base, ext) = split_extension(name);
    if ext.is_empty() { (name, "") } else { name.split_at(base.len()) }
}

/// `name.ext` at the last dot, as the panel does: a leading dot (`.bashrc`)
/// or a trailing one does not start an extension.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => (&name[..dot], &name[dot + 1..]),
        _ => (name, ""),
    }
}

struct Values<'a> {
    file: &'a RenameFile,
    index: usize,
    counter: Counter,
    tz: &'a TimeZone,
}

/// Replaces every `[...]` of `mask` (`sReplace` of the original): the
/// text up to the first `]` after a `[` is one placeholder; a `[` without
/// a `]` and all after it stay as they are.
fn expand(mask: &str, values: &Values<'_>) -> String {
    let mut out = String::new();
    let mut rest = mask;
    while let Some(start) = rest.find('[') {
        let Some(length) = rest[start + 1..].find(']') else { break };
        out.push_str(&rest[..start]);
        out.push_str(&placeholder(&rest[start + 1..start + 1 + length], values));
        rest = &rest[start + 2 + length..];
    }
    out.push_str(rest);
    out
}

/// One placeholder without its brackets (`sHandleFormatString`).
fn placeholder(text: &str, values: &Values<'_>) -> String {
    let file = values.file;
    let Some(first) = text.chars().next() else { return String::new() };
    let rest = &text[first.len_utf8()..];
    match first {
        // `[[]` is a `[`.
        '[' | ']' => text.to_owned(),
        'N' => characters(rest, file.split().0),
        'E' => characters(rest, file.split().1),
        'A' => characters(rest, &file.dir.join(&file.name).to_string_lossy()).replace('/', "."),
        'G' => guid(),
        'C' => {
            let start = rest.parse().unwrap_or(values.counter.start);
            let step = i64::try_from(values.index).unwrap_or(i64::MAX).saturating_mul(values.counter.step);
            pad_number(start.saturating_add(step), values.counter.width)
        }
        'P' => parent(rest.parse().unwrap_or(0), &file.dir),
        _ => match file.modified {
            Some(time) => format_date_time(text, &time.to_zoned(values.tz.clone())),
            None => text.to_owned(),
        },
    }
}

/// Characters of `text` by the positions after the letter (`sReplaceXX`),
/// counted from 1, negative from the end: none for all, `x` one, `x:y`
/// from x to y, `x,n` n from x.
fn characters(positions: &str, text: &str) -> String {
    if positions.is_empty() {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let length = i64::try_from(chars.len()).unwrap_or(i64::MAX);
    let from_end = |position: i64| if position < 0 { length + position + 1 } else { position };
    let (from, to) = if let Some((from, to)) = positions.split_once(':') {
        let from = from_end(from.parse().unwrap_or(1));
        let to = from_end(to.parse().unwrap_or(i64::from(i16::MAX)));
        if to < from { (to, from) } else { (from, to) }
    } else if let Some((from, count)) = positions.split_once(',') {
        let from = from.parse().unwrap_or(1);
        let count = count.parse::<i64>().map_or(i64::from(i16::MAX), i64::abs);
        if from >= 0 {
            (from, from + count - 1)
        } else {
            let to = length + from + 1;
            ((to - count + 1).max(1), to)
        }
    } else {
        let at = from_end(positions.parse().unwrap_or(1));
        (at, at)
    };
    let from = usize::try_from(from.max(1) - 1).unwrap_or(0);
    let to = usize::try_from(to.max(0)).unwrap_or(0).min(chars.len());
    chars.get(from..to).map(String::from_iter).unwrap_or_default()
}

/// A directory of the path `dir` (`[P]`): the root's is the 0th (empty),
/// counted down from it, or from the file with a negative `index`.
fn parent(index: i64, dir: &Path) -> String {
    let mut parts: Vec<String> = vec![String::new()];
    parts.extend(dir.components().filter_map(|part| match part {
        std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
        _ => None,
    }));
    parts.push(String::new());
    let last = i64::try_from(parts.len() - 1).unwrap_or(0);
    let at = if index < 0 { (last + index).max(0) } else { index.min(last) };
    parts.get(usize::try_from(at).unwrap_or(0)).cloned().unwrap_or_default()
}

/// Directories of `dir` that `[P...]` can give, nearest first, each with
/// its placeholder: for the menu of the masks.
#[must_use]
pub fn parent_placeholders(dir: &Path) -> Vec<(String, String)> {
    let names: Vec<String> = dir
        .components()
        .filter_map(|part| match part {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    names.iter().rev().enumerate().map(|(back, name)| (format!("[P-{}]", back + 1), name.clone())).collect()
}

/// `%.<width>d` of Pascal: the sign, then digits padded with zeros.
fn pad_number(number: i64, width: usize) -> String {
    let digits = number.unsigned_abs().to_string();
    let sign = if number < 0 { "-" } else { "" };
    format!("{sign}{digits:0>width$}")
}

/// `{XXXXXXXX-XXXX-4XXX-YXXX-XXXXXXXXXXXX}` as `GuidToString` writes it.
fn guid() -> String {
    let mut bytes = [0u8; 16];
    if std::fs::File::open("/dev/urandom").and_then(|mut random| random.read_exact(&mut bytes)).is_err() {
        // Not random then, but still unique enough within one run.
        let now = Timestamp::now().as_nanosecond().to_le_bytes();
        bytes.copy_from_slice(&now);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut text = String::from("{");
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        let _ = write!(text, "{byte:02X}");
    }
    text.push('}');
    text
}

/// Month names in the language of the application.
const MONTHS: [(&str, &str); 12] = [
    NC_("month", "January"),
    NC_("month", "February"),
    NC_("month", "March"),
    NC_("month", "April"),
    NC_("month", "May"),
    NC_("month", "June"),
    NC_("month", "July"),
    NC_("month", "August"),
    NC_("month", "September"),
    NC_("month", "October"),
    NC_("month", "November"),
    NC_("month", "December"),
];

const MONTHS_SHORT: [(&str, &str); 12] = [
    NC_("month short", "Jan"),
    NC_("month short", "Feb"),
    NC_("month short", "Mar"),
    NC_("month short", "Apr"),
    NC_("month short", "May"),
    NC_("month short", "Jun"),
    NC_("month short", "Jul"),
    NC_("month short", "Aug"),
    NC_("month short", "Sep"),
    NC_("month short", "Oct"),
    NC_("month short", "Nov"),
    NC_("month short", "Dec"),
];

/// From Sunday, as Pascal counts.
const DAYS: [(&str, &str); 7] = [
    NC_("weekday", "Sunday"),
    NC_("weekday", "Monday"),
    NC_("weekday", "Tuesday"),
    NC_("weekday", "Wednesday"),
    NC_("weekday", "Thursday"),
    NC_("weekday", "Friday"),
    NC_("weekday", "Saturday"),
];

const DAYS_SHORT: [(&str, &str); 7] = [
    NC_("weekday short", "Sun"),
    NC_("weekday short", "Mon"),
    NC_("weekday short", "Tue"),
    NC_("weekday short", "Wed"),
    NC_("weekday short", "Thu"),
    NC_("weekday short", "Fri"),
    NC_("weekday short", "Sat"),
];

fn name_of(names: &[(&str, &str)], index: usize) -> String {
    names.get(index).map(|(context, text)| pgettext(*context, *text)).unwrap_or_default()
}

/// `FormatDateTime` of Pascal for the letters the masks use, in either
/// case: `y`, `m`, `d`, `h`, `n` (minutes), `s`, repeated for more digits
/// or names. Everything else stays as it is.
fn format_date_time(format: &str, time: &jiff::Zoned) -> String {
    let chars: Vec<char> = format.chars().collect();
    let mut out = String::new();
    let mut at = 0;
    while let Some(&ch) = chars.get(at) {
        let lower = ch.to_ascii_lowercase();
        let run = chars[at..].iter().take_while(|next| next.to_ascii_lowercase() == lower).count();
        let month = usize::try_from(time.month() - 1).unwrap_or(0);
        let weekday = usize::from(time.weekday().to_sunday_zero_offset().unsigned_abs());
        let two = |value: i8| format!("{value:02}");
        let text = match (lower, run) {
            ('y', 1 | 2) => format!("{:02}", time.year() % 100),
            ('y', _) => format!("{:04}", time.year()),
            ('m', 1) => time.month().to_string(),
            ('m', 2) => two(time.month()),
            ('m', 3) => name_of(&MONTHS_SHORT, month),
            ('m', _) => name_of(&MONTHS, month),
            ('d', 1) => time.day().to_string(),
            ('d', 2) => two(time.day()),
            ('d', 3) => name_of(&DAYS_SHORT, weekday),
            ('d', _) => name_of(&DAYS, weekday),
            ('h', 1) => time.hour().to_string(),
            ('h', _) => two(time.hour()),
            ('n', 1) => time.minute().to_string(),
            ('n', _) => two(time.minute()),
            ('s', 1) => time.second().to_string(),
            ('s', _) => two(time.second()),
            _ => {
                out.push(ch);
                at += 1;
                continue;
            }
        };
        out.push_str(&text);
        at += run;
    }
    out
}

/// What happens to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// New names that would be the same in one directory: the first of them.
#[must_use]
pub fn first_duplicate(files: &[RenameFile], names: &[String]) -> Option<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    files.iter().zip(names).map(|(file, name)| file.dir.join(name)).find(|path| !seen.insert(path.clone()))
}

/// The renames to do: unchanged names left out. With `auto_rename`, a
/// name that is already taken in its directory becomes `name (1).ext`,
/// `name (2).ext`, ... as in the original.
#[must_use]
pub fn plan(files: &[RenameFile], names: &[String], auto_rename: bool) -> Vec<Rename> {
    let mut taken = std::collections::HashSet::new();
    let mut renames = Vec::new();
    for (file, name) in files.iter().zip(names) {
        let mut to = file.dir.join(name);
        if auto_rename && taken.contains(&to) {
            let (base, ext) = split_extension(name);
            // One more number than names taken is always free.
            to = (1..=taken.len() + 1)
                .map(|number| {
                    let numbered = if ext.is_empty() { format!("{base} ({number})") } else { format!("{base} ({number}).{ext}") };
                    file.dir.join(numbered)
                })
                .find(|candidate| !taken.contains(candidate))
                .unwrap_or(to);
        }
        taken.insert(to.clone());
        let from = file.dir.join(&file.name);
        if from != to {
            renames.push(Rename { from, to });
        }
    }
    renames
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> RenameFile {
        RenameFile {
            dir: PathBuf::from("/home/user/photos"),
            name: name.to_owned(),
            is_dir: false,
            modified: Some("2026-03-07T08:05:09Z".parse().expect("a valid time")),
        }
    }

    fn rename(mask: &RenameMask, name: &str, index: usize) -> String {
        mask.compile().expect("a valid mask").new_name(&file(name), index, &TimeZone::UTC)
    }

    fn masked(name: &str, extension: &str) -> RenameMask {
        RenameMask { name: name.to_owned(), extension: extension.to_owned(), ..RenameMask::default() }
    }

    #[test]
    fn default_mask_keeps_the_name() {
        assert_eq!(rename(&RenameMask::default(), "IMG 01.JPG", 0), "IMG 01.JPG");
        assert_eq!(rename(&RenameMask::default(), ".bashrc", 0), ".bashrc");
    }

    #[test]
    fn character_ranges_like_the_original() {
        assert_eq!(characters("", "abcdef"), "abcdef");
        assert_eq!(characters("2", "abcdef"), "b");
        assert_eq!(characters("-2", "abcdef"), "e");
        assert_eq!(characters("2:4", "abcdef"), "bcd");
        assert_eq!(characters("4:2", "abcdef"), "bcd");
        assert_eq!(characters("2:-2", "abcdef"), "bcde");
        assert_eq!(characters("2,3", "abcdef"), "bcd");
        assert_eq!(characters("-2,3", "abcdef"), "cde");
        assert_eq!(characters("9", "abc"), "");
        assert_eq!(characters("2:", "žluťoučký"), "luťoučký");
    }

    #[test]
    fn counter_date_and_literals() {
        let mask = RenameMask { counter: Counter { start: 5, step: 10, width: 3 }, ..masked("[YYYY]-[MM]-[DD] [C]", "[E]") };
        assert_eq!(rename(&mask, "x.jpg", 2), "2026-03-07 025.jpg");
        assert_eq!(rename(&masked("[C12]", "[E]"), "x.jpg", 0), "12.jpg");
        assert_eq!(rename(&masked("[hh]h[nn]_[YYYY-MM-DD]", ""), "x.jpg", 0), "08h05_2026-03-07");
        assert_eq!(rename(&masked("[[]a[]] [N", "[E]"), "x.jpg", 0), "[a] [N.jpg");
        assert_eq!(pad_number(-1, 2), "-01");
    }

    #[test]
    fn path_parts() {
        assert_eq!(rename(&masked("[P-1]_[N]", "[E]"), "x.jpg", 0), "photos_x.jpg");
        assert_eq!(rename(&masked("[P1]", ""), "x.jpg", 0), "home");
        assert_eq!(rename(&masked("[A]", ""), "x.jpg", 0), ".home.user.photos.x.jpg");
        assert_eq!(parent_placeholders(Path::new("/a/b"))[0], ("[P-1]".to_owned(), "b".to_owned()));
    }

    #[test]
    fn find_and_replace() {
        let with = |fr: FindReplace| RenameMask { find_replace: fr, ..RenameMask::default() };
        let plain = FindReplace { find: "a|B".into(), replace: "x|y".into(), ..FindReplace::default() };
        assert_eq!(rename(&with(plain.clone()), "abab.bak", 0), "xyxy.bak");
        let first = FindReplace { only_first: true, case_sensitive: true, ..plain };
        assert_eq!(rename(&with(first), "abab.a", 0), "xbab.a");
        let regex = FindReplace {
            find: r"(\d+)-(\d+)".into(),
            replace: "$2_$1".into(),
            regex: true,
            substitution: true,
            ..FindReplace::default()
        };
        assert_eq!(rename(&with(regex.clone()), "12-34.txt", 0), "34_12.txt");
        let literal = FindReplace { substitution: false, ..regex };
        assert_eq!(rename(&with(literal), "12-34.txt", 0), "$2_$1.txt");
        let ext = FindReplace { find: "txt".into(), replace: "md".into(), in_extension: true, ..FindReplace::default() };
        assert_eq!(rename(&with(ext), "txt.txt", 0), "md.md");
        let bad = FindReplace { find: "(".into(), regex: true, ..FindReplace::default() };
        assert!(with(bad).compile().is_err());
    }

    #[test]
    fn letter_case() {
        let mask = RenameMask { name_case: CaseStyle::WordsUpper, extension_case: CaseStyle::Lower, ..RenameMask::default() };
        assert_eq!(rename(&mask, "my OLD-photo 2nd.JPG", 0), "My Old-Photo 2Nd.jpg");
        assert_eq!(CaseStyle::FirstUpper.apply("  hELLO wORLD"), "  Hello world");
    }

    #[test]
    fn duplicates_get_numbers() {
        let files = [file("a.jpg"), file("b.jpg"), file("c.jpg")];
        let names = vec!["x.jpg".to_owned(), "x.jpg".to_owned(), "c.jpg".to_owned()];
        assert_eq!(first_duplicate(&files, &names), Some(PathBuf::from("/home/user/photos/x.jpg")));
        let plan = plan(&files, &names, true);
        assert_eq!(plan.len(), 2, "unchanged c.jpg is left out");
        assert_eq!(plan[1].to, PathBuf::from("/home/user/photos/x (1).jpg"));
    }
}

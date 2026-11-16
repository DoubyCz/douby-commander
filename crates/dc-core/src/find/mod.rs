//! Finding files (`cm_Search`, Alt+F7): what a search asks for and the walk
//! that finds it. After `TFindThread` and `uFindFiles` of the original,
//! without its plugins.
//!
//! [`FindQuery`] is what the window shows and what a saved search keeps
//! ([`templates`]); [`Finder`] is a query checked and compiled, ready to
//! run in a worker thread. The worker tells the window how far it got
//! through a shared [`Progress`].

mod attributes;
mod duplicates;
pub mod templates;
mod text;

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use dc_i18n::{fill, gettext, ngettext, plural_count};
use dc_vfs::EntryKind;
use dc_vfs::archive::{ArchiveSource, is_browsable_archive};
use jiff::civil::{Date, Time};
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp, Zoned};
use serde::{Deserialize, Serialize};

use crate::mask::{MaskList, MaskOptions};

pub use attributes::Attributes;
pub use duplicates::Duplicates;
pub use text::{DOCUMENT_TOOLS, ENCODINGS, TextQuery, tool_adds};

/// Everything a search asks for, as the window shows it.
#[expect(clippy::struct_excessive_bools, reason = "the check boxes of the search window")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FindQuery {
    /// Where to start, several directories separated by `;`.
    pub start: String,
    /// Directories not to enter: names, paths relative to the start or
    /// absolute paths, masks allowed, separated by `;`.
    pub exclude_dirs: String,
    /// Names to find, masks separated by `;` (or a regular expression).
    pub masks: String,
    /// Names to leave out, the same way.
    pub exclude_files: String,
    /// A mask without `*` and `?` finds the text anywhere in the name.
    pub partial_names: bool,
    /// `masks` and `exclude_files` are regular expressions.
    pub name_regex: bool,
    /// Levels of subdirectories below the start, `None` all of them.
    pub depth: Option<u32>,
    pub follow_links: bool,
    /// Names inside archives the panel can enter are searched too.
    pub in_archives: bool,
    pub text: TextQuery,
    pub date_from: Option<Date>,
    pub time_from: Option<Time>,
    pub date_to: Option<Date>,
    pub time_to: Option<Time>,
    /// Modified at most so long ago; the dates are left aside then.
    pub age: Option<Age>,
    pub size_from: Option<u64>,
    pub size_to: Option<u64>,
    pub size_unit: SizeUnit,
    /// File types and permissions, `d+ux-` (see [`Attributes`]).
    pub attributes: String,
    pub duplicates: Option<Duplicates>,
}

impl Default for FindQuery {
    /// A new search as the original starts it: all names, any depth, a
    /// part of the name is enough.
    fn default() -> Self {
        Self {
            start: String::new(),
            exclude_dirs: String::new(),
            masks: String::new(),
            exclude_files: String::new(),
            partial_names: true,
            name_regex: false,
            depth: None,
            follow_links: false,
            in_archives: false,
            text: TextQuery::default(),
            date_from: None,
            time_from: None,
            date_to: None,
            time_to: None,
            age: None,
            size_from: None,
            size_to: None,
            size_unit: SizeUnit::Kilo,
            attributes: String::new(),
            duplicates: None,
        }
    }
}

/// "Not older than" so many units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Age {
    pub amount: u32,
    pub unit: TimeUnit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeUnit {
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

impl TimeUnit {
    pub const ALL: [Self; 7] = [Self::Second, Self::Minute, Self::Hour, Self::Day, Self::Week, Self::Month, Self::Year];

    /// The unit after the number `count` ("5 days").
    #[must_use]
    pub fn title(self, count: u32) -> String {
        let n = plural_count(usize::try_from(count).unwrap_or(usize::MAX));
        match self {
            Self::Second => ngettext("second", "seconds", n),
            Self::Minute => ngettext("minute", "minutes", n),
            Self::Hour => ngettext("hour", "hours", n),
            Self::Day => ngettext("day", "days", n),
            Self::Week => ngettext("week", "weeks", n),
            Self::Month => ngettext("month", "months", n),
            Self::Year => ngettext("year", "years", n),
        }
    }

    fn span(self, amount: i64) -> Span {
        match self {
            Self::Second => Span::new().seconds(amount),
            Self::Minute => Span::new().minutes(amount),
            Self::Hour => Span::new().hours(amount),
            Self::Day => Span::new().days(amount),
            Self::Week => Span::new().weeks(amount),
            Self::Month => Span::new().months(amount),
            Self::Year => Span::new().years(amount),
        }
    }
}

/// Unit of the size fields, powers of 1024 as everywhere in the panels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SizeUnit {
    Bytes,
    Kilo,
    Mega,
    Giga,
    Tera,
}

impl SizeUnit {
    pub const ALL: [Self; 5] = [Self::Bytes, Self::Kilo, Self::Mega, Self::Giga, Self::Tera];

    #[must_use]
    pub fn title(self) -> String {
        match self {
            Self::Bytes => gettext("bytes"),
            Self::Kilo => gettext("KiB"),
            Self::Mega => gettext("MiB"),
            Self::Giga => gettext("GiB"),
            Self::Tera => gettext("TiB"),
        }
    }

    #[must_use]
    pub fn bytes(self, amount: u64) -> u64 {
        let shift = match self {
            Self::Bytes => 0,
            Self::Kilo => 10,
            Self::Mega => 20,
            Self::Giga => 30,
            Self::Tera => 40,
        };
        amount.saturating_mul(1 << shift)
    }
}

/// Where a search runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Roots {
    /// Through these directories.
    Directories(Vec<PathBuf>),
    /// Only these items (marked in the panel): files are checked, directories
    /// searched through.
    Selected(Vec<PathBuf>),
}

/// Why a search cannot start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindError {
    /// A start directory that is not there.
    NoDirectory(PathBuf),
    /// A regular expression that does not compile; which field, the error.
    BadExpression { field: Field, message: String },
    /// Hexadecimal text with something else than pairs of hex digits.
    BadHex(String),
    /// An encoding nobody knows.
    BadEncoding(String),
    /// An attribute pattern with an unknown part.
    BadAttributes(String),
    /// Duplicates by neither name nor size.
    DuplicatesHowUnknown,
}

/// The field a regular expression is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Masks,
    ExcludeFiles,
    Text,
}

impl fmt::Display for FindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            // rsFindDirNoEx of the original.
            Self::NoDirectory(path) => fill(&gettext("Directory {path} does not exist!"), &[("path", &path.display())]),
            Self::BadExpression { field, message } => {
                let template = match field {
                    Field::Masks => gettext("Invalid regular expression in the file mask:\n{message}"),
                    Field::ExcludeFiles => gettext("Invalid regular expression in the excluded files:\n{message}"),
                    Field::Text => gettext("Invalid regular expression in the text to find:\n{message}"),
                };
                fill(&template, &[("message", message)])
            }
            Self::BadHex(text) => fill(
                &gettext("\"{text}\" is not a hexadecimal string: pairs of digits 0-9 and letters A-F, spaces between them allowed."),
                &[("text", text)],
            ),
            Self::BadEncoding(name) => fill(&gettext("Unknown encoding: {name}"), &[("name", name)]),
            Self::BadAttributes(part) => fill(&gettext("Unknown attribute: {part}"), &[("part", part)]),
            Self::DuplicatesHowUnknown => gettext("Duplicate files are found by the same name, the same size or both."),
        };
        f.write_str(&text)
    }
}

impl std::error::Error for FindError {}

/// One item found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Inside an archive: the archive's path followed by the path within.
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Duplicates: the group of the same files, counted from 1.
    pub group: Option<usize>,
}

/// What a running search has done so far, shared with the window.
#[derive(Debug, Default)]
pub struct Progress {
    scanned: AtomicU64,
    cancelled: AtomicBool,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    current: PathBuf,
    found: Vec<Found>,
    found_count: usize,
    comparing: bool,
    missing_tools: Vec<&'static str>,
}

impl Progress {
    /// Stops the search as soon as it notices.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Items looked at.
    #[must_use]
    pub fn scanned(&self) -> u64 {
        self.scanned.load(Ordering::Relaxed)
    }

    /// Items found, also those taken already.
    #[must_use]
    pub fn found_count(&self) -> usize {
        self.state().found_count
    }

    /// The directory being searched.
    #[must_use]
    pub fn current(&self) -> PathBuf {
        self.state().current.clone()
    }

    /// The walk is over, files are compared for duplicates.
    #[must_use]
    pub fn is_comparing(&self) -> bool {
        self.state().comparing
    }

    /// Programs that documents needed but that are not installed (known
    /// once the search is over).
    #[must_use]
    pub fn missing_tools(&self) -> Vec<&'static str> {
        self.state().missing_tools.clone()
    }

    /// What was found since the last call.
    pub fn take_found(&self) -> Vec<Found> {
        std::mem::take(&mut self.state().found)
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn enter(&self, dir: &Path) {
        dir.clone_into(&mut self.state().current);
    }

    fn add(&self, found: Found) {
        let mut state = self.state();
        state.found.push(found);
        state.found_count += 1;
    }
}

/// How names are checked.
enum Names {
    Masks { include: MaskList, exclude: MaskList },
    Regex { include: Option<regex::Regex>, exclude: Option<regex::Regex> },
}

impl Names {
    fn matches(&self, name: &str) -> bool {
        match self {
            Self::Masks { include, exclude } => include.matches(name) && !exclude.matches(name),
            Self::Regex { include, exclude } => {
                include.as_ref().is_none_or(|re| re.is_match(name)) && !exclude.as_ref().is_some_and(|re| re.is_match(name))
            }
        }
    }
}

/// One bound of the modification time: a moment, a day or a time of day.
#[derive(Debug, Clone, Copy)]
enum Bound {
    Moment(Timestamp),
    Day(Date),
    TimeOfDay(Time),
}

/// A query checked and compiled.
pub struct Finder {
    roots: Roots,
    names: Names,
    exclude_dirs: Vec<String>,
    depth: Option<u32>,
    follow_links: bool,
    in_archives: bool,
    text: Option<text::TextMatcher>,
    from: Option<Bound>,
    to: Option<Bound>,
    tz: TimeZone,
    size_from: Option<u64>,
    size_to: Option<u64>,
    attributes: Attributes,
    duplicates: Option<Duplicates>,
}

impl fmt::Debug for Finder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Finder").field("roots", &self.roots).finish_non_exhaustive()
    }
}

/// Names compared with masks: in any case and, as the quick search of the
/// panels, without diacritics (`zprava` finds `Zpráva.odt`; the original
/// tells them apart).
const NAME_MASKS: MaskOptions = MaskOptions { case_sensitive: false, ignore_accents: true };

/// Masks of a search for part of the name: one without `*` and `?` is
/// found anywhere in the name, nothing at all means every name
/// (`FileMaskOptionsToChecks` of the original).
fn partial_masks(masks: &str) -> String {
    let parts: Vec<String> = masks
        .split([';', ','])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| if part.contains(['*', '?']) { part.to_owned() } else { format!("*{part}*") })
        .collect();
    if parts.is_empty() { "*".to_owned() } else { parts.join(";") }
}

fn name_regex(pattern: &str, field: Field) -> Result<Option<regex::Regex>, FindError> {
    if pattern.is_empty() {
        return Ok(None);
    }
    regex::Regex::new(pattern).map(Some).map_err(|err| FindError::BadExpression { field, message: err.to_string() })
}

impl Finder {
    /// Checks `query` for a search in `roots`; `now` and `tz` place its dates.
    ///
    /// # Errors
    /// The first thing wrong with the query, see [`FindError`].
    pub fn new(query: &FindQuery, roots: Roots, now: Timestamp, tz: TimeZone) -> Result<Self, FindError> {
        if let Roots::Directories(dirs) = &roots
            && let Some(missing) = dirs.iter().find(|dir| !dir.is_dir())
        {
            return Err(FindError::NoDirectory(missing.clone()));
        }
        let names = if query.name_regex {
            Names::Regex {
                include: name_regex(&query.masks, Field::Masks)?,
                exclude: name_regex(&query.exclude_files, Field::ExcludeFiles)?,
            }
        } else {
            let include = if query.partial_names { partial_masks(&query.masks) } else { query.masks.clone() };
            let include = if include.trim().is_empty() { "*".to_owned() } else { include };
            Names::Masks {
                include: MaskList::new(&include, NAME_MASKS),
                exclude: MaskList::new(&query.exclude_files, NAME_MASKS),
            }
        };
        let text = if query.text.enabled { Some(text::TextMatcher::new(&query.text)?) } else { None };
        let (from, to) = match query.age {
            Some(age) => {
                let since = now.to_zoned(tz.clone()).checked_sub(age.unit.span(i64::from(age.amount)));
                (Some(Bound::Moment(since.map_or(Timestamp::MIN, |since| since.timestamp()))), None)
            }
            None => (bound(query.date_from, query.time_from, &tz), bound(query.date_to, query.time_to, &tz)),
        };
        let duplicates = query.duplicates;
        if duplicates.is_some_and(|by| !by.name && !by.size) {
            return Err(FindError::DuplicatesHowUnknown);
        }
        Ok(Self {
            roots,
            names,
            exclude_dirs: query
                .exclude_dirs
                .split(';')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect(),
            depth: query.depth,
            follow_links: query.follow_links,
            in_archives: query.in_archives,
            text,
            from,
            to,
            tz,
            size_from: query.size_from.map(|size| query.size_unit.bytes(size)),
            size_to: query.size_to.map(|size| query.size_unit.bytes(size)),
            attributes: Attributes::parse(&query.attributes)?,
            duplicates,
        })
    }

    /// Searches, telling `progress` what it finds, until done or cancelled.
    pub fn run(&self, progress: &Progress) {
        let mut walk = Walk { finder: self, progress, visited: HashSet::new(), candidates: Vec::new() };
        match &self.roots {
            Roots::Directories(dirs) => {
                for dir in dirs {
                    if let Ok(metadata) = fs::metadata(dir) {
                        walk.visited.insert((metadata.dev(), metadata.ino()));
                    }
                    walk.directory(dir, dir, 0);
                }
            }
            Roots::Selected(items) => {
                for item in items {
                    let Ok(metadata) = fs::symlink_metadata(item) else { continue };
                    let is_dir = fs::metadata(item).is_ok_and(|target| target.is_dir());
                    if is_dir && (!metadata.is_symlink() || self.follow_links) {
                        walk.directory(item, item, 0);
                    } else {
                        walk.item(item, &metadata, is_dir);
                    }
                }
            }
        }
        if let Some(text) = &self.text {
            progress.state().missing_tools = text.missing_tools();
        }
        if self.duplicates.is_some() && !progress.is_cancelled() {
            progress.state().comparing = true;
            let candidates = std::mem::take(&mut walk.candidates);
            if let Some(by) = self.duplicates {
                duplicates::group(candidates, by, progress);
            }
        }
    }

    fn name_matches(&self, name: &OsStr) -> bool {
        self.names.matches(&name.to_string_lossy())
    }

    fn time_matches(&self, modified: Option<SystemTime>) -> bool {
        if self.from.is_none() && self.to.is_none() {
            return true;
        }
        let Some(time) = modified.and_then(|time| Timestamp::try_from(time).ok()) else { return false };
        let zoned = time.to_zoned(self.tz.clone());
        self.from.is_none_or(|from| after(&zoned, from)) && self.to.is_none_or(|to| before(&zoned, to))
    }

    fn size_matches(&self, size: u64, is_dir: bool) -> bool {
        if self.size_from.is_none() && self.size_to.is_none() {
            return true;
        }
        // A directory has no size of its own worth comparing.
        !is_dir && self.size_from.is_none_or(|from| size >= from) && self.size_to.is_none_or(|to| size <= to)
    }

    /// Whether the walk goes into the directory `path` under `root`.
    fn may_enter(&self, path: &Path, root: &Path) -> bool {
        let name = path.file_name().map(OsStr::to_string_lossy).unwrap_or_default();
        let relative = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
        let full = path.to_string_lossy();
        !self.exclude_dirs.iter().any(|exclude| {
            let mask = MaskList::new(exclude, MaskOptions::default());
            if exclude.starts_with('/') {
                mask.matches(&full)
            } else if exclude.contains('/') {
                mask.matches(&relative)
            } else {
                mask.matches(&name)
            }
        })
    }
}

/// The bound given by a date, a time or both.
fn bound(date: Option<Date>, time: Option<Time>, tz: &TimeZone) -> Option<Bound> {
    match (date, time) {
        (Some(date), Some(time)) => {
            Some(date.to_datetime(time).to_zoned(tz.clone()).map_or(Bound::Day(date), |zoned| Bound::Moment(zoned.timestamp())))
        }
        (Some(date), None) => Some(Bound::Day(date)),
        (None, Some(time)) => Some(Bound::TimeOfDay(time)),
        (None, None) => None,
    }
}

fn after(time: &Zoned, from: Bound) -> bool {
    match from {
        Bound::Moment(moment) => time.timestamp() >= moment,
        Bound::Day(day) => time.date() >= day,
        Bound::TimeOfDay(of_day) => time.time() >= of_day,
    }
}

fn before(time: &Zoned, to: Bound) -> bool {
    match to {
        Bound::Moment(moment) => time.timestamp() <= moment,
        Bound::Day(day) => time.date() <= day,
        Bound::TimeOfDay(of_day) => time.time() <= of_day,
    }
}

/// One run of a search.
struct Walk<'a> {
    finder: &'a Finder,
    progress: &'a Progress,
    /// Directories entered by device and inode, a link back up is a cycle.
    visited: HashSet<(u64, u64)>,
    /// Files that may be duplicates, compared at the end.
    candidates: Vec<Found>,
}

impl Walk<'_> {
    fn directory(&mut self, dir: &Path, root: &Path, depth: u32) {
        if self.progress.is_cancelled() {
            return;
        }
        self.progress.enter(dir);
        let Ok(read) = fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = read.flatten().collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            if self.progress.is_cancelled() {
                return;
            }
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else { continue };
            let is_link = metadata.is_symlink();
            let is_dir = if is_link { fs::metadata(&path).is_ok_and(|target| target.is_dir()) } else { metadata.is_dir() };
            self.item(&path, &metadata, is_dir);
            if !is_dir || self.finder.depth.is_some_and(|max| depth >= max) || !self.finder.may_enter(&path, root) {
                continue;
            }
            if is_link && !self.finder.follow_links {
                continue;
            }
            if self.finder.follow_links {
                let Ok(target) = fs::metadata(&path) else { continue };
                if !self.visited.insert((target.dev(), target.ino())) {
                    continue;
                }
            }
            self.directory(&path, root, depth + 1);
            self.progress.enter(dir);
        }
    }

    /// Checks one item (`metadata` of the item itself, a link not followed).
    fn item(&mut self, path: &Path, metadata: &Metadata, is_dir: bool) {
        let finder = self.finder;
        let name = path.file_name().unwrap_or(path.as_os_str());
        if finder.in_archives && metadata.is_file() && is_browsable_archive(name) {
            self.archive(path);
        }
        self.progress.scanned.fetch_add(1, Ordering::Relaxed);
        let modified = metadata.modified().ok();
        let matches = finder.name_matches(name)
            && finder.time_matches(modified)
            && finder.size_matches(metadata.len(), is_dir)
            && finder.attributes.matches(metadata.mode())
            && finder.text.as_ref().is_none_or(|text| !is_dir && text.check(path, &self.progress.cancelled));
        if !matches {
            return;
        }
        let found = Found { path: path.to_owned(), is_dir, size: metadata.len(), modified, group: None };
        if finder.duplicates.is_some() {
            if metadata.is_file() {
                self.candidates.push(found);
            }
        } else {
            self.progress.add(found);
        }
    }

    /// The names inside an archive; its contents are not read, so a search
    /// for text finds nothing there (as in the original).
    fn archive(&mut self, file: &Path) {
        let finder = self.finder;
        if finder.text.is_some() || finder.duplicates.is_some() {
            return;
        }
        let Ok(archive) = ArchiveSource::open(file) else { return };
        let mut items: Vec<_> = archive.items().collect();
        items.sort_by_key(|(path, _)| *path);
        for (inner, entry) in items {
            self.progress.scanned.fetch_add(1, Ordering::Relaxed);
            let is_dir = entry.kind == EntryKind::Directory;
            let matches = finder.name_matches(&entry.name)
                && finder.time_matches(entry.modified)
                && finder.size_matches(entry.size, is_dir)
                && finder.attributes.matches(entry.mode);
            if matches {
                let path = file.join(inner);
                self.progress.add(Found { path, is_dir, size: entry.size, modified: entry.modified, group: None });
            }
        }
    }
}

#[cfg(test)]
mod tests;

//! Where a copy or move goes: the target text of the dialog read as a
//! directory and a rename mask, as `GetDestinationPathAndMask` and
//! `ApplyRenameMask` of the original.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

/// Target of a copy or move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    /// Directory the items go into. Created when missing.
    pub dir: PathBuf,
    /// Rename mask (`*.bak`, `new name`); `None` keeps the names (`*.*`).
    pub mask: Option<String>,
}

impl Destination {
    /// Reads the target `text` of the dialog. A relative text is relative to
    /// `source_dir`. A text ending with `/` or naming an existing directory
    /// is the directory; otherwise its last part is a mask, except for
    /// several items and a plain name: that is a directory to create.
    /// `None` for an empty text.
    #[must_use]
    pub fn parse(text: &str, source_dir: &Path, item_count: usize) -> Option<Self> {
        if text.trim().is_empty() {
            return None;
        }
        let path = normalize(&source_dir.join(text));
        if text.ends_with('/') || path.is_dir() {
            return Some(Self { dir: path, mask: None });
        }
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return Some(Self { dir: path, mask: None });
        };
        let name = name.to_string_lossy().into_owned();
        if item_count > 1 && !has_wildcards(&name) {
            return Some(Self { dir: path, mask: None });
        }
        let mask = (name != "*" && name != "*.*").then_some(name);
        Some(Self { dir: dir.to_owned(), mask })
    }

    /// Name of a top level item at the target. With one directory and a
    /// mask without wildcards the directory is copied under that name;
    /// otherwise files get the mask, directories and links keep their names.
    #[must_use]
    pub fn name_of(&self, name: &OsStr, is_file: bool, single_directory: bool) -> OsString {
        match &self.mask {
            Some(mask) if names_the_directory(mask, single_directory) => mask.into(),
            Some(mask) if is_file => apply_mask(name, mask),
            _ => name.to_owned(),
        }
    }

    /// Name of an item inside a copied directory: files get the mask,
    /// unless the mask only renamed the directory itself.
    #[must_use]
    pub fn inner_name_of(&self, name: &OsStr, is_file: bool, single_directory: bool) -> OsString {
        match &self.mask {
            Some(mask) if is_file && !names_the_directory(mask, single_directory) => apply_mask(name, mask),
            _ => name.to_owned(),
        }
    }

    /// Whether any name changes, which rules out moving trees by a rename.
    #[must_use]
    pub fn renames(&self) -> bool {
        self.mask.is_some()
    }
}

fn has_wildcards(mask: &str) -> bool {
    mask.contains(['*', '?'])
}

/// One directory and a plain name: "copy the directory as `name`".
fn names_the_directory(mask: &str, single_directory: bool) -> bool {
    single_directory && !has_wildcards(mask)
}

/// `.` and `..` resolved without touching the disk (`ExpandAbsolutePath`).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// `mask` applied to `name` (`SplitFileMask` + `ApplyRenameMask`): name and
/// extension separately, by character position: `?` takes the character at
/// its position, `*` the rest from its position, anything else is literal.
/// `*.bak` makes `a.txt` into `a.bak`, `*.*.old` makes it `a.txt.old`.
#[must_use]
pub fn apply_mask(name: &OsStr, mask: &str) -> OsString {
    let (name_mask, ext_mask) = match mask.find("*.*") {
        Some(position) => mask.split_at(position + 1),
        None => split_extension(mask),
    };
    let (name_mask, ext_mask) = if name_mask.is_empty() && ext_mask.is_empty() { ("*", ".*") } else { (name_mask, ext_mask) };
    if name_mask == "*" && ext_mask == ".*" {
        return name.to_owned();
    }
    let name = name.to_string_lossy();
    let (stem, extension) = split_extension(&name);
    let mut out = apply_part(stem, name_mask);
    let extension = apply_part(extension, ext_mask);
    if extension != "." {
        out.push_str(&extension);
    }
    out.into()
}

/// At the last dot, the extension with it (`DivFileName`).
fn split_extension(name: &str) -> (&str, &str) {
    name.rfind('.').map_or((name, ""), |dot| name.split_at(dot))
}

fn apply_part(text: &str, mask: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    for (i, ch) in mask.chars().enumerate() {
        match ch {
            '?' => match chars.get(i) {
                Some(ch) => out.push(*ch),
                None => return text.to_owned(),
            },
            '*' => out.extend(chars.iter().skip(i)),
            literal => out.push(literal),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn masked(name: &str, mask: &str) -> String {
        apply_mask(OsStr::new(name), mask).into_string().unwrap()
    }

    #[test]
    fn masks_like_the_original() {
        assert_eq!(masked("a.txt", "*.bak"), "a.bak");
        assert_eq!(masked("a.txt", "*.*.old"), "a.txt.old");
        assert_eq!(masked("file", "x*"), "xile");
        assert_eq!(masked("a.txt", "*.*"), "a.txt");
        assert_eq!(masked("a.txt", "new.name"), "new.name");
        assert_eq!(masked("report.txt", "??.*"), "re.txt");
        assert_eq!(masked("a.txt", "???.*"), "a.txt", "? beyond the name keeps it whole");
        assert_eq!(masked("čaj.txt", "*_old.*"), "čaj_old.txt");
    }

    #[test]
    fn target_text_is_a_directory_or_a_mask() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let other = tmp.path().join("other");
        fs::create_dir(&src).unwrap();
        fs::create_dir(&other).unwrap();
        let parse = |text: &str, count| Destination::parse(text, &src, count).unwrap();

        assert_eq!(parse(&format!("{}/", other.display()), 1), Destination { dir: other.clone(), mask: None });
        assert_eq!(parse(&other.display().to_string(), 1), Destination { dir: other.clone(), mask: None });
        assert_eq!(parse(&format!("{}/*.*", other.display()), 3), Destination { dir: other.clone(), mask: None });
        assert_eq!(parse(&format!("{}/x.txt", other.display()), 1), Destination { dir: other.clone(), mask: Some("x.txt".into()) });
        assert_eq!(parse(&format!("{}/*.bak", other.display()), 3), Destination { dir: other.clone(), mask: Some("*.bak".into()) });
        // Several items and a plain name: a new directory.
        assert_eq!(parse(&format!("{}/new", other.display()), 2), Destination { dir: other.join("new"), mask: None });
        // Relative to the source directory, with `..`.
        assert_eq!(parse("../other/", 1), Destination { dir: other.clone(), mask: None });
        assert_eq!(parse("copy.txt", 1), Destination { dir: src.clone(), mask: Some("copy.txt".into()) });
        assert_eq!(Destination::parse("  ", &src, 1), None);
    }

    #[test]
    fn one_directory_with_a_plain_name_is_renamed_itself() {
        let dest = Destination { dir: "/t".into(), mask: Some("renamed".into()) };
        assert_eq!(dest.name_of(OsStr::new("dir"), false, true), "renamed");
        assert_eq!(dest.inner_name_of(OsStr::new("f.txt"), true, true), "f.txt");

        let dest = Destination { dir: "/t".into(), mask: Some("*.bak".into()) };
        assert_eq!(dest.name_of(OsStr::new("dir"), false, true), "dir");
        assert_eq!(dest.inner_name_of(OsStr::new("f.txt"), true, true), "f.bak");
        assert_eq!(dest.name_of(OsStr::new("g.txt"), true, false), "g.bak");
    }
}

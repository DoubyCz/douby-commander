//! File types and permissions a search asks for, written as the original
//! writes them on Unix (`UnixSingleStrToFileAttr`): `d+` a directory,
//! `ux+gw-` executable by its owner and not writable by its group, `644+`
//! octal bits. `!` in front negates a pattern, `|` between patterns means
//! any of them.
//!
//! Unlike the original, a type is compared as a type (`r+` is a regular
//! file, never a link, whose type bits contain the regular file's).

use dc_i18n::gettext;

use super::FindError;

/// The mask of the type bits of a mode.
const TYPE_MASK: u32 = 0o170_000;

/// The words of the patterns: permission bits, then types.
const WORDS: [(&str, u32, bool); 19] = [
    ("ur", 0o400, false),
    ("uw", 0o200, false),
    ("ux", 0o100, false),
    ("gr", 0o040, false),
    ("gw", 0o020, false),
    ("gx", 0o010, false),
    ("or", 0o004, false),
    ("ow", 0o002, false),
    ("ox", 0o001, false),
    ("us", 0o4000, false),
    ("gs", 0o2000, false),
    ("sb", 0o1000, false),
    ("r", 0o100_000, true),
    ("d", 0o040_000, true),
    ("l", 0o120_000, true),
    ("f", 0o010_000, true),
    ("c", 0o020_000, true),
    ("b", 0o060_000, true),
    ("s", 0o140_000, true),
];

/// One pattern between `|`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Pattern {
    bits: u32,
    no_bits: u32,
    types: Vec<u32>,
    no_types: Vec<u32>,
    negated: bool,
}

impl Pattern {
    fn matches(&self, mode: u32) -> bool {
        let kind = mode & TYPE_MASK;
        let result = mode & self.bits == self.bits
            && mode & self.no_bits == 0
            && self.types.iter().all(|&wanted| kind == wanted)
            && !self.no_types.contains(&kind);
        result != self.negated
    }
}

/// Patterns of types and permissions; none means anything goes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attributes {
    patterns: Vec<Pattern>,
}

impl Attributes {
    /// # Errors
    /// [`FindError::BadAttributes`] with the word that is not known.
    pub fn parse(text: &str) -> Result<Self, FindError> {
        let mut patterns = Vec::new();
        for part in text.split('|') {
            let mut pattern = Pattern::default();
            let mut word = String::new();
            let mut empty = true;
            for (index, ch) in part.trim().chars().enumerate() {
                match ch {
                    '!' if index == 0 => pattern.negated = true,
                    '+' | '-' => {
                        let have = ch == '+';
                        let word = std::mem::take(&mut word);
                        if word.is_empty() {
                            continue;
                        }
                        empty = false;
                        if word.starts_with(|ch: char| ch.is_ascii_digit()) {
                            let bits = u32::from_str_radix(&word, 8).map_err(|_| FindError::BadAttributes(word.clone()))?;
                            if have { pattern.bits |= bits } else { pattern.no_bits |= bits }
                            continue;
                        }
                        let (_, value, is_type) =
                            WORDS.iter().find(|(name, ..)| *name == word).ok_or(FindError::BadAttributes(word))?;
                        match (is_type, have) {
                            (true, true) => pattern.types.push(*value),
                            (true, false) => pattern.no_types.push(*value),
                            (false, true) => pattern.bits |= value,
                            (false, false) => pattern.no_bits |= value,
                        }
                    }
                    ' ' => {}
                    _ => word.push(ch),
                }
            }
            if !word.is_empty() {
                return Err(FindError::BadAttributes(word));
            }
            if !empty {
                patterns.push(pattern);
            }
        }
        Ok(Self { patterns })
    }

    /// Whether the `mode` (`st_mode`, a link not followed) is wanted.
    #[must_use]
    pub fn matches(&self, mode: u32) -> bool {
        self.patterns.is_empty() || self.patterns.iter().any(|pattern| pattern.matches(mode))
    }

    /// The words with what they mean, for a menu that writes them.
    #[must_use]
    pub fn words() -> Vec<(&'static str, String)> {
        WORDS
            .iter()
            .map(|(word, ..)| {
                let meaning = match *word {
                    "ur" => gettext("owner can read"),
                    "uw" => gettext("owner can write"),
                    "ux" => gettext("owner can execute"),
                    "gr" => gettext("group can read"),
                    "gw" => gettext("group can write"),
                    "gx" => gettext("group can execute"),
                    "or" => gettext("others can read"),
                    "ow" => gettext("others can write"),
                    "ox" => gettext("others can execute"),
                    "us" => gettext("set user ID"),
                    "gs" => gettext("set group ID"),
                    "sb" => gettext("sticky bit"),
                    "r" => gettext("regular file"),
                    "d" => gettext("directory"),
                    "l" => gettext("symbolic link"),
                    "f" => gettext("named pipe (FIFO)"),
                    "c" => gettext("character device"),
                    "b" => gettext("block device"),
                    _ => gettext("socket"),
                };
                (*word, meaning)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE_644: u32 = 0o100_644;
    const SCRIPT_755: u32 = 0o100_755;
    const DIR_755: u32 = 0o040_755;
    const LINK: u32 = 0o120_777;

    #[test]
    fn types_and_permissions() {
        let all = Attributes::parse("").unwrap();
        assert!(all.matches(FILE_644) && all.matches(LINK));
        let dirs = Attributes::parse("d+").unwrap();
        assert!(dirs.matches(DIR_755) && !dirs.matches(FILE_644));
        let scripts = Attributes::parse("r+ux+").unwrap();
        assert!(scripts.matches(SCRIPT_755) && !scripts.matches(FILE_644) && !scripts.matches(DIR_755));
        // A link is no regular file, though its type bits contain them.
        assert!(!Attributes::parse("r+").unwrap().matches(LINK));
        assert!(Attributes::parse("l+").unwrap().matches(LINK));
        assert!(Attributes::parse("!d+").unwrap().matches(FILE_644));
        assert!(Attributes::parse("644+").unwrap().matches(SCRIPT_755));
        assert!(!Attributes::parse("ow+").unwrap().matches(SCRIPT_755));
        let either = Attributes::parse("d+ | l+").unwrap();
        assert!(either.matches(DIR_755) && either.matches(LINK) && !either.matches(FILE_644));
        assert_eq!(Attributes::parse("x+"), Err(FindError::BadAttributes("x".to_owned())));
        assert_eq!(Attributes::parse("ux"), Err(FindError::BadAttributes("ux".to_owned())));
    }
}

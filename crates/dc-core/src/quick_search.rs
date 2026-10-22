//! Quick search: typing letters in a panel jumps to the matching item.
//!
//! Matching follows `TFileListBuilder.PrepareFilter` of the original with
//! default options (match beginning and ending, case insensitive):
//! `abc` matches names starting with `abc`, `abc.t` matches the name `abc`
//! with an extension starting with `t`. `*` and `?` work as wildcards and
//! `;` or `,` separate several patterns.
//!
//! Two deliberate differences from the original: typed characters only
//! accumulate while they come quickly one after another (the original keeps
//! a search box open until Esc), and diacritics are always ignored (the
//! original respects them by default).

use std::time::{Duration, Instant};

use crate::mask::{MaskOptions, normalize, wildcard_match};

/// Characters typed further apart than this start a new search.
pub const TYPING_TIMEOUT: Duration = Duration::from_millis(1000);

/// Accumulates characters typed in quick succession.
#[derive(Debug, Default)]
pub struct TypeAhead {
    text: String,
    last_key: Option<Instant>,
}

impl TypeAhead {
    /// Adds a character typed at `now`, starting over after a pause.
    /// Returns the text to search for.
    pub fn push(&mut self, ch: char, now: Instant) -> &str {
        if !self.is_active(now) {
            self.text.clear();
        }
        self.text.push(ch);
        self.last_key = Some(now);
        &self.text
    }

    /// Backspace inside a running search. Returns `None` when there is no
    /// search to shorten, the key then keeps its usual meaning.
    pub fn pop(&mut self, now: Instant) -> Option<&str> {
        if !self.is_active(now) {
            return None;
        }
        self.text.pop();
        self.last_key = Some(now);
        Some(&self.text)
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether a key at `now` continues the current search.
    #[must_use]
    pub fn is_active(&self, now: Instant) -> bool {
        !self.text.is_empty() && self.last_key.is_some_and(|last| now.duration_since(last) <= TYPING_TIMEOUT)
    }

    pub fn reset(&mut self) {
        self.text.clear();
        self.last_key = None;
    }
}

/// Compiled search text.
#[derive(Debug)]
pub struct Pattern {
    masks: Vec<Vec<char>>,
}

impl Pattern {
    #[must_use]
    pub fn new(text: &str) -> Self {
        let masks = text
            .split([';', ','])
            .filter(|part| !part.is_empty())
            // With both "match beginning" and "match ending" on, PrepareFilter
            // of the original turns `abc` into `abc*` and `abc.t` into `abc.t*`.
            .map(|part| fold(part).chain(['*']).collect())
            .collect();
        Self { masks }
    }

    #[must_use]
    pub fn matches(&self, name: &str) -> bool {
        let name: Vec<char> = fold(name).collect();
        self.masks.iter().any(|mask| wildcard_match(mask, &name))
    }
}

/// Lowercase without diacritics, so `sablony` finds `Šablony` and the other way round.
fn fold(text: &str) -> impl Iterator<Item = char> + '_ {
    normalize(text, MaskOptions { case_sensitive: false, ignore_accents: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_and_case_insensitive() {
        let pattern = Pattern::new("dok");
        assert!(pattern.matches("Dokumenty"));
        assert!(pattern.matches("dok"));
        assert!(!pattern.matches("Mydok"));
    }

    #[test]
    fn dot_splits_name_and_extension() {
        let pattern = Pattern::new("cargo.t");
        assert!(pattern.matches("Cargo.toml"));
        assert!(!pattern.matches("Cargo.lock"));
        assert!(!pattern.matches("cargo-x.toml"));
    }

    #[test]
    fn wildcards_and_several_patterns() {
        assert!(Pattern::new("*.rs").matches("main.rs"));
        assert!(Pattern::new("m?in").matches("main.rs"));
        assert!(Pattern::new("*rc").matches(".bashrc"));
        let both = Pattern::new("foo;bar");
        assert!(both.matches("bar.txt"));
        assert!(both.matches("food"));
        assert!(!both.matches("baz"));
    }

    #[test]
    fn diacritics_are_ignored_on_both_sides() {
        assert!(Pattern::new("sablony").matches("Šablony"));
        assert!(Pattern::new("šablony").matches("sablony"));
        assert!(Pattern::new("obra").matches("Obrázky"));
        assert!(Pattern::new("zlutoucky kun").matches("Žluťoučký kůň.txt"));
        assert!(Pattern::new("uber").matches("Über"));
        assert!(Pattern::new("lodz").matches("Łódź"));
        assert!(Pattern::new("orebro").matches("Örebro"));
        assert!(Pattern::new("ostergaard").matches("Østergaard"));
        // Decomposed input (NFD, as some file systems store names) works as well.
        assert!(Pattern::new("sablony").matches("S\u{30c}ablony"));
    }

    #[test]
    fn typing_accumulates_until_a_pause() {
        let start = Instant::now();
        let mut typed = TypeAhead::default();
        assert_eq!(typed.push('d', start), "d");
        assert_eq!(typed.push('o', start + Duration::from_millis(300)), "do");
        assert_eq!(typed.push('k', start + Duration::from_millis(600)), "dok");

        let later = start + Duration::from_millis(600) + TYPING_TIMEOUT + Duration::from_millis(1);
        assert!(!typed.is_active(later));
        assert_eq!(typed.pop(later), None);
        assert_eq!(typed.push('x', later), "x");
    }

    #[test]
    fn backspace_shortens_running_search() {
        let start = Instant::now();
        let mut typed = TypeAhead::default();
        typed.push('a', start);
        typed.push('b', start);
        assert_eq!(typed.pop(start), Some("a"));
        assert_eq!(typed.pop(start), Some(""));
        // Empty search is over, Backspace goes back to its normal meaning.
        assert_eq!(typed.pop(start), None);
    }
}

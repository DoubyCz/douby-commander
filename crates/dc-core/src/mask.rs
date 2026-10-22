//! File name masks: `*` any run of characters, `?` one character, `;` or
//! `,` between several masks (`TMaskList` of the original).

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// How names are compared with a mask.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MaskOptions {
    pub case_sensitive: bool,
    /// `cs` matches `čš`, `l` matches `ł`.
    pub ignore_accents: bool,
}

/// Compiled masks; a name matches when any of them matches it whole.
#[derive(Debug)]
pub struct MaskList {
    masks: Vec<Vec<char>>,
    options: MaskOptions,
}

impl MaskList {
    #[must_use]
    pub fn new(text: &str, options: MaskOptions) -> Self {
        let masks = text
            .split([';', ','])
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| normalize(part, options).collect())
            .collect();
        Self { masks, options }
    }

    #[must_use]
    pub fn matches(&self, name: &str) -> bool {
        let name: Vec<char> = normalize(name, self.options).collect();
        self.masks.iter().any(|mask| wildcard_match(mask, &name))
    }
}

/// Decomposed (NFD, so both ways of writing `č` compare equal), without
/// diacritics and lowercase as the options say.
pub(crate) fn normalize(text: &str, options: MaskOptions) -> impl Iterator<Item = char> + '_ {
    text.nfd()
        .filter(move |&ch| !(options.ignore_accents && is_combining_mark(ch)))
        .map(move |ch| if options.ignore_accents { base_letter(ch) } else { ch })
        .flat_map(move |ch| {
            let lower = (!options.case_sensitive).then(|| ch.to_lowercase());
            lower.into_iter().flatten().chain(options.case_sensitive.then_some(ch))
        })
}

/// Letters whose stroke or ligature has no Unicode decomposition.
fn base_letter(ch: char) -> char {
    match ch {
        'ł' => 'l',
        'Ł' => 'L',
        'đ' | 'ð' => 'd',
        'Đ' | 'Ð' => 'D',
        'ø' => 'o',
        'Ø' => 'O',
        'ħ' => 'h',
        'Ħ' => 'H',
        'ı' => 'i',
        'ŧ' => 't',
        'Ŧ' => 'T',
        'ƀ' => 'b',
        'Ƀ' => 'B',
        'ƶ' => 'z',
        'Ƶ' => 'Z',
        _ => ch,
    }
}

/// Glob match with `*` (any run of characters) and `?` (one character).
pub(crate) fn wildcard_match(mask: &[char], name: &[char]) -> bool {
    let (mut m, mut n) = (0, 0);
    // Position after the last `*` and the name position it is matched against.
    let mut backtrack: Option<(usize, usize)> = None;

    while n < name.len() {
        match mask.get(m) {
            Some('*') => {
                m += 1;
                backtrack = Some((m, n));
            }
            Some(&c) if c == '?' || c == name[n] => {
                m += 1;
                n += 1;
            }
            _ => match backtrack {
                Some((star_m, star_n)) => {
                    m = star_m;
                    n = star_n + 1;
                    backtrack = Some((star_m, star_n + 1));
                }
                None => return false,
            },
        }
    }
    mask[m..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: MaskOptions = MaskOptions { case_sensitive: false, ignore_accents: false };

    #[test]
    fn whole_name_with_wildcards() {
        let docs = MaskList::new("*.doc", PLAIN);
        assert!(docs.matches("zprava.doc"));
        assert!(docs.matches("ZPRAVA.DOC"));
        assert!(!docs.matches("zprava.docx"));
        assert!(!docs.matches("doc"));
        assert!(MaskList::new("*", PLAIN).matches("cokoliv"));
        assert!(MaskList::new("a?c", PLAIN).matches("abc"));
        assert!(!MaskList::new("a?c", PLAIN).matches("abbc"));
        assert!(MaskList::new("readme", PLAIN).matches("README"));
        assert!(!MaskList::new("readme", PLAIN).matches("README.md"));
    }

    #[test]
    fn several_masks() {
        let list = MaskList::new("*.doc; *.odt,*.txt", PLAIN);
        assert!(list.matches("a.odt"));
        assert!(list.matches("b.txt"));
        assert!(!list.matches("c.pdf"));
        assert!(!MaskList::new(";", PLAIN).matches("a"));
    }

    #[test]
    fn case_sensitive() {
        let list = MaskList::new("*.JPG", MaskOptions { case_sensitive: true, ..PLAIN });
        assert!(list.matches("foto.JPG"));
        assert!(!list.matches("foto.jpg"));
    }

    #[test]
    fn accents() {
        assert!(!MaskList::new("sablona*", PLAIN).matches("šablona.ott"));
        // Composed and decomposed `š` are the same letter.
        assert!(MaskList::new("šablona*", PLAIN).matches("s\u{30c}ablona.ott"));
        let ignoring = MaskOptions { ignore_accents: true, ..PLAIN };
        assert!(MaskList::new("sablona*", ignoring).matches("Šablona.ott"));
        assert!(MaskList::new("lodz", ignoring).matches("Łódź"));
        let both = MaskOptions { case_sensitive: true, ignore_accents: true };
        assert!(MaskList::new("Lodz", both).matches("Łódź"));
        assert!(!MaskList::new("lodz", both).matches("Łódź"));
    }
}

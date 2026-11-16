//! Text inside files: found by the search libraries of ripgrep
//! (`grep-searcher`, `grep-regex`), which read files of any size in
//! blocks, know the encodings of the Encoding Standard and the byte order
//! marks. Text of documents comes from what the distribution offers:
//! `pdftotext` for PDF, `djvutxt` for DjVu, `pandoc` for RTF, `catdoc`
//! for old Word files; documents that are zips of XML (OpenDocument,
//! Office Open XML, EPUB) are read here, no tool needed.

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Encoding, Searcher, SearcherBuilder, Sink, SinkMatch};
use serde::{Deserialize, Serialize};

use super::{Field, FindError};
use crate::program;

/// Encodings offered besides the automatic one (UTF-8, or UTF-16 by its
/// byte order mark): the ones files on Linux still come in.
pub const ENCODINGS: [&str; 14] = [
    "UTF-8",
    "UTF-16LE",
    "UTF-16BE",
    "windows-1250",
    "ISO-8859-2",
    "windows-1252",
    "ISO-8859-1",
    "ISO-8859-15",
    "windows-1251",
    "KOI8-R",
    "IBM866",
    "Shift_JIS",
    "GBK",
    "Big5",
];

/// The text part of a search.
#[expect(clippy::struct_excessive_bools, reason = "the check boxes of the search window")]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TextQuery {
    pub enabled: bool,
    pub text: String,
    /// Found text replaced by this in every file found (UTF-8 files, not
    /// with hexadecimal text or in documents).
    pub replace: Option<String>,
    pub case_sensitive: bool,
    pub regex: bool,
    /// `text` is bytes written in hexadecimal (`4D 5A`).
    pub hex: bool,
    /// Files that do not contain the text are found.
    pub not_containing: bool,
    /// One of [`ENCODINGS`], empty for the automatic one.
    pub encoding: String,
    /// The text of documents (PDF, office files, e-books) is searched,
    /// not their bytes.
    pub documents: bool,
}

/// How the text of a document is read.
#[derive(Debug, Clone, Copy)]
enum Document {
    /// A zip with the text in XML members (which ones it says).
    Zipped(fn(&str) -> bool),
    /// XML itself (FictionBook).
    Xml,
    /// A program that writes the text to its output.
    Tool { program: &'static str, args: fn(&OsStr) -> Vec<OsString> },
}

/// Programs for documents that may be missing.
pub const DOCUMENT_TOOLS: [&str; 4] = ["pdftotext", "djvutxt", "pandoc", "catdoc"];

/// What `program` adds to the search.
#[must_use]
pub fn tool_adds(program: &str) -> String {
    match program {
        "pdftotext" => dc_i18n::gettext("text of PDF documents"),
        "djvutxt" => dc_i18n::gettext("text of DjVu documents"),
        "pandoc" => dc_i18n::gettext("text of RTF documents"),
        _ => dc_i18n::gettext("text of old Word documents (.doc)"),
    }
}

fn document(path: &Path) -> Option<Document> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "docx" | "docm" | "dotx" => Document::Zipped(|name| {
            name == "word/document.xml" || (name.starts_with("word/") && (name.contains("header") || name.contains("footer")))
        }),
        "xlsx" | "xlsm" => Document::Zipped(|name| name == "xl/sharedStrings.xml" || name.starts_with("xl/worksheets/")),
        "pptx" | "ppsx" => Document::Zipped(|name| {
            name.starts_with("ppt/slides/") && Path::new(name).extension().is_some_and(|extension| extension == "xml")
        }),
        "odt" | "ods" | "odp" | "odg" | "ott" | "ots" | "otp" => Document::Zipped(|name| name == "content.xml"),
        "epub" => Document::Zipped(|name| {
            let extension = Path::new(name).extension().and_then(OsStr::to_str).map(str::to_ascii_lowercase);
            matches!(extension.as_deref(), Some("xhtml" | "html" | "htm"))
        }),
        "fb2" => Document::Xml,
        "pdf" => Document::Tool { program: "pdftotext", args: |path| vec![path.to_owned(), "-".into()] },
        "djvu" | "djv" => Document::Tool { program: "djvutxt", args: |path| vec![path.to_owned()] },
        "rtf" => Document::Tool { program: "pandoc", args: |path| vec!["-t".into(), "plain".into(), path.to_owned()] },
        "doc" => Document::Tool { program: "catdoc", args: |path| vec!["-w".into(), path.to_owned()] },
        _ => return None,
    })
}

/// More text of one document is not searched.
const MAX_DOCUMENT_TEXT: usize = 64 << 20;
/// A tool that takes longer is stopped, the document is not found.
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);

/// The text of XML: tags out, the end of a paragraph or a line a new line,
/// the five named entities and numbered characters as what they stand for.
fn xml_text(xml: &[u8]) -> Vec<u8> {
    let xml = String::from_utf8_lossy(xml);
    let mut text = String::with_capacity(xml.len() / 2);
    let mut rest = xml.as_ref();
    while let Some(start) = rest.find(['<', '&']) {
        text.push_str(&rest[..start]);
        rest = &rest[start..];
        if rest.starts_with('<') {
            let end = rest.find('>').map_or(rest.len(), |end| end + 1);
            let tag = &rest[..end];
            let name = tag.trim_start_matches(['<', '/']).split([' ', '>', '/']).next().unwrap_or("");
            let local = name.rsplit(':').next().unwrap_or(name);
            if tag.starts_with("</") && matches!(local, "p" | "h" | "div" | "tr" | "si" | "row" | "title")
                || matches!(local, "br" | "tab" | "line-break" | "cr")
            {
                text.push('\n');
            }
            rest = &rest[end..];
        } else {
            let end = rest.find(';').filter(|&end| end <= 10);
            let entity = end.map(|end| &rest[1..end]);
            let decoded = match entity {
                Some("amp") => Some('&'),
                Some("lt") => Some('<'),
                Some("gt") => Some('>'),
                Some("quot") => Some('"'),
                Some("apos") => Some('\''),
                Some(number) if number.starts_with('#') => {
                    let digits = &number[1..];
                    let code = match digits.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => digits.parse().ok(),
                    };
                    code.and_then(char::from_u32)
                }
                _ => None,
            };
            if let (Some(ch), Some(end)) = (decoded, end) {
                text.push(ch);
                rest = &rest[end + 1..];
            } else {
                text.push('&');
                rest = &rest[1..];
            }
        }
    }
    text.push_str(rest);
    text.into_bytes()
}

/// Bytes written as hexadecimal digits, spaces between them allowed.
fn parse_hex(text: &str) -> Result<Vec<u8>, FindError> {
    let digits: Vec<char> = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    let bad = || FindError::BadHex(text.to_owned());
    if digits.is_empty() || !digits.len().is_multiple_of(2) {
        return Err(bad());
    }
    digits
        .chunks(2)
        .map(|pair| {
            let pair: String = pair.iter().collect();
            u8::from_str_radix(&pair, 16).map_err(|_| bad())
        })
        .collect()
}

/// The first match is enough.
struct First(bool);

impl Sink for First {
    type Error = io::Error;

    fn matched(&mut self, _: &Searcher, _: &SinkMatch<'_>) -> Result<bool, io::Error> {
        self.0 = true;
        Ok(false)
    }
}

/// A text query compiled.
pub(super) struct TextMatcher {
    matcher: RegexMatcher,
    encoding: Option<Encoding>,
    multi_line: bool,
    not_containing: bool,
    documents: bool,
    replace: Option<(regex::bytes::Regex, Vec<u8>, bool)>,
    /// Programs documents needed but that are not installed.
    missing: std::sync::Mutex<std::collections::BTreeSet<&'static str>>,
}

impl TextMatcher {
    pub(super) fn new(query: &TextQuery) -> Result<Self, FindError> {
        let (pattern, case_insensitive) = if query.hex {
            let bytes = parse_hex(&query.text)?;
            let mut pattern = String::new();
            for byte in bytes {
                let _ = write!(pattern, "(?-u:\\x{byte:02X})");
            }
            (pattern, false)
        } else if query.regex {
            (query.text.clone(), !query.case_sensitive)
        } else {
            (regex::escape(&query.text), !query.case_sensitive)
        };
        let bad = |message: String| FindError::BadExpression { field: Field::Text, message };
        let multi_line = query.regex && !query.hex;
        let mut builder = RegexMatcherBuilder::new();
        builder.case_insensitive(case_insensitive).multi_line(multi_line);
        if !multi_line {
            builder.line_terminator(Some(b'\n'));
        }
        let matcher = builder.build(&pattern).map_err(|err| bad(err.to_string()))?;
        let encoding = if query.encoding.is_empty() || query.hex {
            None
        } else {
            Some(Encoding::new(&query.encoding).map_err(|_| FindError::BadEncoding(query.encoding.clone()))?)
        };
        let replace = match &query.replace {
            Some(by) if !query.hex && encoding.is_none() => {
                let regex = regex::bytes::RegexBuilder::new(&pattern)
                    .case_insensitive(case_insensitive)
                    .multi_line(multi_line)
                    .build()
                    .map_err(|err| bad(err.to_string()))?;
                Some((regex, by.clone().into_bytes(), query.regex))
            }
            _ => None,
        };
        Ok(Self {
            matcher,
            encoding,
            multi_line,
            not_containing: query.not_containing,
            documents: query.documents,
            replace,
            missing: std::sync::Mutex::default(),
        })
    }

    /// Whether the file at `path` is found by the text: a regular file
    /// (also behind a link) that contains it, or does not.
    pub(super) fn check(&self, path: &Path, cancel: &AtomicBool) -> bool {
        if !fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) {
            return false;
        }
        let Some(found) = self.contains(path, cancel) else { return false };
        if found && let Some((regex, by, expand)) = &self.replace {
            // A file that cannot be written stays found, unchanged.
            let _ = replace_in(path, regex, by, *expand);
        }
        found != self.not_containing
    }

    /// Programs that documents needed, not installed.
    pub(super) fn missing_tools(&self) -> Vec<&'static str> {
        self.missing.lock().map(|missing| missing.iter().copied().collect()).unwrap_or_default()
    }

    fn searcher(&self) -> Searcher {
        SearcherBuilder::new()
            .binary_detection(BinaryDetection::none())
            .encoding(self.encoding.clone())
            .multi_line(self.multi_line)
            .line_number(false)
            .build()
    }

    /// `None` when the file cannot be read.
    fn contains(&self, path: &Path, cancel: &AtomicBool) -> Option<bool> {
        let mut first = First(false);
        if self.documents && let Some(document) = document(path) {
            let text = match document {
                Document::Zipped(wanted) => {
                    xml_text(&dc_vfs::archive::read_members(path, wanted, MAX_DOCUMENT_TEXT).ok()?)
                }
                Document::Xml => xml_text(&fs::read(path).ok()?),
                Document::Tool { program, args } => {
                    let Some(found) = program::find(program) else {
                        if let Ok(mut missing) = self.missing.lock() {
                            missing.insert(program);
                        }
                        return None;
                    };
                    program::run(&found, &args(path.as_os_str()), None, TOOL_TIMEOUT, MAX_DOCUMENT_TEXT, cancel)?
                }
            };
            // The text of a document is UTF-8 whatever the encoding asked for.
            let mut searcher = SearcherBuilder::new().multi_line(self.multi_line).line_number(false).build();
            searcher.search_slice(&self.matcher, &text, &mut first).ok()?;
        } else {
            self.searcher().search_path(&self.matcher, path, &mut first).ok()?;
        }
        Some(first.0)
    }
}

/// Replaces every match in the file, in place: owner, permissions and
/// hard links stay as they were.
fn replace_in(path: &Path, regex: &regex::bytes::Regex, by: &[u8], expand: bool) -> io::Result<()> {
    let data = fs::read(path)?;
    let replaced =
        if expand { regex.replace_all(&data, by) } else { regex.replace_all(&data, regex::bytes::NoExpand(by)) };
    if let std::borrow::Cow::Owned(replaced) = replaced {
        fs::write(path, replaced)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(text: &str) -> TextQuery {
        TextQuery { enabled: true, text: text.to_owned(), ..TextQuery::default() }
    }

    fn found(query: &TextQuery, path: &Path) -> bool {
        TextMatcher::new(query).unwrap().check(path, &AtomicBool::new(false))
    }

    #[test]
    fn finds_text_in_any_case_and_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.txt");
        fs::write(&file, "Příliš žluťoučký kůň\núpěl ďábelské ódy\n").unwrap();
        assert!(found(&query("ŽLUŤOUČKÝ"), &file));
        assert!(!found(&TextQuery { case_sensitive: true, ..query("ŽLUŤOUČKÝ") }, &file));
        assert!(found(&TextQuery { regex: true, ..query(r"kůň\s+úpěl") }, &file));
        assert!(found(&TextQuery { not_containing: true, ..query("kocour") }, &file));
        assert!(!found(&TextQuery { not_containing: true, ..query("kůň") }, &file));

        let binary = tmp.path().join("a.bin");
        fs::write(&binary, [0x00, 0x4D, 0x5A, 0xFF, 0x00]).unwrap();
        assert!(found(&TextQuery { hex: true, ..query("5a ff") }, &binary));
        assert!(!found(&TextQuery { hex: true, ..query("ff 5a") }, &binary));
        assert!(matches!(TextMatcher::new(&TextQuery { hex: true, ..query("5g") }), Err(FindError::BadHex(_))));
        // Directories and other non-files never contain anything.
        assert!(!found(&TextQuery { not_containing: true, ..query("x") }, tmp.path()));
    }

    #[test]
    fn reads_other_encodings() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("cp1250.txt");
        // "kůň" in windows-1250.
        fs::write(&file, [b'k', 0xF9, 0xF2]).unwrap();
        assert!(!found(&query("kůň"), &file));
        assert!(found(&TextQuery { encoding: "windows-1250".to_owned(), ..query("kůň") }, &file));
        let utf16 = tmp.path().join("utf16.txt");
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend("kůň".encode_utf16().flat_map(u16::to_le_bytes));
        fs::write(&utf16, bytes).unwrap();
        assert!(found(&query("kůň"), &utf16));
    }

    #[test]
    fn replaces_what_it_finds() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.txt");
        fs::write(&file, "verze 1.2, Verze 3.4").unwrap();
        assert!(found(&TextQuery { replace: Some("version".to_owned()), ..query("verze") }, &file));
        assert_eq!(fs::read_to_string(&file).unwrap(), "version 1.2, version 3.4");
        let regex = TextQuery { regex: true, replace: Some("$2.$1".to_owned()), ..query(r"(\d)\.(\d)") };
        assert!(found(&regex, &file));
        assert_eq!(fs::read_to_string(&file).unwrap(), "version 2.1, version 4.3");
    }

    #[test]
    fn text_of_xml() {
        let xml = b"<w:p><w:r><w:t>Tom &amp; Jerry</w:t></w:r></w:p><w:p><w:t>&#x10D;&#269;</w:t></w:p>";
        assert_eq!(String::from_utf8(xml_text(xml)).unwrap(), "Tom & Jerry\nčč\n");
        assert_eq!(String::from_utf8(xml_text(b"a &b c")).unwrap(), "a &b c");
    }
}

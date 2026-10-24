//! Programs of the system that tell more about a file than its type, and
//! what to install when they are missing.
//!
//! Douby Commander reads none of these formats itself: the programs do,
//! the quick view shows what the installed ones tell and names the others
//! with the command that installs them on this distribution.

use std::ffi::{OsStr, OsString};
use std::time::Duration;

use dc_i18n::{fill, gettext};

use super::is_soundfont;

/// A program that tells something about some types of files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoTool {
    /// The presets of a SoundFont.
    Fluidsynth,
    /// The beginning of the text of books and documents, and what a book
    /// says about itself.
    Pandoc,
    /// Metadata of almost any file: documents, fonts, programs,
    /// SoundFont banks.
    Exiftool,
}

impl InfoTool {
    /// In the order they are shown: the special ones first.
    pub const ALL: [Self; 3] = [Self::Fluidsynth, Self::Pandoc, Self::Exiftool];

    #[must_use]
    pub fn program(self) -> &'static str {
        match self {
            Self::Fluidsynth => "fluidsynth",
            Self::Pandoc => "pandoc",
            Self::Exiftool => "exiftool",
        }
    }

    /// How long it may run. Pandoc converts a whole book before it writes
    /// the beginning (seconds for a large one).
    #[must_use]
    pub fn timeout(self) -> Duration {
        match self {
            Self::Pandoc => Duration::from_secs(30),
            Self::Fluidsynth | Self::Exiftool => Duration::from_secs(5),
        }
    }

    /// Whether it tells anything about files of `mime`.
    #[must_use]
    pub fn applies(self, mime: &str) -> bool {
        match self {
            Self::Fluidsynth => is_soundfont(mime),
            Self::Pandoc => pandoc_format(mime).is_some(),
            Self::Exiftool => true,
        }
    }

    /// What it adds to the quick view, for the hint when it is missing.
    #[must_use]
    pub fn adds(self) -> String {
        match self {
            Self::Fluidsynth => gettext("list of instruments in the bank"),
            Self::Pandoc => gettext("beginning of the text"),
            Self::Exiftool => gettext("metadata of the file"),
        }
    }

    /// Arguments for `path` of `mime`; `language` is one of exiftool's
    /// ([`exiftool_language`]).
    #[must_use]
    pub fn args(self, path: &OsStr, mime: &str, language: Option<&str>) -> Vec<OsString> {
        let mut args: Vec<OsString> = match self {
            // No MIDI input, sound into nothing; the commands come on stdin.
            Self::Fluidsynth => ["-q", "-n", "-a", "file", "-o", "audio.file.name=/dev/null"].map(Into::into).to_vec(),
            // The format by the content, not the name; the template on stdin.
            Self::Pandoc => {
                let format = pandoc_format(mime).unwrap_or_default();
                ["-f", format, "-t", "plain", "--wrap=none", "--template=/dev/stdin", "--"].map(Into::into).to_vec()
            }
            // Tab separated; what the file system knows is shown already, the
            // insides of a zip (books and documents are zips) say nothing.
            Self::Exiftool => {
                let mut args: Vec<OsString> = vec!["-t".into()];
                if let Some(language) = language {
                    args.extend(["-lang".into(), language.into()]);
                }
                args.extend(["--File:all", "--ExifTool:all", "--ZIP:all", "--"].map(Into::into));
                args
            }
        };
        args.push(path.to_owned());
        args
    }

    /// What it reads on its standard input.
    #[must_use]
    pub fn input(self) -> Option<&'static [u8]> {
        match self {
            Self::Fluidsynth => Some(b"inst 1\nquit\n"),
            Self::Pandoc => Some(PANDOC_TEMPLATE.as_bytes()),
            Self::Exiftool => None,
        }
    }

    /// Its output as parts of the quick view; none when it told nothing.
    #[must_use]
    pub fn parse(self, output: &str) -> Vec<Section> {
        match self {
            Self::Fluidsynth => {
                let items: Vec<String> = output.lines().filter_map(preset).map(str::to_owned).collect();
                if items.is_empty() {
                    return Vec::new();
                }
                vec![Section::List {
                    title: fill(&gettext("Instruments in the bank: {count}"), &[("count", &items.len())]),
                    items,
                }]
            }
            Self::Pandoc => parse_pandoc(output),
            Self::Exiftool => {
                let fields: Vec<(String, String)> = output
                    .lines()
                    .filter_map(|line| line.split_once('\t'))
                    .map(|(name, value)| (name.trim(), value.trim()))
                    .filter(|(name, value)| !name.is_empty() && !value.is_empty())
                    .map(|(name, value)| (name.to_owned(), value.to_owned()))
                    .collect();
                if fields.is_empty() {
                    return Vec::new();
                }
                vec![Section::Fields { title: gettext("Metadata"), fields }]
            }
        }
    }
}

/// The reader of pandoc for `mime`.
fn pandoc_format(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "application/epub+zip" => "epub",
        "application/x-fictionbook+xml" => "fb2",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.oasis.opendocument.text" => "odt",
        _ => return None,
    })
}

/// What a book says about itself, one `key<TAB>value` a line (a value may
/// go on over more lines), then the text after a line with `\x01`.
const PANDOC_TEMPLATE: &str = "title\t$title$\n\
    author\t$for(author)$$author$$sep$, $endfor$\n\
    publisher\t$publisher$\n\
    date\t$date$\n\
    language\t$lang$\n\
    description\t$description$\n\
    \x01\n\
    $body$\n";

/// So much of the beginning of the text is shown, in characters.
const TEXT_BEGINNING: usize = 3000;

fn parse_pandoc(output: &str) -> Vec<Section> {
    let (head, body) = output.split_once("\n\x01\n").unwrap_or(("", output));
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in head.lines() {
        let label = line.split_once('\t').and_then(|(key, value)| {
            let label = match key {
                "title" => gettext("Title"),
                "author" => gettext("Author"),
                "publisher" => gettext("Publisher"),
                "date" => gettext("Date"),
                "language" => gettext("Language"),
                "description" => gettext("Description"),
                _ => return None,
            };
            Some((label, value))
        });
        match (label, fields.last_mut()) {
            (Some((label, value)), _) => fields.push((label, value.trim().to_owned())),
            (None, Some((_, value))) => {
                value.push('\n');
                value.push_str(line.trim());
            }
            (None, None) => {}
        }
    }
    for (_, value) in &mut fields {
        *value = value.trim().to_owned();
    }
    fields.retain(|(_, value)| !value.is_empty());
    let mut sections = Vec::new();
    if !fields.is_empty() {
        sections.push(Section::Fields { title: gettext("About the book"), fields });
    }
    let text = text_beginning(body);
    if !text.is_empty() {
        sections.push(Section::Text { title: gettext("Beginning of the text"), text });
    }
    sections
}

/// The first [`TEXT_BEGINNING`] characters of `body`, whole paragraphs,
/// without empty pictures (`[]`) and runs of empty lines.
fn text_beginning(body: &str) -> String {
    let mut text = String::new();
    for paragraph in body.split("\n\n").map(str::trim).filter(|p| !p.is_empty() && *p != "[]") {
        if !text.is_empty() {
            if text.chars().count() + 2 + paragraph.chars().count() > TEXT_BEGINNING {
                text.push_str("\n\n…");
                break;
            }
            text.push_str("\n\n");
        }
        text.push_str(paragraph);
    }
    text
}

/// A preset line of fluidsynth's `inst`: `000-001 Piano 2` (bank-program
/// and name), the first one after the prompt `> `.
fn preset(line: &str) -> Option<&str> {
    let line = line.trim_start_matches("> ").trim_end();
    let (number, name) = line.split_once(' ')?;
    let (bank, program) = number.split_once('-')?;
    let digits = |part: &str| part.len() == 3 && part.bytes().all(|b| b.is_ascii_digit());
    (digits(bank) && digits(program) && !name.trim().is_empty()).then_some(line)
}

/// A part of the quick view that a program told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Section {
    /// Names and values.
    Fields { title: String, fields: Vec<(String, String)> },
    /// A list of lines.
    List { title: String, items: Vec<String> },
    /// Running text.
    Text { title: String, text: String },
}

/// The languages exiftool can name its fields in (`exiftool -lang`).
const EXIFTOOL_LANGUAGES: [&str; 19] = [
    "cs", "de", "en", "en-ca", "en-gb", "es", "fi", "fr", "it", "ja", "ko", "nl", "pl", "ru", "sk", "sv", "tr",
    "zh-cn", "zh-tw",
];

/// The first of the user's `languages` (`cs_CZ.UTF-8`, `cs`, as glib lists
/// them) that exiftool knows.
#[must_use]
pub fn exiftool_language<'a>(languages: impl IntoIterator<Item = &'a str>) -> Option<&'static str> {
    languages.into_iter().find_map(|name| {
        let name = name.split(['.', '@']).next().unwrap_or_default().to_ascii_lowercase().replace('_', "-");
        let short = name.split('-').next().unwrap_or_default().to_owned();
        [name, short].into_iter().find_map(|name| EXIFTOOL_LANGUAGES.into_iter().find(|known| *known == name))
    })
}

/// A thumbnailer the desktop would need for a picture of `mime`, with
/// what it draws; for when no installed package offers one.
#[must_use]
pub fn known_thumbnailer(mime: &str) -> Option<(&'static str, String)> {
    const DOCUMENTS: [&str; 8] = [
        "application/pdf",
        "application/postscript",
        "application/oxps",
        "application/vnd.ms-xpsdocument",
        "application/vnd.comicbook+zip",
        "application/vnd.comicbook-rar",
        "application/x-cb7",
        "application/x-cbt",
    ];
    let office = mime.starts_with("application/vnd.oasis.opendocument.")
        || mime.starts_with("application/vnd.openxmlformats-officedocument.")
        || ["application/msword", "application/vnd.ms-excel", "application/vnd.ms-powerpoint"].contains(&mime);
    if DOCUMENTS.contains(&mime) {
        Some(("evince-thumbnailer", gettext("picture of the first page")))
    } else if office {
        Some(("gsf-office-thumbnailer", gettext("picture of the document")))
    } else if ["application/epub+zip", "application/x-mobipocket-ebook"].contains(&mime) {
        Some(("gnome-epub-thumbnailer", gettext("cover of the book")))
    } else if mime.starts_with("font/") || mime.starts_with("application/x-font") {
        Some(("gnome-thumbnail-font", gettext("sample of the font")))
    } else {
        None
    }
}

/// A program the quick view would use, missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub program: String,
    /// What it would add.
    pub adds: String,
}

/// The package managers whose package names are known here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distribution {
    /// Arch Linux and derivatives (pacman).
    Arch,
    /// Debian, Ubuntu and derivatives (apt).
    Debian,
    /// Fedora, RHEL and derivatives (dnf).
    Fedora,
    /// openSUSE (zypper).
    Suse,
}

impl Distribution {
    /// From `/etc/os-release`: its `ID`, then the families in `ID_LIKE`.
    #[must_use]
    pub fn from_os_release(text: &str) -> Option<Self> {
        let value = |key: &str| {
            text.lines()
                .find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='))
                .map(|value| value.trim().trim_matches(['"', '\'']).to_owned())
                .unwrap_or_default()
        };
        let (id, like) = (value("ID"), value("ID_LIKE"));
        std::iter::once(id.as_str()).chain(like.split_whitespace()).find_map(|id| match id {
            "arch" => Some(Self::Arch),
            "debian" | "ubuntu" => Some(Self::Debian),
            "fedora" | "rhel" | "centos" => Some(Self::Fedora),
            "opensuse" | "suse" | "sles" => Some(Self::Suse),
            _ if id.starts_with("opensuse") => Some(Self::Suse),
            _ => None,
        })
    }

    /// The package with `program`; `None` where it is not known.
    #[must_use]
    pub fn package(self, program: &str) -> Option<&'static str> {
        Some(match (program, self) {
            ("exiftool", Self::Arch) => "perl-image-exiftool",
            ("exiftool", Self::Debian) => "libimage-exiftool-perl",
            ("exiftool", Self::Fedora) => "perl-Image-ExifTool",
            ("exiftool", Self::Suse) => "exiftool",
            ("fluidsynth", _) => "fluidsynth",
            ("evince-thumbnailer", Self::Fedora) => "evince-thumbnailer",
            ("evince-thumbnailer", _) => "evince",
            ("gsf-office-thumbnailer", Self::Arch | Self::Fedora) => "libgsf",
            ("gsf-office-thumbnailer", Self::Debian) => "libgsf-bin",
            ("gnome-thumbnail-font", _) => "gnome-font-viewer",
            ("gnome-epub-thumbnailer", _) => "gnome-epub-thumbnailer",
            ("pdftoppm" | "pdfinfo", Self::Arch) => "poppler",
            ("pdftoppm" | "pdfinfo", Self::Debian | Self::Fedora) => "poppler-utils",
            ("pdftoppm" | "pdfinfo", Self::Suse) => "poppler-tools",
            ("pandoc", Self::Arch) => "pandoc-cli",
            ("pandoc", _) => "pandoc",
            _ => return None,
        })
    }

    /// The command that installs `packages`.
    #[must_use]
    pub fn install_command(self, packages: &[&str]) -> String {
        let manager = match self {
            Self::Arch => "sudo pacman -S --needed",
            Self::Debian => "sudo apt install",
            Self::Fedora => "sudo dnf install",
            Self::Suse => "sudo zypper install",
        };
        format!("{manager} {}", packages.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fluidsynth_presets() {
        let output = "Type 'help' for help topics.\n\n> inst 1\n000-000 Piano 1\n000-001 Piano 2\n128-000 Standard\n> ";
        let sections = InfoTool::Fluidsynth.parse(output);
        let [Section::List { items, .. }] = sections.as_slice() else { panic!("no list") };
        assert_eq!(items, &["000-000 Piano 1", "000-001 Piano 2", "128-000 Standard"]);
        assert_eq!(InfoTool::Fluidsynth.parse("> inst 1\n> "), []);
        assert_eq!(preset("> 000-000 Piano 1"), Some("000-000 Piano 1"));
        assert_eq!(preset("0-0 x"), None);
    }

    #[test]
    fn exiftool_fields() {
        let output = "Název\tSound Canvas Pure.sf2\nDatum a čas generování digitálních dat\t2026:10:01 07:35\nPrázdné\t\n";
        let sections = InfoTool::Exiftool.parse(output);
        let [Section::Fields { fields, .. }] = sections.as_slice() else { panic!("no fields") };
        assert_eq!(
            fields,
            &[
                ("Název".to_owned(), "Sound Canvas Pure.sf2".to_owned()),
                ("Datum a čas generování digitálních dat".to_owned(), "2026:10:01 07:35".to_owned()),
            ]
        );
        assert_eq!(InfoTool::Exiftool.parse(""), []);
    }

    #[test]
    fn pandoc_book() {
        let output = "title\tA House Divided\nauthor\tAdam Yoshida, Someone\npublisher\t\ndate\t2014\nlanguage\ten\n\
            description\tFirst paragraph.\n\nSecond.\n\x01\n[]\n\n[]\n\nContents\n\n\n\nChapter One\n";
        let sections = InfoTool::Pandoc.parse(output);
        let [Section::Fields { fields, .. }, Section::Text { text, .. }] = sections.as_slice() else { panic!("{sections:?}") };
        let values: Vec<&str> = fields.iter().map(|(_, value)| value.as_str()).collect();
        assert_eq!(values, ["A House Divided", "Adam Yoshida, Someone", "2014", "en", "First paragraph.\n\nSecond."]);
        assert_eq!(text, "Contents\n\nChapter One");
    }

    #[test]
    fn pandoc_document_without_metadata() {
        let sections = InfoTool::Pandoc.parse("title\t\nauthor\t\n\x01\nHello.\n");
        assert_eq!(sections, [Section::Text { title: "Beginning of the text".to_owned(), text: "Hello.".to_owned() }]);
    }

    #[test]
    fn long_text_ends_after_a_whole_paragraph() {
        let paragraph = "word ".repeat(200);
        let body = [paragraph.trim(); 10].join("\n\n");
        let text = text_beginning(&body);
        assert!(text.ends_with("\n\n…"));
        assert!(text.chars().count() <= TEXT_BEGINNING + 3);
        assert_eq!(text_beginning(&"x".repeat(5000)).len(), 5000, "one long paragraph stays whole");
    }

    #[test]
    fn pandoc_arguments() {
        let args = InfoTool::Pandoc.args(OsStr::new("book.zip"), "application/epub+zip", None);
        assert_eq!(args, ["-f", "epub", "-t", "plain", "--wrap=none", "--template=/dev/stdin", "--", "book.zip"]);
        assert!(!InfoTool::Pandoc.applies("application/zip"));
    }

    #[test]
    fn exiftool_arguments() {
        let args = InfoTool::Exiftool.args(OsStr::new("-f.sf2"), "", Some("cs"));
        assert_eq!(args, ["-t", "-lang", "cs", "--File:all", "--ExifTool:all", "--ZIP:all", "--", "-f.sf2"]);
        assert_eq!(InfoTool::Exiftool.args(OsStr::new("f"), "", None).len(), 6);
    }

    #[test]
    fn languages_of_exiftool() {
        assert_eq!(exiftool_language(["cs_CZ.UTF-8", "cs_CZ", "cs", "C"]), Some("cs"));
        assert_eq!(exiftool_language(["en_GB.UTF-8"]), Some("en-gb"));
        assert_eq!(exiftool_language(["zh_TW"]), Some("zh-tw"));
        assert_eq!(exiftool_language(["C", "POSIX"]), None);
    }

    #[test]
    fn thumbnailers_to_install() {
        assert_eq!(known_thumbnailer("application/pdf").map(|(program, _)| program), Some("evince-thumbnailer"));
        assert_eq!(
            known_thumbnailer("application/vnd.oasis.opendocument.text").map(|(program, _)| program),
            Some("gsf-office-thumbnailer")
        );
        assert_eq!(known_thumbnailer("font/ttf").map(|(program, _)| program), Some("gnome-thumbnail-font"));
        assert_eq!(known_thumbnailer("application/octet-stream"), None);
    }

    #[test]
    fn distributions() {
        assert_eq!(Distribution::from_os_release("NAME=\"Arch Linux\"\nID=arch\n"), Some(Distribution::Arch));
        assert_eq!(Distribution::from_os_release("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n"), Some(Distribution::Debian));
        assert_eq!(Distribution::from_os_release("ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n"), Some(Distribution::Fedora));
        assert_eq!(Distribution::from_os_release("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n"), Some(Distribution::Suse));
        assert_eq!(Distribution::from_os_release("ID=gentoo\n"), None);
        // ID_LIKE is not ID.
        assert_eq!(Distribution::from_os_release("ID_LIKE=arch\nID=endeavouros\n"), Some(Distribution::Arch));
    }

    #[test]
    fn install_commands() {
        let arch = Distribution::Arch;
        let packages: Vec<&str> = ["exiftool", "fluidsynth"].iter().filter_map(|p| arch.package(p)).collect();
        assert_eq!(arch.install_command(&packages), "sudo pacman -S --needed perl-image-exiftool fluidsynth");
        assert_eq!(Distribution::Debian.install_command(&["libimage-exiftool-perl"]), "sudo apt install libimage-exiftool-perl");
        assert_eq!(Distribution::Suse.package("gsf-office-thumbnailer"), None);
    }
}

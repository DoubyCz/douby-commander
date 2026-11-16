//! Add-ons: programs of the distribution that functions of Douby Commander
//! use when they are installed (the quick view, the search in the text of
//! documents). What the original did with plugins, the package manager of
//! the system offers here; we only know which packages bring what.
//!
//! [`Feature`] lists what each function can use and why, [`Distribution`]
//! knows the packages and the command of its package manager, and
//! [`InstallQueue`] keeps what the user chose to install later, in
//! `~/.config/doubycmd/addons.toml`.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use dc_i18n::{fill, gettext};
use serde::{Deserialize, Serialize};

use crate::find::{DOCUMENT_TOOLS, tool_adds};
use crate::preview::pages::PageRenderer;
use crate::preview::tools::InfoTool;
use crate::program;

/// A program a function would use, missing (or offered).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub program: String,
    /// What it adds to the function, in its words.
    pub adds: String,
}

/// A function that can use add-ons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// F3, the quick view.
    QuickView,
    /// Alt+F7 with "Text of documents".
    DocumentText,
}

impl Feature {
    pub const ALL: [Self; 2] = [Self::QuickView, Self::DocumentText];

    #[must_use]
    pub fn title(self) -> String {
        match self {
            Self::QuickView => gettext("Quick view (F3)"),
            Self::DocumentText => gettext("Find files: text of documents"),
        }
    }

    /// Every add-on the function can use, with what it adds.
    #[must_use]
    pub fn add_ons(self) -> Vec<Hint> {
        let hint = |program: &str, adds: String| Hint { program: program.to_owned(), adds };
        match self {
            Self::QuickView => {
                let mut add_ons = vec![
                    hint("nvim", gettext("text and source code with syntax highlighting")),
                    hint(PageRenderer::Pdf.render_program(), gettext("PDF documents page by page")),
                    hint(InfoTool::Fluidsynth.program(), gettext("list of instruments in SoundFont banks")),
                    hint(InfoTool::Pandoc.program(), gettext("beginning of the text of books and documents (EPUB, FB2, DOCX, ODT)")),
                    hint(InfoTool::Exiftool.program(), gettext("metadata of almost any file: documents, pictures, fonts, programs")),
                ];
                add_ons.extend([
                    hint("evince-thumbnailer", gettext("picture of the first page of PDF and comics")),
                    hint("gsf-office-thumbnailer", gettext("picture of office documents")),
                    hint("gnome-epub-thumbnailer", gettext("cover of e-books")),
                    hint("gnome-thumbnail-font", gettext("sample of fonts")),
                ]);
                add_ons
            }
            Self::DocumentText => DOCUMENT_TOOLS.iter().map(|tool| hint(tool, tool_adds(tool))).collect(),
        }
    }
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
    pub const ALL: [Self; 4] = [Self::Arch, Self::Debian, Self::Fedora, Self::Suse];

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

    /// The family and its package manager, for the user.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Arch => "Arch Linux (pacman)",
            Self::Debian => "Debian / Ubuntu (apt)",
            Self::Fedora => "Fedora (dnf)",
            Self::Suse => "openSUSE (zypper)",
        }
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
            ("gsf-office-thumbnailer", Self::Suse) => "gsf-office-thumbnailer",
            ("gnome-thumbnail-font", _) => "gnome-font-viewer",
            ("gnome-epub-thumbnailer", _) => "gnome-epub-thumbnailer",
            ("pdftoppm" | "pdfinfo" | "pdftotext", Self::Arch) => "poppler",
            ("pdftoppm" | "pdfinfo" | "pdftotext", Self::Debian | Self::Fedora) => "poppler-utils",
            ("pdftoppm" | "pdfinfo" | "pdftotext", Self::Suse) => "poppler-tools",
            ("djvutxt", Self::Debian) => "djvulibre-bin",
            ("djvutxt", _) => "djvulibre",
            ("catdoc", _) => "catdoc",
            ("pandoc", Self::Arch) => "pandoc-cli",
            ("pandoc", _) => "pandoc",
            ("nvim", _) => "neovim",
            _ => return None,
        })
    }

    /// The package manager with its arguments that install `packages`
    /// without asking again (the user chose them already), to run with
    /// the rights of root ([`elevation`]).
    #[must_use]
    pub fn install_args(self, packages: &[&str]) -> Vec<String> {
        let manager: &[&str] = match self {
            Self::Arch => &["pacman", "-S", "--needed", "--noconfirm"],
            Self::Debian => &["apt-get", "install", "-y"],
            Self::Fedora => &["dnf", "install", "-y"],
            Self::Suse => &["zypper", "--non-interactive", "install"],
        };
        manager.iter().chain(packages).map(|part| (*part).to_owned()).collect()
    }
}

/// The distribution this runs on, from `/etc/os-release`.
#[must_use]
pub fn distribution() -> Option<Distribution> {
    let os_release = fs::read_to_string("/etc/os-release").or_else(|_| fs::read_to_string("/usr/lib/os-release"));
    Distribution::from_os_release(&os_release.ok()?)
}

/// What gives a command the rights of root, asking for the password in a
/// terminal: `sudo`, else systemd's `run0`, else polkit's `pkexec`.
#[must_use]
pub fn elevation() -> Option<&'static str> {
    ["sudo", "run0", "pkexec"].into_iter().find(|program| program::find(program).is_some())
}

/// Whether `program` is installed (on `$PATH`).
#[must_use]
pub fn is_installed(program: &str) -> bool {
    program::find(program).is_some()
}

/// An add-on chosen for installing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub program: String,
    pub package: String,
    /// The function it is for and why, as shown when it was chosen.
    pub feature: String,
    pub purpose: String,
}

impl Request {
    /// A request for `hint` of the function `feature` on `distribution`;
    /// `None` when the package is not known there.
    #[must_use]
    pub fn new(distribution: Distribution, feature: &str, hint: &Hint) -> Option<Self> {
        Some(Self {
            program: hint.program.clone(),
            package: distribution.package(&hint.program)?.to_owned(),
            feature: feature.to_owned(),
            purpose: hint.adds.clone(),
        })
    }
}

/// Add-ons waiting to be installed, for the whole application.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallQueue {
    #[serde(default, rename = "addon")]
    items: Vec<Request>,
}

/// The queue file cannot be read or written.
#[derive(Debug)]
pub struct QueueError {
    pub path: PathBuf,
    pub reason: String,
}

impl fmt::Display for QueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&fill(&gettext("The queue of add-ons in {path} cannot be used"), &[("path", &self.path.display())]))
    }
}

impl std::error::Error for QueueError {}

impl InstallQueue {
    /// The queue kept in `path`, empty when there is no such file.
    ///
    /// # Errors
    /// When the file is there but cannot be read or understood.
    pub fn load(path: &Path) -> Result<Self, QueueError> {
        let error = |reason: String| QueueError { path: path.to_owned(), reason };
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|err| error(err.to_string())),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(error(err.to_string())),
        }
    }

    /// Writes the queue to `path` (an empty one removes the file).
    ///
    /// # Errors
    /// When the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), QueueError> {
        let error = |reason: String| QueueError { path: path.to_owned(), reason };
        if self.items.is_empty() {
            return match fs::remove_file(path) {
                Err(err) if err.kind() != io::ErrorKind::NotFound => Err(error(err.to_string())),
                _ => Ok(()),
            };
        }
        let text = toml::to_string_pretty(self).map_err(|err| error(err.to_string()))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|err| error(err.to_string()))?;
        }
        let new = path.with_extension("toml.new");
        fs::write(&new, text).and_then(|()| fs::rename(&new, path)).map_err(|err| error(err.to_string()))
    }

    #[must_use]
    pub fn items(&self) -> &[Request] {
        &self.items
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn contains(&self, program: &str) -> bool {
        self.items.iter().any(|item| item.program == program)
    }

    /// Adds `requests`; a program queued already stays once.
    pub fn add(&mut self, requests: impl IntoIterator<Item = Request>) {
        for request in requests {
            if !self.contains(&request.program) {
                self.items.push(request);
            }
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Leaves out what `installed` says is there now.
    pub fn retain_missing(&mut self, installed: impl Fn(&str) -> bool) {
        self.items.retain(|item| !installed(&item.program));
    }

    /// The packages to install, each once, in the order queued.
    #[must_use]
    pub fn packages(&self) -> Vec<&str> {
        packages(&self.items)
    }
}

/// The packages of `requests`, each once, in their order.
#[must_use]
pub fn packages(requests: &[Request]) -> Vec<&str> {
    let mut packages: Vec<&str> = Vec::new();
    for request in requests {
        if !packages.contains(&request.package.as_str()) {
            packages.push(&request.package);
        }
    }
    packages
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(arch.package("exiftool"), Some("perl-image-exiftool"));
        assert_eq!(Distribution::Suse.package("surely-not-known"), None);
        assert_eq!(arch.install_args(&["poppler", "catdoc"]), ["pacman", "-S", "--needed", "--noconfirm", "poppler", "catdoc"]);
        assert_eq!(Distribution::Debian.install_args(&["catdoc"]), ["apt-get", "install", "-y", "catdoc"]);
        assert_eq!(Distribution::Suse.install_args(&["catdoc"]), ["zypper", "--non-interactive", "install", "catdoc"]);
    }

    #[test]
    fn every_add_on_has_a_package_everywhere() {
        for distribution in Distribution::ALL {
            for feature in Feature::ALL {
                for hint in feature.add_ons() {
                    assert!(distribution.package(&hint.program).is_some(), "{} on {distribution:?}", hint.program);
                }
            }
        }
        assert_eq!(Distribution::Debian.package("djvutxt"), Some("djvulibre-bin"));
    }

    fn request(program: &str, package: &str) -> Request {
        Request { program: program.to_owned(), package: package.to_owned(), feature: "F3".to_owned(), purpose: "x".to_owned() }
    }

    #[test]
    fn queue_keeps_each_program_once_and_forgets_installed_ones() {
        let mut queue = InstallQueue::default();
        queue.add([request("pdftotext", "poppler"), request("pdftoppm", "poppler"), request("catdoc", "catdoc")]);
        queue.add([request("pdftotext", "poppler")]);
        assert_eq!(queue.items().len(), 3);
        assert_eq!(queue.packages(), ["poppler", "catdoc"]);
        queue.retain_missing(|program| program == "catdoc");
        assert_eq!(queue.packages(), ["poppler"]);
        assert!(queue.contains("pdftoppm") && !queue.contains("catdoc"));

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("doubycmd/addons.toml");
        assert_eq!(InstallQueue::load(&path).unwrap(), InstallQueue::default());
        queue.save(&path).unwrap();
        assert_eq!(InstallQueue::load(&path).unwrap(), queue);
        queue.clear();
        queue.save(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn requests_need_a_known_package() {
        let hint = Hint { program: "pdftotext".to_owned(), adds: "text of PDF".to_owned() };
        let made = Request::new(Distribution::Fedora, "Find", &hint).unwrap();
        assert_eq!(made.package, "poppler-utils");
        let unknown = Hint { program: "nothing-like-it".to_owned(), adds: String::new() };
        assert_eq!(Request::new(Distribution::Arch, "Find", &unknown), None);
    }
}

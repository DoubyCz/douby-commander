//! Documents as pages under each other, drawn one by one by programs of
//! the system: first how many pages there are and how large, then each
//! page as it comes into view.

use std::ffi::{OsStr, OsString};

use dc_i18n::gettext;

/// The size of a page in points, turned as it is shown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSize {
    pub width: f64,
    pub height: f64,
}

/// Programs that draw the pages of a type of documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageRenderer {
    /// Poppler: `pdfinfo` and `pdftoppm`.
    Pdf,
}

impl PageRenderer {
    #[must_use]
    pub fn for_mime(mime: &str) -> Option<Self> {
        (mime == "application/pdf").then_some(Self::Pdf)
    }

    /// The program that tells the pages and their sizes.
    #[must_use]
    pub fn info_program(self) -> &'static str {
        match self {
            Self::Pdf => "pdfinfo",
        }
    }

    /// The program that draws a page.
    #[must_use]
    pub fn render_program(self) -> &'static str {
        match self {
            Self::Pdf => "pdftoppm",
        }
    }

    /// What it adds, for the hint when it is missing.
    #[must_use]
    pub fn adds(self) -> String {
        gettext("pages of the document")
    }

    /// Arguments of [`Self::info_program`] for the absolute `path`.
    #[must_use]
    pub fn info_args(self, path: &OsStr) -> Vec<OsString> {
        match self {
            // Every page (a last page beyond the end is the end).
            Self::Pdf => vec!["-f".into(), "1".into(), "-l".into(), i32::MAX.to_string().into(), path.to_owned()],
        }
    }

    /// The sizes of the pages from the output of [`Self::info_program`].
    #[must_use]
    pub fn parse_pages(self, output: &str) -> Vec<PageSize> {
        match self {
            Self::Pdf => parse_pdfinfo(output),
        }
    }

    /// Arguments of [`Self::render_program`] that write page `page` (from
    /// 1) of the absolute `path` as PNG `width` pixels wide to stdout.
    #[must_use]
    pub fn render_args(self, path: &OsStr, page: usize, width: u32) -> Vec<OsString> {
        match self {
            Self::Pdf => {
                let page = page.to_string();
                let mut args: Vec<OsString> = ["-f", &page, "-l", &page, "-scale-to-x", &width.to_string()]
                    .map(Into::into)
                    .to_vec();
                args.extend(["-scale-to-y", "-1", "-png", "-singlefile"].map(Into::into));
                args.push(path.to_owned());
                args
            }
        }
    }
}

/// `Page    1 size:  596 x 842 pts (A4)` and `Page    1 rot:   90` for each
/// page; without them `Pages:` times `Page size:` (an old pdfinfo).
fn parse_pdfinfo(output: &str) -> Vec<PageSize> {
    let size = |text: &str| {
        let mut numbers = text.split_whitespace().filter_map(|word| word.parse::<f64>().ok());
        Some(PageSize { width: numbers.next()?, height: numbers.next()? })
    };
    let mut pages: Vec<PageSize> = Vec::new();
    let (mut count, mut common, mut common_turned) = (0, None, false);
    for line in output.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let words: Vec<&str> = key.split_whitespace().collect();
        match words.as_slice() {
            ["Page", number, "size"] if number.parse::<usize>().is_ok() => pages.extend(size(value)),
            ["Page", number, "rot"] if number.parse::<usize>().is_ok() => {
                if let Some(last) = pages.last_mut().filter(|_| turned(value)) {
                    *last = PageSize { width: last.height, height: last.width };
                }
            }
            ["Pages"] => count = value.trim().parse().unwrap_or(0),
            ["Page", "size"] => common = size(value),
            ["Page", "rot"] => common_turned = turned(value),
            _ => {}
        }
    }
    if pages.is_empty()
        && let Some(common) = common
    {
        let common = if common_turned { PageSize { width: common.height, height: common.width } } else { common };
        pages = vec![common; count];
    }
    pages
}

/// A rotation by 90 or 270 degrees.
fn turned(value: &str) -> bool {
    value.trim().parse::<i32>().is_ok_and(|degrees| degrees.rem_euclid(180) == 90)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A4: PageSize = PageSize { width: 596.0, height: 842.0 };

    #[test]
    fn pages_of_pdfinfo() {
        let output = "Pages:           3\nPage size:       596 x 842 pts (A4)\nPage rot:        0\n\
            Page    1 size:  596 x 842 pts (A4)\nPage    1 rot:   0\n\
            Page    2 size:  596 x 842 pts (A4)\nPage    2 rot:   90\n\
            Page    3 size:  612 x 792 pts (letter)\nPage    3 rot:   0\n";
        assert_eq!(
            PageRenderer::Pdf.parse_pages(output),
            [A4, PageSize { width: 842.0, height: 596.0 }, PageSize { width: 612.0, height: 792.0 }]
        );
    }

    #[test]
    fn pages_without_sizes_of_each() {
        let output = "Pages:           2\nPage size:       596 x 842 pts (A4)\nPage rot:        270\n";
        assert_eq!(PageRenderer::Pdf.parse_pages(output), [PageSize { width: 842.0, height: 596.0 }; 2]);
        assert_eq!(PageRenderer::Pdf.parse_pages("Syntax Error: broken\n"), []);
    }

    #[test]
    fn arguments() {
        let path = OsStr::new("/a b.pdf");
        assert_eq!(
            PageRenderer::Pdf.render_args(path, 3, 1200),
            ["-f", "3", "-l", "3", "-scale-to-x", "1200", "-scale-to-y", "-1", "-png", "-singlefile", "/a b.pdf"]
        );
        assert_eq!(PageRenderer::Pdf.info_args(path).last().map(OsString::as_os_str), Some(path));
        assert_eq!(PageRenderer::for_mime("application/pdf"), Some(PageRenderer::Pdf));
        assert_eq!(PageRenderer::for_mime("application/zip"), None);
    }
}

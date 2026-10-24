//! How the quick view (F3) shows a file: as text, an image, a player of
//! video and music, or only what it is.
//!
//! The decision takes the MIME type the desktop guesses from the name and
//! the first bytes, and those bytes themselves: a file of an unknown type
//! that reads as text is text, a "text" file with NUL bytes is not.
//!
//! Files no viewer here can show are described by the programs of the
//! system ([`tools`], [`thumbnailer`]): as much as those that are installed
//! tell, with the others named, so that whoever needs more knows what to
//! install.

pub mod pages;
pub mod thumbnailer;
pub mod tools;

/// How many bytes from the start of a file decide.
pub const HEAD_SIZE: usize = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    Text,
    Image,
    Video,
    Audio,
    /// Binary data, or a type nothing here can show.
    Other,
}

/// A SoundFont, a bank of instruments for MIDI. The desktop knows it only
/// as RIFF data, SoundFont 3 as `audio/x.sf3`.
pub const SOUNDFONT: &str = "audio/x-sfbk";

/// The MIME type of `head` where the content tells more than the desktop's
/// guess.
#[must_use]
pub fn sniff(head: &[u8]) -> Option<&'static str> {
    (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"sfbk".as_slice())).then_some(SOUNDFONT)
}

/// Whether `mime` is a SoundFont (2 or 3).
#[must_use]
pub fn is_soundfont(mime: &str) -> bool {
    mime == SOUNDFONT || mime == "audio/x.sf3"
}

/// `mime` as the desktop guessed it (or [`sniff`] found), `text_type` whether that type is a
/// kind of `text/plain` (scripts, sources, configuration), `head` the
/// first [`HEAD_SIZE`] bytes of the file (all of it when shorter).
#[must_use]
pub fn classify(mime: &str, text_type: bool, head: &[u8]) -> PreviewKind {
    let (top, _) = mime.split_once('/').unwrap_or((mime, ""));
    match top {
        // Audio, but nothing to play: samples for a synthesizer.
        _ if is_soundfont(mime) => PreviewKind::Other,
        "image" => PreviewKind::Image,
        "video" => PreviewKind::Video,
        "audio" => PreviewKind::Audio,
        _ if head.contains(&0) => PreviewKind::Other,
        _ if text_type || is_utf8_start(head) => PreviewKind::Text,
        _ => PreviewKind::Other,
    }
}

/// Valid UTF-8, except for a character cut off at the end of the head.
fn is_utf8_start(head: &[u8]) -> bool {
    match std::str::from_utf8(head) {
        Ok(_) => true,
        Err(err) => err.error_len().is_none() && head.len() == HEAD_SIZE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_by_type() {
        assert_eq!(classify("image/png", false, b"\x89PNG\0"), PreviewKind::Image);
        assert_eq!(classify("image/svg+xml", true, b"<svg>"), PreviewKind::Image);
        assert_eq!(classify("video/mp4", false, b"\0\0"), PreviewKind::Video);
        assert_eq!(classify("audio/flac", false, b"fLaC\0"), PreviewKind::Audio);
    }

    #[test]
    fn soundfont_by_content() {
        assert_eq!(sniff(b"RIFF\x10\0\0\0sfbkLIST"), Some(SOUNDFONT));
        assert_eq!(sniff(b"RIFF\x10\0\0\0WAVEfmt "), None);
        assert_eq!(sniff(b"RIFF"), None);
        assert_eq!(classify(SOUNDFONT, false, b"RIFF\0"), PreviewKind::Other);
        assert_eq!(classify("audio/x.sf3", false, b"RIFF\0"), PreviewKind::Other);
    }

    #[test]
    fn text_by_type_or_content() {
        assert_eq!(classify("text/x-rust", true, b"fn main() {}"), PreviewKind::Text);
        assert_eq!(classify("application/x-zerosize", false, b""), PreviewKind::Text);
        assert_eq!(classify("application/octet-stream", false, "Žluťoučký kůň".as_bytes()), PreviewKind::Text);
        assert_eq!(classify("text/plain", true, b"text\0with a zero"), PreviewKind::Other);
        assert_eq!(classify("application/octet-stream", false, b"\xff\xfe\x00"), PreviewKind::Other);
        assert_eq!(classify("application/octet-stream", false, b"latin-1 \xe9t\xe9"), PreviewKind::Other);
    }

    #[test]
    fn character_cut_off_at_the_end_of_the_head() {
        let mut head = vec![b'a'; HEAD_SIZE - 1];
        head.push(0xc5);
        assert_eq!(classify("application/octet-stream", false, &head), PreviewKind::Text);
        assert_eq!(classify("application/octet-stream", false, &head[1..]), PreviewKind::Other, "short file");
    }
}

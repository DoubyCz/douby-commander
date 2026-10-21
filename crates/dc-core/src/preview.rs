//! How the quick view (F3) shows a file: as text, an image, a player of
//! video and music, or only what it is.
//!
//! The decision takes the MIME type the desktop guesses from the name and
//! the first bytes, and those bytes themselves: a file of an unknown type
//! that reads as text is text, a "text" file with NUL bytes is not.

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

/// `mime` as the desktop guessed it, `text_type` whether that type is a
/// kind of `text/plain` (scripts, sources, configuration), `head` the
/// first [`HEAD_SIZE`] bytes of the file (all of it when shorter).
#[must_use]
pub fn classify(mime: &str, text_type: bool, head: &[u8]) -> PreviewKind {
    let (top, _) = mime.split_once('/').unwrap_or((mime, ""));
    match top {
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

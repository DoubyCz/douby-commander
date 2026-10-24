//! Thumbnailers of the desktop. Every package that can draw a picture of
//! some types of files says so in a `.thumbnailer` file in
//! `<data dir>/thumbnailers` (freedesktop thumbnail specification): the
//! MIME types and the command line. File managers share them, so whatever
//! the system can draw, the quick view can show.

/// One `.thumbnailer` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnailer {
    try_exec: Option<String>,
    exec: Vec<String>,
    mime_types: Vec<String>,
}

impl Thumbnailer {
    /// The `[Thumbnailer Entry]` group of a `.thumbnailer` file; `None`
    /// without a command line.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut in_entry = false;
        let (mut try_exec, mut exec, mut mime_types) = (None, None, Vec::new());
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                in_entry = line == "[Thumbnailer Entry]";
                continue;
            }
            let Some((key, value)) = line.split_once('=').filter(|_| in_entry && !line.starts_with('#')) else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "TryExec" => try_exec = Some(value.to_owned()).filter(|program| !program.is_empty()),
                "Exec" => exec = split_exec(value),
                "MimeType" => {
                    mime_types = value.split(';').map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned).collect();
                }
                _ => {}
            }
        }
        let exec = exec.filter(|exec| !exec.is_empty())?;
        Some(Self { try_exec, exec, mime_types })
    }

    #[must_use]
    pub fn handles(&self, mime: &str) -> bool {
        self.mime_types.iter().any(|handled| handled.eq_ignore_ascii_case(mime))
    }

    /// The program that has to be installed for it to work.
    #[must_use]
    pub fn program(&self) -> &str {
        self.try_exec.as_deref().or(self.exec.first().map(String::as_str)).unwrap_or_default()
    }

    /// The command line for one file: `%i` its path, `%u` its URI, `%o`
    /// the picture to write (PNG), `%s` the size in pixels, `%%` a percent
    /// sign. Fields may be parts of arguments (`--input=%i`). Unknown fields
    /// are dropped, as in desktop entries, with the argument if nothing
    /// else is left of it.
    #[must_use]
    pub fn command(&self, input: &str, uri: &str, output: &str, size: u32) -> Vec<String> {
        self.exec
            .iter()
            .filter_map(|argument| {
                let mut expanded = String::with_capacity(argument.len());
                let mut chars = argument.chars();
                while let Some(c) = chars.next() {
                    if c != '%' {
                        expanded.push(c);
                        continue;
                    }
                    match chars.next() {
                        Some('i') => expanded.push_str(input),
                        Some('u') => expanded.push_str(uri),
                        Some('o') => expanded.push_str(output),
                        Some('s') => expanded.push_str(&size.to_string()),
                        Some('%') => expanded.push('%'),
                        _ => {}
                    }
                }
                (!expanded.is_empty() || argument.is_empty()).then_some(expanded)
            })
            .collect()
    }
}

/// The arguments of an `Exec` value: separated by spaces, double quotes
/// keep spaces, a backslash in quotes escapes the next character (desktop
/// entry rules). `None` for an unclosed quote.
fn split_exec(value: &str) -> Option<Vec<String>> {
    let mut arguments = Vec::new();
    let mut current: Option<String> = None;
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => arguments.extend(current.take()),
            '"' => {
                let argument = current.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => argument.push(chars.next()?),
                        c => argument.push(c),
                    }
                }
            }
            c => current.get_or_insert_with(String::new).push(c),
        }
    }
    arguments.extend(current);
    Some(arguments)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVINCE: &str = "[Thumbnailer Entry]\n\
        TryExec=evince-thumbnailer\n\
        Exec=evince-thumbnailer -s %s %u %o\n\
        MimeType=application/pdf;application/x-bzpdf;\n";

    #[test]
    fn parses_an_entry() {
        let thumbnailer = Thumbnailer::parse(EVINCE).unwrap();
        assert!(thumbnailer.handles("application/pdf"));
        assert!(!thumbnailer.handles("application/x-pdf"));
        assert_eq!(thumbnailer.program(), "evince-thumbnailer");
        assert_eq!(
            thumbnailer.command("/a b.pdf", "file:///a%20b.pdf", "/tmp/o.png", 512),
            ["evince-thumbnailer", "-s", "512", "file:///a%20b.pdf", "/tmp/o.png"]
        );
    }

    #[test]
    fn program_without_try_exec_is_the_command() {
        let thumbnailer = Thumbnailer::parse("[Thumbnailer Entry]\nExec=/usr/bin/glycin-thumbnailer --input %u\n").unwrap();
        assert_eq!(thumbnailer.program(), "/usr/bin/glycin-thumbnailer");
    }

    #[test]
    fn fields_inside_arguments_and_quotes() {
        let thumbnailer = Thumbnailer::parse("[Thumbnailer Entry]\nExec=\"my thumb\" --in=%i \"say \\\"%%\\\"\" %x\n").unwrap();
        assert_eq!(thumbnailer.command("/f", "file:///f", "/o", 1), ["my thumb", "--in=/f", "say \"%\""]);
    }

    #[test]
    fn other_groups_and_broken_entries() {
        assert_eq!(Thumbnailer::parse("[Other]\nExec=x %i\n"), None);
        assert_eq!(Thumbnailer::parse("[Thumbnailer Entry]\nExec=\"unclosed %i\n"), None);
        assert_eq!(Thumbnailer::parse("[Thumbnailer Entry]\nMimeType=a/b;\n"), None);
    }
}

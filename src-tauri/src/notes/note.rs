//! A single note on disk: reading, encoding, title and revision.
//!
//! - Text is UTF-8. A leading BOM is kept, and so are CRLF line endings: the editor gets LF
//!   text, and [`encode`] puts the file's own BOM and line endings back on write.
//! - A file that isn't valid UTF-8 is flagged and never opened for editing.
//! - Files above [`MAX_NOTE_BYTES`] are never read.
//! - The revision is the SHA-256 of the bytes on disk.
//! - Frontmatter is parsed read-only, only for `title`. Readback never writes it.

use std::borrow::Cow;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::macos::fs::MetadataExt as _;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::errors::Error;
use super::paths;

/// Notes above this size are listed but never read.
pub const MAX_NOTE_BYTES: u64 = 5 * 1024 * 1024;
const BOM: &[u8] = b"\xEF\xBB\xBF";
/// `st_flags` bit for a dataless file (contents evicted to iCloud or another File Provider).
/// Reading such a file makes macOS download it; checking metadata doesn't.
const SF_DATALESS: u32 = 0x4000_0000;

pub fn revision(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn is_dataless(meta: &fs::Metadata) -> bool {
    meta.st_flags() & SF_DATALESS != 0
}

/// How the file on disk writes text, so writes keep it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub bom: bool,
    pub crlf: bool,
}

impl Style {
    pub fn of(bytes: &[u8]) -> Self {
        let bom = bytes.starts_with(BOM);
        let crlf_count = bytes.windows(2).filter(|w| w == b"\r\n").count();
        let lf_count = bytes.iter().filter(|b| **b == b'\n').count();
        // CRLF when most line breaks are CRLF (a mixed file is written back as its majority).
        Self {
            bom,
            crlf: crlf_count > 0 && crlf_count * 2 >= lf_count,
        }
    }
}

/// Editor text from file bytes: BOM removed, CRLF turned into LF. `None` if not UTF-8.
pub fn decode(bytes: &[u8]) -> Option<String> {
    let body = bytes.strip_prefix(BOM).unwrap_or(bytes);
    let text = std::str::from_utf8(body).ok()?;
    Some(if text.contains("\r\n") {
        text.replace("\r\n", "\n")
    } else {
        text.to_owned()
    })
}

/// File bytes from editor text, in the file's style.
pub fn encode(text: &str, style: Style) -> Vec<u8> {
    let normalized: Cow<'_, str> = if text.contains("\r\n") {
        Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(text)
    };
    let mut out = Vec::with_capacity(normalized.len() + 3);
    if style.bom {
        out.extend_from_slice(BOM);
    }
    if style.crlf {
        out.extend_from_slice(normalized.replace('\n', "\r\n").as_bytes());
    } else {
        out.extend_from_slice(normalized.as_bytes());
    }
    out
}

/// Why a note can't be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unreadable {
    TooLarge,
    Offloaded,
    PermissionDenied,
}

/// What reading a note's bytes produced.
pub enum Loaded {
    Text {
        bytes: Vec<u8>,
        text: String,
    },
    /// Bytes were read but aren't UTF-8; the revision is still known.
    Binary {
        bytes: Vec<u8>,
    },
    Skipped(Unreadable),
}

/// Reads a note's bytes, refusing dataless and oversized files before opening them.
pub fn load(path: &Path) -> io::Result<Loaded> {
    let meta = fs::symlink_metadata(path)?;
    if is_dataless(&meta) {
        return Ok(Loaded::Skipped(Unreadable::Offloaded));
    }
    if meta.len() > MAX_NOTE_BYTES {
        return Ok(Loaded::Skipped(Unreadable::TooLarge));
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            return Ok(Loaded::Skipped(Unreadable::PermissionDenied));
        }
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    // One byte over the limit tells us the file grew past it since `stat`.
    file.take(MAX_NOTE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_NOTE_BYTES {
        return Ok(Loaded::Skipped(Unreadable::TooLarge));
    }
    Ok(match decode(&bytes) {
        Some(text) => Loaded::Text { bytes, text },
        None => Loaded::Binary { bytes },
    })
}

pub fn unreadable_error(reason: Unreadable, name: &str) -> Error {
    match reason {
        Unreadable::TooLarge => Error::too_large(name, MAX_NOTE_BYTES),
        Unreadable::Offloaded => Error::offloaded(name),
        Unreadable::PermissionDenied => Error::io(
            &io::Error::from(io::ErrorKind::PermissionDenied),
            "reading",
            name,
        ),
    }
}

/// The note's title: frontmatter `title`, else the first `# ` heading, else the file name
/// without `.md`.
pub fn title(text: &str, file_name: &str) -> String {
    let (frontmatter, body) = split_frontmatter(text);
    frontmatter
        .and_then(frontmatter_title)
        .or_else(|| first_heading(body))
        .unwrap_or_else(|| fallback_title(file_name))
}

pub fn fallback_title(file_name: &str) -> String {
    paths::note_stem(file_name).unwrap_or(file_name).to_owned()
}

/// Splits `---\n…\n---` (or `...`) frontmatter from the body. Text is already LF.
fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (None, text);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n').trim_end();
        if trimmed == "---" || trimmed == "..." {
            return (Some(&rest[..offset]), &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    // No closing fence: not frontmatter.
    (None, text)
}

/// A top-level `title:` scalar: plain, 'single' or "double" quoted. Anything fancier (block
/// scalars, flow collections, anchors, tags) falls back to the heading.
fn frontmatter_title(frontmatter: &str) -> Option<String> {
    let line = frontmatter
        .lines()
        .find_map(|line| line.strip_prefix("title:"))?;
    let value = line.trim();
    let title = if let Some(inner) = value.strip_prefix('"') {
        parse_double_quoted(inner)?
    } else if let Some(inner) = value.strip_prefix('\'') {
        let end = find_single_quote_end(inner)?;
        inner[..end].replace("''", "'")
    } else {
        if value.starts_with(['|', '>', '[', '{', '&', '*', '!', '#', '%', '@', '`']) {
            return None;
        }
        // A plain scalar ends at " #" (a comment).
        let value = value.split(" #").next().unwrap_or(value).trim();
        value.to_owned()
    };
    let title = title.trim().to_owned();
    (!title.is_empty()).then_some(title)
}

fn find_single_quote_end(inner: &str) -> Option<usize> {
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            if bytes.get(i + 1) == Some(&b'\'') {
                i += 2;
                continue;
            }
            return Some(i);
        }
        i += 1;
    }
    None
}

fn parse_double_quoted(inner: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                // Rarer escapes: give up on frontmatter and use the heading instead.
                _ => return None,
            },
            c => out.push(c),
        }
    }
    None
}

/// The first ATX level-1 heading outside fenced code blocks.
fn first_heading(body: &str) -> Option<String> {
    let mut fence: Option<(char, usize)> = None;
    for line in body.lines() {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let trimmed = &line[indent..];
        if indent <= 3 {
            let marker = trimmed.chars().next();
            if let Some(m @ ('`' | '~')) = marker {
                let run = trimmed.chars().take_while(|c| *c == m).count();
                if run >= 3 {
                    match fence {
                        None => fence = Some((m, run)),
                        Some((open, len)) if open == m && run >= len => fence = None,
                        Some(_) => {}
                    }
                    continue;
                }
            }
        }
        if fence.is_some() || indent > 3 {
            continue;
        }
        let rest = if let Some(rest) = trimmed.strip_prefix("# ") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("#\t") {
            rest
        } else {
            continue;
        };
        // Optional closing sequence: " ##".
        let rest = rest.trim();
        let unclosed = rest.trim_end_matches('#');
        let closed =
            unclosed.len() < rest.len() && (unclosed.is_empty() || unclosed.ends_with([' ', '\t']));
        let rest = if closed { unclosed.trim_end() } else { rest };
        if !rest.is_empty() {
            return Some(rest.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::test_support::TempDir;

    #[test]
    fn bom_and_crlf_round_trip_byte_identical() {
        let original = b"\xEF\xBB\xBF# Title\r\n\r\nLine two\r\n".to_vec();
        let style = Style::of(&original);
        assert_eq!(
            style,
            Style {
                bom: true,
                crlf: true
            }
        );
        let text = decode(&original).unwrap();
        assert_eq!(text, "# Title\n\nLine two\n");
        assert_eq!(encode(&text, style), original);
    }

    #[test]
    fn lf_files_stay_lf_and_edits_keep_crlf() {
        let lf = b"a\nb\n";
        assert_eq!(Style::of(lf), Style::default());
        assert_eq!(encode(&decode(lf).unwrap(), Style::of(lf)), lf);

        let crlf = b"a\r\nb\r\n";
        let edited = format!("{}c\n", decode(crlf).unwrap());
        assert_eq!(encode(&edited, Style::of(crlf)), b"a\r\nb\r\nc\r\n");
    }

    #[test]
    fn invalid_utf8_is_flagged() {
        let dir = TempDir::new();
        let path = dir.path().join("latin1.md");
        fs::write(&path, b"caf\xe9\n").unwrap();
        assert!(matches!(load(&path).unwrap(), Loaded::Binary { .. }));
        assert!(decode(b"\xff\xfeU\x00").is_none(), "UTF-16 isn't supported");
    }

    #[test]
    fn oversized_files_are_not_read() {
        let dir = TempDir::new();
        let path = dir.path().join("big.md");
        let file = File::create(&path).unwrap();
        file.set_len(MAX_NOTE_BYTES + 1).unwrap();
        assert!(matches!(
            load(&path).unwrap(),
            Loaded::Skipped(Unreadable::TooLarge)
        ));
    }

    #[test]
    fn revision_is_sha256_of_bytes() {
        assert_eq!(
            revision(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_ne!(revision(b"a\n"), revision(b"a\r\n"));
    }

    #[test]
    fn title_prefers_frontmatter_then_heading_then_file_name() {
        let both = "---\ntitle: From frontmatter\ntags: [a]\n---\n# From heading\n";
        assert_eq!(title(both, "file.md"), "From frontmatter");
        let heading = "---\ntags: [a]\n---\n\nIntro\n# From heading\n";
        assert_eq!(title(heading, "file.md"), "From heading");
        assert_eq!(title("No heading here\n", "My file.md"), "My file");
        assert_eq!(title("", "Empty.MD"), "Empty");
    }

    #[test]
    fn frontmatter_title_forms() {
        let t = |fm: &str| title(&format!("---\n{fm}\n---\n# H\n"), "f.md");
        assert_eq!(t("title: \"Quoted: yes\""), "Quoted: yes");
        assert_eq!(t("title: 'It''s'"), "It's");
        assert_eq!(t("title: Plain # comment"), "Plain");
        assert_eq!(t("title: >\n  folded"), "H");
        assert_eq!(t("title:"), "H");
        assert_eq!(t("  title: nested"), "H");
        assert_eq!(t("subtitle: x"), "H");
        // Unclosed frontmatter isn't frontmatter.
        assert_eq!(title("---\ntitle: x\n# H\n", "f.md"), "H");
    }

    #[test]
    fn heading_rules() {
        assert_eq!(title("```\n# not this\n```\n# This\n", "f.md"), "This");
        assert_eq!(
            title("~~~~\n# no\n~~~\n# still code\n~~~~\n# Yes\n", "f.md"),
            "Yes"
        );
        assert_eq!(title("## Level two\n# Level one\n", "f.md"), "Level one");
        assert_eq!(title("#NoSpace\n", "f.md"), "f");
        assert_eq!(title("   # Indented ##\n", "f.md"), "Indented");
        assert_eq!(title("    # Code block\n", "f.md"), "f");
        assert_eq!(title("# C# tips\n", "f.md"), "C# tips");
    }
}

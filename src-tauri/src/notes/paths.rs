//! Turns paths from the webview into paths on disk, and nothing outside the vault.
//!
//! The webview only ever sends paths relative to the vault root, with `/` separators. Every
//! command resolves them here. A path is rejected when it:
//!
//! - is absolute, or has an empty, `.` or `..` segment;
//! - has a segment starting with `.` (hidden entries are never notes, so the UI never names one);
//! - goes through a symbolic link, wherever it points. The scan never lists links, and refusing
//!   them all means a link can't lead out of the vault;
//! - still ends up outside the root once canonicalised (a second, independent check).
//!
//! Paths that don't exist yet (create, rename target) are checked through their parent, which
//! must exist, plus a separately validated final name.
//!
//! Names on disk are kept exactly as stored. Comparisons (collisions, matching watcher paths)
//! use [`fold`]: NFC normalisation plus lowercasing, because APFS is normalisation-insensitive
//! and, by default, case-insensitive.

use std::borrow::Cow;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use icu_normalizer::ComposingNormalizerBorrowed;

use super::errors::{Error, Result};

/// APFS and HFS+ limit a single name to 255 UTF-8 bytes.
pub const MAX_NAME_BYTES: usize = 255;
pub const NOTE_EXTENSION: &str = "md";
pub const UNTITLED: &str = "Untitled";

/// NFC form of `text`.
pub fn nfc(text: &str) -> Cow<'_, str> {
    ComposingNormalizerBorrowed::new_nfc().normalize(text)
}

/// The key two names collide on: NFC, then lowercase.
pub fn fold(text: &str) -> String {
    nfc(text).to_lowercase()
}

pub fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// `name.md` (any case), with a non-empty stem.
pub fn is_note_name(name: &str) -> bool {
    note_stem(name).is_some()
}

/// The name without its `.md` extension, if it is a note name.
pub fn note_stem(name: &str) -> Option<&str> {
    let dot = name.len().checked_sub(NOTE_EXTENSION.len() + 1)?;
    let (stem, ext) = name.split_at(dot);
    (ext.len() == NOTE_EXTENSION.len() + 1
        && ext.starts_with('.')
        && ext[1..].eq_ignore_ascii_case(NOTE_EXTENSION)
        && !stem.is_empty())
    .then_some(stem)
}

/// The last segment of a relative path (the whole path when it has no `/`).
pub fn file_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// The parent of a relative path; `""` is the vault root.
pub fn parent_rel(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(parent, _)| parent)
}

pub fn join_rel(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

/// True when `rel` is `ancestor` or inside it. `""` contains everything.
pub fn is_within(rel: &str, ancestor: &str) -> bool {
    ancestor.is_empty()
        || rel == ancestor
        || (rel.len() > ancestor.len()
            && rel.starts_with(ancestor)
            && rel.as_bytes()[ancestor.len()] == b'/')
}

/// Cuts `text` to at most `max` bytes without splitting a character.
fn truncate_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Turns a title into a file name stem: NFC, `/` and `:` removed, control characters turned
/// into spaces, leading dots and spaces and trailing spaces trimmed. Never empty.
pub fn sanitize_stem(title: &str) -> String {
    let title = nfc(title);
    let cleaned: String = title
        .chars()
        .filter(|c| *c != '/' && *c != ':')
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut stem = cleaned
        .trim_start_matches(|c: char| c == '.' || c.is_whitespace())
        .trim_end()
        .to_owned();
    // A title ending in ".md" would otherwise become "name.md.md".
    if let Some(inner) = note_stem(&stem) {
        stem = inner.trim_end().to_owned();
    }
    if stem.is_empty() {
        UNTITLED.to_owned()
    } else {
        stem
    }
}

/// Validates a name the user typed for rename (already trimmed). Unlike [`sanitize_stem`], it
/// refuses instead of silently changing what the user asked for.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::invalid_name("Enter a name."));
    }
    if name.contains(['/', ':']) {
        return Err(Error::invalid_name("Names can't contain “/” or “:”."));
    }
    if name.chars().any(char::is_control) {
        return Err(Error::invalid_name(
            "Names can't contain line breaks or tabs.",
        ));
    }
    if is_hidden(name) {
        return Err(Error::invalid_name(
            "Names can't start with “.”: Finder and Readback would hide the item.",
        ));
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(Error::invalid_name("That name is too long."));
    }
    Ok(())
}

/// Picks `stem.ext`, `stem 2.ext`, `stem 3.ext`… so that no entry in `dir` has the same name
/// up to case and Unicode normalisation. Every candidate fits in [`MAX_NAME_BYTES`].
pub fn unique_name(dir: &Path, stem: &str, extension: Option<&str>) -> io::Result<String> {
    let taken: std::collections::HashSet<String> = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().to_str().map(fold))
        .collect();
    let ext = extension.map(|ext| format!(".{ext}")).unwrap_or_default();
    for n in 1u32.. {
        let suffix = if n == 1 {
            String::new()
        } else {
            format!(" {n}")
        };
        let room = MAX_NAME_BYTES - ext.len() - suffix.len();
        let base = truncate_bytes(stem, room).trim_end();
        let base = if base.is_empty() { UNTITLED } else { base };
        let candidate = format!("{base}{suffix}{ext}");
        if !taken.contains(&fold(&candidate)) {
            return Ok(candidate);
        }
    }
    unreachable!("u32 suffixes exhausted")
}

/// True when `dir` holds an entry named like `name` (case- and normalisation-insensitive) that
/// isn't the file `except` (used for case-only renames of the same file).
pub fn name_taken(dir: &Path, name: &str, except: Option<&fs::Metadata>) -> io::Result<bool> {
    let wanted = fold(name);
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let Some(existing) = entry.file_name().to_str().map(fold) else {
            continue;
        };
        if existing != wanted {
            continue;
        }
        let same_file = match except {
            Some(source) => entry
                .metadata()
                .is_ok_and(|meta| meta.dev() == source.dev() && meta.ino() == source.ino()),
            None => false,
        };
        if !same_file {
            return Ok(true);
        }
    }
    Ok(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Folder,
    Note,
}

/// An existing note or folder inside the vault.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub abs: PathBuf,
    pub kind: EntryKind,
}

/// The canonical vault root.
#[derive(Debug, Clone)]
pub struct Root {
    path: PathBuf,
}

impl Root {
    pub fn new(path: &Path) -> io::Result<Self> {
        let path = fs::canonicalize(path)?;
        if !fs::metadata(&path)?.is_dir() {
            return Err(io::Error::from(io::ErrorKind::NotADirectory));
        }
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Splits and validates a webview path. `""` is the root and has no segments.
    pub fn segments(rel: &str) -> Result<Vec<&str>> {
        if rel.is_empty() {
            return Ok(Vec::new());
        }
        if rel.contains('\0') {
            return Err(Error::invalid_path());
        }
        let segments: Vec<&str> = rel.split('/').collect();
        let valid = segments
            .iter()
            .all(|segment| !segment.is_empty() && !is_hidden(segment));
        if !valid {
            return Err(Error::invalid_path());
        }
        // Belt and braces: `Path` must see only normal components.
        let normal = Path::new(rel)
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
        if !normal {
            return Err(Error::invalid_path());
        }
        Ok(segments)
    }

    /// Resolves an existing path. Every component is checked with `lstat`: a missing component
    /// is `NotFound`, a link is `InvalidPath`. The result must canonicalise inside the root.
    fn walk(&self, rel: &str) -> Result<(PathBuf, fs::Metadata)> {
        let segments = Self::segments(rel)?;
        let mut abs = self.path.clone();
        let mut meta = fs::symlink_metadata(&abs).map_err(|_| Error::vault_missing())?;
        for segment in &segments {
            if !meta.is_dir() {
                return Err(Error::not_found(file_name(rel)));
            }
            abs.push(segment);
            meta = match fs::symlink_metadata(&abs) {
                Ok(meta) => meta,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(Error::not_found(file_name(rel)));
                }
                Err(error) => return Err(Error::io(&error, "opening", file_name(rel))),
            };
            if meta.file_type().is_symlink() {
                return Err(Error::invalid_path());
            }
        }
        let canonical = fs::canonicalize(&abs).map_err(|_| Error::invalid_path())?;
        if !canonical.starts_with(&self.path) {
            return Err(Error::invalid_path());
        }
        Ok((abs, meta))
    }

    /// An existing folder (the root is `""`).
    pub fn resolve_dir(&self, rel: &str) -> Result<PathBuf> {
        let (abs, meta) = self.walk(rel)?;
        if !meta.is_dir() {
            return Err(Error::invalid_path());
        }
        Ok(abs)
    }

    /// An existing note or folder, not the root.
    pub fn resolve_entry(&self, rel: &str) -> Result<Resolved> {
        if rel.is_empty() {
            return Err(Error::invalid_path());
        }
        let (abs, meta) = self.walk(rel)?;
        let kind = if meta.is_dir() {
            EntryKind::Folder
        } else if meta.is_file() && is_note_name(file_name(rel)) {
            EntryKind::Note
        } else {
            // Never act on files that aren't notes.
            return Err(Error::invalid_path());
        };
        Ok(Resolved { abs, kind })
    }

    /// An existing note.
    pub fn resolve_note(&self, rel: &str) -> Result<Resolved> {
        let resolved = self.resolve_entry(rel)?;
        if resolved.kind != EntryKind::Note {
            return Err(Error::invalid_path());
        }
        Ok(resolved)
    }

    /// A note that may not exist (any more). Its parent folder must exist and resolve inside the
    /// vault; the note itself must not be a link.
    pub fn resolve_note_slot(&self, rel: &str) -> Result<PathBuf> {
        let segments = Self::segments(rel)?;
        let Some(name) = segments.last() else {
            return Err(Error::invalid_path());
        };
        if !is_note_name(name) {
            return Err(Error::invalid_path());
        }
        let parent = self.resolve_dir(parent_rel(rel))?;
        let abs = parent.join(name);
        match fs::symlink_metadata(&abs) {
            Ok(meta) if !meta.is_file() => Err(Error::invalid_path()),
            _ => Ok(abs),
        }
    }

    /// The relative path of an absolute path reported by the OS, if it is strictly inside the
    /// root and valid UTF-8.
    pub fn rel_of(&self, abs: &Path) -> Option<String> {
        let rest = abs.strip_prefix(&self.path).ok()?;
        let mut parts = Vec::new();
        for component in rest.components() {
            match component {
                Component::Normal(part) => parts.push(part.to_str()?),
                _ => return None,
            }
        }
        (!parts.is_empty()).then(|| parts.join("/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::errors::ErrorKind;
    use crate::notes::test_support::TempDir;
    use std::os::unix::fs::symlink;

    fn vault() -> (TempDir, Root) {
        let dir = TempDir::new();
        fs::create_dir_all(dir.path().join("vault/Projects")).unwrap();
        fs::write(dir.path().join("vault/Projects/Plan.md"), "# Plan").unwrap();
        fs::write(dir.path().join("vault/image.png"), [0u8; 4]).unwrap();
        fs::write(dir.path().join("outside.md"), "secret").unwrap();
        let root = Root::new(&dir.path().join("vault")).unwrap();
        (dir, root)
    }

    fn kind(result: Result<impl std::fmt::Debug>) -> ErrorKind {
        result.unwrap_err().kind
    }

    #[test]
    fn resolves_notes_and_folders() {
        let (_dir, root) = vault();
        let note = root.resolve_entry("Projects/Plan.md").unwrap();
        assert_eq!(note.kind, EntryKind::Note);
        assert!(note.abs.starts_with(root.path()));
        assert_eq!(
            root.resolve_entry("Projects").unwrap().kind,
            EntryKind::Folder
        );
        assert_eq!(root.resolve_dir("").unwrap(), root.path());
    }

    #[test]
    fn rejects_parent_segments_and_absolute_paths() {
        let (_dir, root) = vault();
        for bad in [
            "../outside.md",
            "Projects/../../outside.md",
            "Projects/..",
            "..",
            ".",
            "./Projects",
            "/etc/hosts",
            "/",
            "Projects//Plan.md",
            "Projects/",
            "/Projects",
            "Projects/Plan.md\0",
        ] {
            assert_eq!(
                kind(root.resolve_entry(bad)),
                ErrorKind::InvalidPath,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn rejects_hidden_segments_and_non_notes() {
        let (dir, root) = vault();
        fs::create_dir(dir.path().join("vault/.obsidian")).unwrap();
        fs::write(dir.path().join("vault/.obsidian/app.json"), "{}").unwrap();
        assert_eq!(
            kind(root.resolve_entry(".obsidian")),
            ErrorKind::InvalidPath
        );
        assert_eq!(
            kind(root.resolve_entry(".obsidian/app.json")),
            ErrorKind::InvalidPath
        );
        assert_eq!(
            kind(root.resolve_entry("image.png")),
            ErrorKind::InvalidPath
        );
        assert_eq!(kind(root.resolve_entry("")), ErrorKind::InvalidPath);
    }

    #[test]
    fn rejects_symlink_escaping_the_vault() {
        let (dir, root) = vault();
        symlink(
            dir.path().join("outside.md"),
            dir.path().join("vault/link.md"),
        )
        .unwrap();
        symlink(dir.path(), dir.path().join("vault/escape")).unwrap();
        assert_eq!(kind(root.resolve_entry("link.md")), ErrorKind::InvalidPath);
        assert_eq!(
            kind(root.resolve_entry("escape/outside.md")),
            ErrorKind::InvalidPath
        );
        assert_eq!(kind(root.resolve_dir("escape")), ErrorKind::InvalidPath);
        assert_eq!(
            kind(root.resolve_note_slot("escape/new.md")),
            ErrorKind::InvalidPath
        );
        assert_eq!(
            kind(root.resolve_note_slot("link.md")),
            ErrorKind::InvalidPath
        );
    }

    #[test]
    fn rejects_symlinks_even_inside_the_vault() {
        let (dir, root) = vault();
        symlink(
            dir.path().join("vault/Projects"),
            dir.path().join("vault/Alias"),
        )
        .unwrap();
        assert_eq!(
            kind(root.resolve_entry("Alias/Plan.md")),
            ErrorKind::InvalidPath
        );
    }

    #[test]
    fn missing_entries_are_not_found() {
        let (_dir, root) = vault();
        assert_eq!(
            kind(root.resolve_entry("Projects/Gone.md")),
            ErrorKind::NotFound
        );
        assert_eq!(
            kind(root.resolve_entry("Gone/Plan.md")),
            ErrorKind::NotFound
        );
        assert_eq!(
            kind(root.resolve_entry("Projects/Plan.md/x.md")),
            ErrorKind::NotFound
        );
    }

    #[test]
    fn note_slot_checks_the_parent_for_new_paths() {
        let (_dir, root) = vault();
        let slot = root.resolve_note_slot("Projects/New.md").unwrap();
        assert_eq!(slot, root.path().join("Projects/New.md"));
        assert_eq!(
            kind(root.resolve_note_slot("Missing/New.md")),
            ErrorKind::NotFound
        );
        assert_eq!(
            kind(root.resolve_note_slot("Projects/new.txt")),
            ErrorKind::InvalidPath
        );
        assert_eq!(
            kind(root.resolve_note_slot("../New.md")),
            ErrorKind::InvalidPath
        );
    }

    #[test]
    fn rel_of_maps_only_paths_strictly_inside() {
        let (dir, root) = vault();
        assert_eq!(
            root.rel_of(&root.path().join("Projects/Plan.md"))
                .as_deref(),
            Some("Projects/Plan.md")
        );
        assert_eq!(root.rel_of(root.path()), None);
        assert_eq!(root.rel_of(&dir.path().join("outside.md")), None);
    }

    #[test]
    fn nfc_and_nfd_names_fold_together() {
        let nfc_name = "Caf\u{e9}.md";
        let nfd_name = "Cafe\u{301}.md";
        assert_ne!(nfc_name, nfd_name);
        assert_eq!(fold(nfc_name), fold(nfd_name));
        assert_eq!(fold("CAFÉ.MD"), fold(nfd_name));
        assert_eq!(sanitize_stem("Cafe\u{301}"), "Caf\u{e9}");
    }

    #[test]
    fn collisions_ignore_case_and_normalisation() {
        let dir = TempDir::new();
        fs::write(dir.path().join("Cafe\u{301}.md"), "").unwrap();
        fs::write(dir.path().join("ideas.md"), "").unwrap();
        fs::write(dir.path().join("Ideas 2.md"), "").unwrap();
        assert_eq!(
            unique_name(dir.path(), "Caf\u{e9}", Some("md")).unwrap(),
            "Caf\u{e9} 2.md"
        );
        assert_eq!(
            unique_name(dir.path(), "IDEAS", Some("md")).unwrap(),
            "IDEAS 3.md"
        );
        assert_eq!(
            unique_name(dir.path(), "Fresh", Some("md")).unwrap(),
            "Fresh.md"
        );
        assert!(name_taken(dir.path(), "IDEAS.MD", None).unwrap());
        assert!(!name_taken(dir.path(), "other.md", None).unwrap());
    }

    #[test]
    fn case_only_rename_of_the_same_file_is_not_a_collision() {
        let dir = TempDir::new();
        fs::write(dir.path().join("ideas.md"), "").unwrap();
        fs::write(dir.path().join("other.md"), "").unwrap();
        let source = fs::metadata(dir.path().join("ideas.md")).unwrap();
        assert!(!name_taken(dir.path(), "Ideas.md", Some(&source)).unwrap());
        let other = fs::metadata(dir.path().join("other.md")).unwrap();
        assert!(name_taken(dir.path(), "Ideas.md", Some(&other)).unwrap());
    }

    #[test]
    fn sanitizes_titles_into_file_names() {
        assert_eq!(sanitize_stem("Meeting: Q3/Q4"), "Meeting Q3Q4");
        assert_eq!(sanitize_stem("..hidden"), "hidden");
        assert_eq!(sanitize_stem("  . spaced  "), "spaced");
        assert_eq!(sanitize_stem("trailing   "), "trailing");
        assert_eq!(sanitize_stem("line\nbreak"), "line break");
        assert_eq!(sanitize_stem("Notes.md"), "Notes");
        assert_eq!(sanitize_stem("Notes.MD"), "Notes");
        assert_eq!(sanitize_stem("///"), UNTITLED);
        assert_eq!(sanitize_stem(""), UNTITLED);
        assert_eq!(sanitize_stem(".md"), "md");
    }

    #[test]
    fn long_names_are_capped_at_255_bytes_on_char_boundaries() {
        let dir = TempDir::new();
        let long = "é".repeat(200); // 400 bytes
        let first = unique_name(dir.path(), &long, Some("md")).unwrap();
        assert!(first.len() <= MAX_NAME_BYTES);
        assert!(first.ends_with(".md"));
        fs::write(dir.path().join(&first), "").unwrap();
        let second = unique_name(dir.path(), &long, Some("md")).unwrap();
        assert!(second.len() <= MAX_NAME_BYTES);
        assert!(second.ends_with(" 2.md"), "{second}");
        fs::write(dir.path().join(&second), "").unwrap();
    }

    #[test]
    fn validates_typed_names() {
        assert!(validate_name("Ideas.md").is_ok());
        for bad in ["", "a/b", "a:b", ".hidden", "tab\there"] {
            assert_eq!(
                validate_name(bad).unwrap_err().kind,
                ErrorKind::InvalidName,
                "{bad:?}"
            );
        }
        assert!(validate_name(&"a".repeat(256)).is_err());
    }

    #[test]
    fn note_names_and_relative_helpers() {
        assert!(is_note_name("a.md") && is_note_name("A.MD") && is_note_name("a.Md"));
        assert!(!is_note_name(".md") && !is_note_name("a.markdown") && !is_note_name("md"));
        assert_eq!(note_stem("Plan.MD"), Some("Plan"));
        assert_eq!(parent_rel("a/b/c.md"), "a/b");
        assert_eq!(parent_rel("c.md"), "");
        assert_eq!(join_rel("", "c.md"), "c.md");
        assert!(is_within("a/b", "a") && is_within("a", "a") && is_within("a", ""));
        assert!(!is_within("ab", "a") && !is_within("a", "a/b"));
    }
}

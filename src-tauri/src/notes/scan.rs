//! Walking the vault and describing entries.
//!
//! Listed: folders, and files ending in `.md` (any case). Skipped: anything whose name starts
//! with `.`, symbolic links (never followed), other files, and dataless (offloaded) notes.
//! A dataless folder is listed but not entered, since listing it would download it.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::Serialize;

use super::note::{self, Loaded, Unreadable};
use super::paths::{self, EntryKind, Root};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    /// Above the size limit: listed, never opened.
    TooLarge,
    /// Not UTF-8: listed, never opened for editing.
    UnsupportedEncoding,
    /// Permissions don't allow reading it.
    Unreadable,
    /// A folder whose contents are in the cloud and not downloaded.
    Offloaded,
}

/// Display data for one note or folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub kind: EntryKind,
    /// Relative to the vault root, `/`-separated, exactly as named on disk.
    pub rel_path: String,
    pub name: String,
    /// Notes only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Unix milliseconds.
    pub mtime_ms: Option<i64>,
    pub size: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<Flag>,
}

/// How to tell whether a note changed without keeping its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fingerprint {
    /// SHA-256 of the bytes on disk.
    Revision(String),
    /// For files we don't read (too large, unreadable).
    Meta {
        size: u64,
        mtime_ms: Option<i64>,
    },
    Folder,
}

/// An entry plus what the watcher needs to compare it: its fingerprint and file identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indexed {
    pub entry: Entry,
    pub fingerprint: Fingerprint,
    pub dev: u64,
    pub ino: u64,
}

/// The whole vault as display data.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tree {
    pub root_name: String,
    pub root_path: String,
    pub entries: Vec<Entry>,
}

fn mtime_ms(meta: &fs::Metadata) -> Option<i64> {
    let since = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(since.as_millis()).ok()
}

/// Describes a folder at `abs` (already known to be a real directory).
pub fn index_folder(rel: &str, meta: &fs::Metadata, flags: Vec<Flag>) -> Indexed {
    Indexed {
        entry: Entry {
            kind: EntryKind::Folder,
            rel_path: rel.to_owned(),
            name: paths::file_name(rel).to_owned(),
            title: None,
            mtime_ms: mtime_ms(meta),
            size: 0,
            flags,
        },
        fingerprint: Fingerprint::Folder,
        dev: meta.dev(),
        ino: meta.ino(),
    }
}

/// Describes the note at `abs`. `None` when it isn't a listable note (gone, link, not a regular
/// file, not `.md`, dataless).
pub fn index_note(abs: &Path, rel: &str) -> io::Result<Option<Indexed>> {
    let name = paths::file_name(rel);
    if paths::is_hidden(name) || !paths::is_note_name(name) {
        return Ok(None);
    }
    let meta = match fs::symlink_metadata(abs) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !meta.is_file() || note::is_dataless(&meta) {
        return Ok(None);
    }
    let meta_fingerprint = || Fingerprint::Meta {
        size: meta.len(),
        mtime_ms: mtime_ms(&meta),
    };
    let (title, fingerprint, flags) = match note::load(abs) {
        Ok(Loaded::Text { bytes, text }) => (
            note::title(&text, name),
            Fingerprint::Revision(note::revision(&bytes)),
            vec![],
        ),
        Ok(Loaded::Binary { bytes }) => (
            note::fallback_title(name),
            Fingerprint::Revision(note::revision(&bytes)),
            vec![Flag::UnsupportedEncoding],
        ),
        Ok(Loaded::Skipped(Unreadable::Offloaded)) => return Ok(None),
        Ok(Loaded::Skipped(Unreadable::TooLarge)) => (
            note::fallback_title(name),
            meta_fingerprint(),
            vec![Flag::TooLarge],
        ),
        Ok(Loaded::Skipped(Unreadable::PermissionDenied)) => (
            note::fallback_title(name),
            meta_fingerprint(),
            vec![Flag::Unreadable],
        ),
        // Deleted between stat and open.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(Some(Indexed {
        entry: Entry {
            kind: EntryKind::Note,
            rel_path: rel.to_owned(),
            name: name.to_owned(),
            title: Some(title),
            mtime_ms: mtime_ms(&meta),
            size: meta.len(),
            flags,
        },
        fingerprint,
        dev: meta.dev(),
        ino: meta.ino(),
    }))
}

/// Describes whatever is at `rel` now: a note, a folder (with everything inside it), or nothing.
pub fn index_path(root: &Root, rel: &str) -> io::Result<Vec<Indexed>> {
    if rel.split('/').any(paths::is_hidden) {
        return Ok(Vec::new());
    }
    let abs = root.path().join(rel);
    let meta = match fs::symlink_metadata(&abs) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    if meta.is_dir() {
        let mut out = Vec::new();
        scan_folder(&abs, rel, &meta, &mut out);
        Ok(out)
    } else {
        Ok(index_note(&abs, rel)?.into_iter().collect())
    }
}

/// Indexes the folder at `abs` and, recursively, its contents.
fn scan_folder(abs: &Path, rel: &str, meta: &fs::Metadata, out: &mut Vec<Indexed>) {
    if note::is_dataless(meta) {
        out.push(index_folder(rel, meta, vec![Flag::Offloaded]));
        return;
    }
    match fs::read_dir(abs) {
        Ok(entries) => {
            out.push(index_folder(rel, meta, vec![]));
            scan_children(abs, rel, entries, out);
        }
        Err(_) => out.push(index_folder(rel, meta, vec![Flag::Unreadable])),
    }
}

fn scan_children(abs: &Path, rel: &str, entries: fs::ReadDir, out: &mut Vec<Indexed>) {
    for entry in entries.filter_map(|entry| entry.ok()) {
        let Ok(name) = entry.file_name().into_string() else {
            continue; // Not UTF-8: can't be named by the UI.
        };
        if paths::is_hidden(&name) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let child_rel = paths::join_rel(rel, &name);
        let child_abs = abs.join(&name);
        if file_type.is_dir() {
            if let Ok(meta) = fs::symlink_metadata(&child_abs) {
                scan_folder(&child_abs, &child_rel, &meta, out);
            }
        } else if file_type.is_file()
            && let Ok(Some(indexed)) = index_note(&child_abs, &child_rel)
        {
            out.push(indexed);
        }
        // Symbolic links and other file types are skipped.
    }
}

/// Names of the entries in `abs` the scan would list (or skip as dataless notes): folders and
/// `.md` files that aren't hidden or links. Doesn't read any file.
pub fn visible_children(abs: &Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(abs)?.filter_map(|entry| entry.ok()) {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if paths::is_hidden(&name) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() || (file_type.is_file() && paths::is_note_name(&name)) {
            names.push(name);
        }
    }
    Ok(names)
}

/// Every listable entry in the vault (the root itself isn't an entry).
pub fn scan(root: &Root) -> io::Result<Vec<Indexed>> {
    let entries = fs::read_dir(root.path())?;
    let mut out = Vec::new();
    scan_children(root.path(), "", entries, &mut out);
    out.sort_by_cached_key(|indexed| paths::fold(&indexed.entry.rel_path));
    Ok(out)
}

pub fn tree(root: &Root, entries: Vec<Entry>) -> Tree {
    Tree {
        root_name: root
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Notes")
            .to_owned(),
        root_path: root.path().to_string_lossy().into_owned(),
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::test_support::TempDir;
    use std::os::unix::fs::symlink;

    fn rels(entries: &[Indexed]) -> Vec<&str> {
        entries.iter().map(|i| i.entry.rel_path.as_str()).collect()
    }

    #[test]
    fn lists_folders_and_notes_and_skips_everything_else() {
        let dir = TempDir::new();
        let v = dir.path();
        for folder in [".git", ".obsidian", ".trash", "Projects/Sub", "Empty"] {
            fs::create_dir_all(v.join(folder)).unwrap();
        }
        for file in [
            "Inbox.md",
            "LOUD.MD",
            "Projects/Plan.md",
            "Projects/Sub/Deep.md",
            ".DS_Store",
            ".hidden.md",
            ".obsidian/workspace.md",
            ".git/HEAD",
            "image.png",
            "board.canvas",
            "Projects/doc.pdf",
            "notes.markdown",
        ] {
            fs::write(v.join(file), "x").unwrap();
        }
        symlink(v.join("Projects"), v.join("Linked")).unwrap();
        symlink(v.join("Inbox.md"), v.join("Alias.md")).unwrap();
        symlink(dir.path(), v.join("Projects/Loop")).unwrap();

        let root = Root::new(v).unwrap();
        let entries = scan(&root).unwrap();
        assert_eq!(
            rels(&entries),
            vec![
                "Empty",
                "Inbox.md",
                "LOUD.MD",
                "Projects",
                "Projects/Plan.md",
                "Projects/Sub",
                "Projects/Sub/Deep.md",
            ]
        );
    }

    #[test]
    fn titles_and_flags() {
        let dir = TempDir::new();
        let v = dir.path();
        fs::write(v.join("fm.md"), "---\ntitle: Front\n---\n# Head\n").unwrap();
        fs::write(v.join("head.md"), "intro\n# Head\n").unwrap();
        fs::write(v.join("plain name.md"), "text\n").unwrap();
        fs::write(v.join("latin1.md"), b"caf\xe9").unwrap();
        let big = fs::File::create(v.join("big.md")).unwrap();
        big.set_len(note::MAX_NOTE_BYTES + 1).unwrap();

        let root = Root::new(v).unwrap();
        let by_name = |name: &str| {
            scan(&root)
                .unwrap()
                .into_iter()
                .find(|i| i.entry.name == name)
                .unwrap()
        };
        assert_eq!(by_name("fm.md").entry.title.as_deref(), Some("Front"));
        assert_eq!(by_name("head.md").entry.title.as_deref(), Some("Head"));
        assert_eq!(
            by_name("plain name.md").entry.title.as_deref(),
            Some("plain name")
        );
        let latin1 = by_name("latin1.md");
        assert_eq!(latin1.entry.flags, vec![Flag::UnsupportedEncoding]);
        assert_eq!(latin1.entry.title.as_deref(), Some("latin1"));
        let big = by_name("big.md");
        assert_eq!(big.entry.flags, vec![Flag::TooLarge]);
        assert!(matches!(big.fingerprint, Fingerprint::Meta { .. }));
    }

    #[test]
    fn scanning_never_modifies_files() {
        let dir = TempDir::new();
        let v = dir.path();
        let content = b"---\r\ntitle: 'Keep'\r\nid: 123\r\n---\r\n# H\r\n";
        fs::write(v.join("n.md"), content).unwrap();
        let before = fs::metadata(v.join("n.md")).unwrap().modified().unwrap();
        let root = Root::new(v).unwrap();
        scan(&root).unwrap();
        assert_eq!(fs::read(v.join("n.md")).unwrap(), content);
        assert_eq!(
            fs::metadata(v.join("n.md")).unwrap().modified().unwrap(),
            before
        );
        let names: Vec<_> = fs::read_dir(v)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "no files created");
    }
}

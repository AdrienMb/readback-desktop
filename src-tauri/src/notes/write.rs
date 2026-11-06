//! Saving a note: conflict check, then an atomic replace.
//!
//! Before writing, the file on disk is re-read and hashed. If it no longer matches the
//! revision the edit started from, nothing is written and the caller gets the disk revision.
//! The remaining window between that check and the replace is a few milliseconds.

use std::fs;
use std::io;
use std::path::Path;

use super::errors::{Error, Result};
use super::macos;
use super::note::{self, Loaded, MAX_NOTE_BYTES, Style};
use super::paths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The file now holds the text. `revision` is the hash of its bytes.
    Saved { revision: String },
    /// The file changed since `base_revision` (or was deleted: `None`, or created: `Some`).
    /// Nothing was written.
    Conflict { disk_revision: Option<String> },
}

/// Writes `text` to the note at `path`.
///
/// - `base_revision: Some(r)`: the file must still hash to `r`.
/// - `base_revision: None`: the file must not exist (create, or recreate after a deletion).
pub fn write_note(path: &Path, text: &str, base_revision: Option<&str>) -> Result<Outcome> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    let fail = |error: io::Error| Error::io(&error, "saving", &name);

    let disk = match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() => return Err(Error::invalid_path()),
        Ok(_) => match note::load(path).map_err(fail)? {
            Loaded::Text { bytes, .. } | Loaded::Binary { bytes } => Some(bytes),
            Loaded::Skipped(reason) => return Err(note::unreadable_error(reason, &name)),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(fail(error)),
    };

    match (base_revision, disk) {
        (None, Some(bytes)) => Ok(Outcome::Conflict {
            disk_revision: Some(note::revision(&bytes)),
        }),
        (Some(_), None) => Ok(Outcome::Conflict {
            disk_revision: None,
        }),
        (None, None) => {
            let bytes = note::encode(text, Style::default());
            check_size(&bytes, &name)?;
            match macos::write_new(path, &bytes, 0o644) {
                Ok(()) => Ok(Outcome::Saved {
                    revision: note::revision(&bytes),
                }),
                // Created by someone else in the meantime.
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    Ok(Outcome::Conflict {
                        disk_revision: fs::read(path).ok().map(|b| note::revision(&b)),
                    })
                }
                Err(error) => Err(fail(error)),
            }
        }
        (Some(base), Some(current)) => {
            let disk_revision = note::revision(&current);
            if disk_revision != base {
                return Ok(Outcome::Conflict {
                    disk_revision: Some(disk_revision),
                });
            }
            if note::decode(&current).is_none() {
                return Err(Error::unsupported_encoding(&name));
            }
            let bytes = note::encode(text, Style::of(&current));
            if bytes == current {
                return Ok(Outcome::Saved {
                    revision: disk_revision,
                });
            }
            check_size(&bytes, &name)?;
            macos::replace(path, &bytes).map_err(fail)?;
            Ok(Outcome::Saved {
                revision: note::revision(&bytes),
            })
        }
    }
}

fn check_size(bytes: &[u8], name: &str) -> Result<()> {
    if bytes.len() as u64 > MAX_NOTE_BYTES {
        return Err(Error::too_large(name, MAX_NOTE_BYTES));
    }
    Ok(())
}

/// Creates a new, empty note named after `title` in `dir`. Returns the file name.
pub fn create_note(dir: &Path, title: &str) -> Result<String> {
    let stem = paths::sanitize_stem(title);
    // Retry if another app takes the name between choosing it and creating the file.
    for _ in 0..8 {
        let name = paths::unique_name(dir, &stem, Some(paths::NOTE_EXTENSION))
            .map_err(|error| Error::io(&error, "creating", &stem))?;
        match macos::write_new(&dir.join(&name), b"", 0o644) {
            Ok(()) => return Ok(name),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(Error::io(&error, "creating", &name)),
        }
    }
    Err(Error::already_exists(&stem))
}

/// Creates a new folder named after `name` in `dir`. Returns the folder name.
pub fn create_folder(dir: &Path, name: &str) -> Result<String> {
    let stem = paths::sanitize_stem(name);
    for _ in 0..8 {
        let name = paths::unique_name(dir, &stem, None)
            .map_err(|error| Error::io(&error, "creating", &stem))?;
        match fs::create_dir(dir.join(&name)) {
            Ok(()) => return Ok(name),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(Error::io(&error, "creating", &name)),
        }
    }
    Err(Error::already_exists(&stem))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::errors::ErrorKind;
    use crate::notes::test_support::TempDir;

    fn saved(outcome: Outcome) -> String {
        match outcome {
            Outcome::Saved { revision } => revision,
            other => panic!("expected Saved, got {other:?}"),
        }
    }

    #[test]
    fn saves_when_the_base_revision_matches() {
        let dir = TempDir::new();
        let path = dir.path().join("n.md");
        fs::write(&path, "one\n").unwrap();
        let base = note::revision(b"one\n");
        let revision = saved(write_note(&path, "two\n", Some(&base)).unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"two\n");
        assert_eq!(revision, note::revision(b"two\n"));
    }

    #[test]
    fn detects_a_change_between_read_and_write() {
        let dir = TempDir::new();
        let path = dir.path().join("n.md");
        fs::write(&path, "original\n").unwrap();
        let base = note::revision(b"original\n");
        // Another app saves in between.
        fs::write(&path, "theirs\n").unwrap();
        let outcome = write_note(&path, "mine\n", Some(&base)).unwrap();
        assert_eq!(
            outcome,
            Outcome::Conflict {
                disk_revision: Some(note::revision(b"theirs\n"))
            }
        );
        assert_eq!(fs::read(&path).unwrap(), b"theirs\n", "nothing was written");
    }

    #[test]
    fn deleted_file_is_a_conflict_and_recreate_needs_no_base() {
        let dir = TempDir::new();
        let path = dir.path().join("n.md");
        let outcome = write_note(&path, "mine\n", Some("abc")).unwrap();
        assert_eq!(
            outcome,
            Outcome::Conflict {
                disk_revision: None
            }
        );
        assert!(!path.exists());

        saved(write_note(&path, "mine\n", None).unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"mine\n");

        // Recreating over a file that reappeared is a conflict too.
        let outcome = write_note(&path, "again\n", None).unwrap();
        assert!(matches!(
            outcome,
            Outcome::Conflict {
                disk_revision: Some(_)
            }
        ));
    }

    #[test]
    fn keeps_bom_and_crlf_and_frontmatter_bytes() {
        let dir = TempDir::new();
        let path = dir.path().join("n.md");
        let original = b"\xEF\xBB\xBF---\r\ntitle: \"Keep: me\"\r\ntags: [a, b]\r\n---\r\nBody\r\n";
        fs::write(&path, original).unwrap();
        let text = note::decode(original).unwrap();
        let edited = text.replace("Body", "Body edited");
        saved(write_note(&path, &edited, Some(&note::revision(original))).unwrap());
        let mut expected = original.to_vec();
        expected.truncate(expected.len() - "Body\r\n".len());
        expected.extend_from_slice(b"Body edited\r\n");
        assert_eq!(fs::read(&path).unwrap(), expected);
    }

    #[test]
    fn unchanged_text_does_not_touch_the_file() {
        let dir = TempDir::new();
        let path = dir.path().join("n.md");
        fs::write(&path, "same\r\n").unwrap();
        let before = fs::metadata(&path).unwrap();
        let base = note::revision(b"same\r\n");
        assert_eq!(
            saved(write_note(&path, "same\n", Some(&base)).unwrap()),
            base
        );
        let after = fs::metadata(&path).unwrap();
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    }

    #[test]
    fn refuses_to_write_over_non_utf8() {
        let dir = TempDir::new();
        let path = dir.path().join("n.md");
        fs::write(&path, b"caf\xe9").unwrap();
        let base = note::revision(b"caf\xe9");
        let error = write_note(&path, "cafe", Some(&base)).unwrap_err();
        assert_eq!(error.kind, ErrorKind::UnsupportedEncoding);
        assert_eq!(fs::read(&path).unwrap(), b"caf\xe9");
    }

    #[test]
    fn atomic_write_leaves_only_the_note() {
        let dir = TempDir::new();
        let path = dir.path().join("n.md");
        fs::write(&path, "a").unwrap();
        let mut base = note::revision(b"a");
        for i in 0..5 {
            base = saved(write_note(&path, &format!("v{i}"), Some(&base)).unwrap());
        }
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec!["n.md"]);
    }

    #[test]
    fn create_note_and_folder_pick_free_names() {
        let dir = TempDir::new();
        fs::write(dir.path().join("ideas.md"), "keep").unwrap();
        assert_eq!(create_note(dir.path(), "Ideas").unwrap(), "Ideas 2.md");
        assert_eq!(create_note(dir.path(), "a: b/c").unwrap(), "a bc.md");
        assert_eq!(fs::read(dir.path().join("ideas.md")).unwrap(), b"keep");
        assert_eq!(fs::read(dir.path().join("Ideas 2.md")).unwrap(), b"");
        assert_eq!(create_folder(dir.path(), "Projects").unwrap(), "Projects");
        assert_eq!(create_folder(dir.path(), "projects").unwrap(), "projects 2");
    }
}

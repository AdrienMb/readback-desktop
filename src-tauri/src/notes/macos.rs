//! The two Foundation calls the notes module needs, through `objc2`'s safe bindings.
//!
//! - [`trash`]: `NSFileManager.trashItemAtURL`, the system Move to Trash (no Finder scripting, so
//!   no Automation permission prompt).
//! - [`replace`]: `NSFileManager.replaceItemAtURL`, an atomic replace that keeps the original
//!   file's creation date, permissions, Finder tags and other extended attributes. The new
//!   bytes are staged in a replacement directory the system picks on the same volume, outside
//!   the vault.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use objc2::rc::autoreleasepool;
use objc2_foundation::{
    NSError, NSFileManager, NSFileManagerItemReplacementOptions, NSSearchPathDirectory,
    NSSearchPathDomainMask, NSString, NSURL,
};

fn url(path: &Path) -> io::Result<objc2::rc::Retained<NSURL>> {
    let path = path
        .to_str()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidFilename))?;
    Ok(NSURL::fileURLWithPath(&NSString::from_str(path)))
}

/// Maps the Cocoa and POSIX errors these calls report onto `io::Error`.
fn io_error(error: &NSError) -> io::Error {
    let domain = error.domain().to_string();
    let code = error.code();
    if domain == "NSPOSIXErrorDomain"
        && let Ok(errno) = i32::try_from(code)
    {
        return io::Error::from_raw_os_error(errno);
    }
    let kind = match (domain.as_str(), code) {
        // NSFileNoSuchFileError, NSFileReadNoSuchFileError
        ("NSCocoaErrorDomain", 4 | 260) => io::ErrorKind::NotFound,
        // NSFileReadNoPermissionError, NSFileWriteNoPermissionError
        ("NSCocoaErrorDomain", 257 | 513) => io::ErrorKind::PermissionDenied,
        // NSFileWriteFileExistsError
        ("NSCocoaErrorDomain", 516) => io::ErrorKind::AlreadyExists,
        // NSFileWriteOutOfSpaceError
        ("NSCocoaErrorDomain", 640) => io::ErrorKind::StorageFull,
        // NSFileWriteVolumeReadOnlyError
        ("NSCocoaErrorDomain", 642) => io::ErrorKind::ReadOnlyFilesystem,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, format!("{domain} {code}"))
}

/// Moves a file or folder (with everything in it) to the user's Trash.
pub fn trash(path: &Path) -> io::Result<()> {
    autoreleasepool(|_| {
        let item = url(path)?;
        let manager = NSFileManager::defaultManager();
        manager
            .trashItemAtURL_resultingItemURL_error(&item, None)
            .map_err(|error| io_error(&error))
    })
}

/// Renames or moves `from` to `to` on the same volume. Fails if `to` exists: unlike
/// `rename(2)`, it never replaces an existing file.
pub fn move_item(from: &Path, to: &Path) -> io::Result<()> {
    autoreleasepool(|_| {
        let (from, to) = (url(from)?, url(to)?);
        let manager = NSFileManager::defaultManager();
        manager
            .moveItemAtURL_toURL_error(&from, &to)
            .map_err(|error| io_error(&error))
    })
}

/// A fresh, empty replacement directory for `target` on the same volume, or `None` if the
/// system can't provide one.
fn replacement_dir(target: &Path) -> Option<PathBuf> {
    autoreleasepool(|_| {
        let target = url(target).ok()?;
        let manager = NSFileManager::defaultManager();
        let dir = manager
            .URLForDirectory_inDomain_appropriateForURL_create_error(
                NSSearchPathDirectory::ItemReplacementDirectory,
                NSSearchPathDomainMask::UserDomainMask,
                Some(&target),
                true,
            )
            .ok()?;
        dir.path().map(|path| PathBuf::from(path.to_string()))
    })
}

/// Atomically replaces the existing file `target` with `bytes`. (If `target` was deleted in the
/// meantime, Foundation moves the new file into place instead, so the edit isn't lost.)
pub fn replace(target: &Path, bytes: &[u8]) -> io::Result<()> {
    match replacement_dir(target) {
        Some(dir) => replace_via(target, bytes, &dir),
        None => replace_in_place_dir(target, bytes),
    }
}

/// Stages `bytes` in `dir`, swaps it in, then removes `dir`. Nothing is left in `dir` whether
/// the replace succeeds or fails.
pub(crate) fn replace_via(target: &Path, bytes: &[u8], dir: &Path) -> io::Result<()> {
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidFilename))?;
    let staged = dir.join(name);
    let result = (|| {
        write_new(&staged, bytes, 0o600)?;
        autoreleasepool(|_| {
            let (original, replacement) = (url(target)?, url(&staged)?);
            let manager = NSFileManager::defaultManager();
            manager
                .replaceItemAtURL_withItemAtURL_backupItemName_options_resultingItemURL_error(
                    &original,
                    &replacement,
                    None,
                    NSFileManagerItemReplacementOptions::empty(),
                    None,
                )
                .map_err(|error| io_error(&error))
        })
    })();
    // On success the staged file has been moved; on failure it may still be there.
    let _ = fs::remove_file(&staged);
    let _ = fs::remove_dir(dir);
    result
}

/// Fallback when the system has no replacement directory for this volume: a hidden temp file
/// next to the target (skipped by the scan), the target's permissions, then `rename`.
fn replace_in_place_dir(target: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidFilename))?;
    let mode = fs::metadata(target)?.permissions().mode() & 0o7777;
    let tmp = parent.join(format!(".readback-{}.tmp", uuid::Uuid::new_v4()));
    let result = write_new(&tmp, bytes, mode).and_then(|()| fs::rename(&tmp, target));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Creates `path` (which must not exist), writes `bytes` and syncs them to disk.
pub fn write_new(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::test_support::TempDir;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    #[test]
    fn replace_keeps_permissions_and_extended_attributes() {
        let dir = TempDir::new();
        let target = dir.path().join("note.md");
        fs::write(&target, "old").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        // A Finder tag is an extended attribute; an atomic rename-over would drop it.
        let status = Command::new("/usr/bin/xattr")
            .args(["-w", "com.readback.test", "kept"])
            .arg(&target)
            .status()
            .unwrap();
        assert!(status.success());

        replace(&target, b"new").unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new");
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
        let output = Command::new("/usr/bin/xattr")
            .args(["-p", "com.readback.test"])
            .arg(&target)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "kept");
    }

    #[test]
    fn replace_leaves_no_temp_file_anywhere() {
        let dir = TempDir::new();
        let vault = dir.path().join("vault");
        let staging = dir.path().join("staging");
        fs::create_dir_all(&vault).unwrap();
        fs::create_dir_all(&staging).unwrap();
        let target = vault.join("note.md");
        fs::write(&target, "old").unwrap();

        replace_via(&target, b"new", &staging).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new");
        let names: Vec<_> = fs::read_dir(&vault)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("note.md")]);
        assert!(!staging.exists(), "the replacement directory is removed");

        // And through the system replacement directory.
        replace(&target, b"newer").unwrap();
        let names: Vec<_> = fs::read_dir(&vault)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("note.md")]);
    }

    #[test]
    fn failed_replace_cleans_up() {
        let dir = TempDir::new();
        let staging = dir.path().join("staging");
        fs::create_dir_all(&staging).unwrap();
        // The target's folder is gone, so the replace fails.
        let target = dir.path().join("gone/note.md");
        assert!(replace_via(&target, b"new", &staging).is_err());
        assert!(!target.exists());
        assert!(!staging.exists(), "the staged copy is removed");
    }

    #[test]
    fn move_item_never_replaces_an_existing_file() {
        let dir = TempDir::new();
        fs::write(dir.path().join("a.md"), "a").unwrap();
        fs::write(dir.path().join("b.md"), "b").unwrap();
        let error = move_item(&dir.path().join("a.md"), &dir.path().join("b.md")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(dir.path().join("b.md")).unwrap(), b"b");
        move_item(&dir.path().join("a.md"), &dir.path().join("c.md")).unwrap();
        assert_eq!(fs::read(dir.path().join("c.md")).unwrap(), b"a");
    }

    #[test]
    #[ignore = "moves a file to the real user Trash; run manually with --ignored"]
    fn trash_moves_to_the_user_trash() {
        let dir = TempDir::new();
        let target = dir.path().join("readback-trash-test.md");
        fs::write(&target, "bye").unwrap();
        trash(&target).unwrap();
        assert!(!target.exists());
    }
}

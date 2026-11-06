//! Typed errors with a user-facing message.
//!
//! Messages name the entry involved (its file name, never note content) and say what happened.
//! They are safe to log and to send to the webview.

use std::fmt;
use std::io;

use serde::Serialize;

/// What went wrong, so the UI can pick the right state or action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// No vault folder is configured yet.
    VaultNotSet,
    /// The vault folder is gone, renamed or on a drive that isn't connected.
    VaultMissing,
    /// macOS (or the folder's permissions) doesn't let Readback read the vault.
    VaultDenied,
    /// The entry doesn't exist (any more).
    NotFound,
    /// An entry with that name already exists.
    AlreadyExists,
    /// The file or folder can't be read or written because of its permissions.
    PermissionDenied,
    /// The note is bigger than Readback opens.
    TooLarge,
    /// The note isn't UTF-8 text.
    UnsupportedEncoding,
    /// The note is stored in iCloud (or another cloud service) and not downloaded.
    Offloaded,
    /// Reading took too long (for example a slow network drive).
    Timeout,
    /// The webview sent a path that doesn't resolve to an entry inside the vault.
    InvalidPath,
    /// A name the user typed can't be used.
    InvalidName,
    /// The operation doesn't apply to this entry (for example moving a folder into itself).
    InvalidOperation,
    /// The disk is full or the volume is read-only.
    DiskFull,
    /// Anything else. The message is still specific.
    Other,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Error {
    pub kind: ErrorKind,
    pub code: &'static str,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(kind: ErrorKind, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            code,
            message: message.into(),
        }
    }

    pub fn vault_not_set() -> Self {
        Self::new(
            ErrorKind::VaultNotSet,
            "vault_not_set",
            "Choose a notes folder first.",
        )
    }

    pub fn vault_missing() -> Self {
        Self::new(
            ErrorKind::VaultMissing,
            "vault_missing",
            "Readback can't find your notes folder. It may have been moved, renamed or be on a \
             drive that isn't connected.",
        )
    }

    pub fn vault_denied() -> Self {
        Self::new(
            ErrorKind::VaultDenied,
            "vault_denied",
            "Readback doesn't have permission to read your notes folder. Allow access in System \
             Settings → Privacy & Security → Files and Folders, or choose another folder.",
        )
    }

    pub fn invalid_path() -> Self {
        Self::new(
            ErrorKind::InvalidPath,
            "invalid_path",
            "That path isn't inside your notes folder.",
        )
    }

    pub fn invalid_name(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidName, "invalid_name", message)
    }

    pub fn not_found(name: &str) -> Self {
        Self::new(
            ErrorKind::NotFound,
            "not_found",
            format!("“{name}” no longer exists in your notes folder."),
        )
    }

    pub fn already_exists(name: &str) -> Self {
        Self::new(
            ErrorKind::AlreadyExists,
            "already_exists",
            format!("An item named “{name}” already exists in this folder."),
        )
    }

    pub fn too_large(name: &str, limit_bytes: u64) -> Self {
        Self::new(
            ErrorKind::TooLarge,
            "too_large",
            format!(
                "“{name}” is larger than {} MB, so Readback doesn't open it.",
                limit_bytes / (1024 * 1024)
            ),
        )
    }

    pub fn unsupported_encoding(name: &str) -> Self {
        Self::new(
            ErrorKind::UnsupportedEncoding,
            "unsupported_encoding",
            format!("“{name}” isn't UTF-8 text, so Readback can't edit it safely."),
        )
    }

    pub fn offloaded(name: &str) -> Self {
        Self::new(
            ErrorKind::Offloaded,
            "offloaded",
            format!(
                "“{name}” is stored in the cloud and isn't downloaded to this Mac. Download it in \
                 Finder, then try again."
            ),
        )
    }

    pub fn timeout(name: &str) -> Self {
        Self::new(
            ErrorKind::Timeout,
            "timeout",
            format!("Reading “{name}” took too long. The drive may be slow or disconnected."),
        )
    }

    pub fn invalid_operation(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidOperation, code, message)
    }

    pub fn other(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Other, code, message)
    }

    /// Maps an I/O error on the entry called `name` ("reading", "saving", "moving"…).
    pub fn io(error: &io::Error, action: &str, name: &str) -> Self {
        use io::ErrorKind as K;
        match error.kind() {
            K::NotFound => Self::not_found(name),
            K::AlreadyExists | K::DirectoryNotEmpty => Self::already_exists(name),
            K::PermissionDenied => Self::new(
                ErrorKind::PermissionDenied,
                "permission_denied",
                format!("Readback doesn't have permission for {action} “{name}”."),
            ),
            K::StorageFull | K::QuotaExceeded => Self::new(
                ErrorKind::DiskFull,
                "disk_full",
                format!("There isn't enough disk space for {action} “{name}”."),
            ),
            K::ReadOnlyFilesystem => Self::new(
                ErrorKind::DiskFull,
                "read_only",
                format!("“{name}” is on a read-only drive, so {action} it isn't possible."),
            ),
            K::InvalidFilename => Self::invalid_name(format!("“{name}” isn't a valid name here.")),
            _ => Self::new(
                ErrorKind::Other,
                "io_error",
                format!("Readback couldn't finish {action} “{name}” ({error})."),
            ),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_map_to_specific_kinds() {
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        let error = Error::io(&denied, "saving", "Ideas.md");
        assert_eq!(error.kind, ErrorKind::PermissionDenied);
        assert!(error.message.contains("Ideas.md"));

        let exists = io::Error::from(io::ErrorKind::AlreadyExists);
        assert_eq!(
            Error::io(&exists, "renaming", "a").kind,
            ErrorKind::AlreadyExists
        );
    }
}

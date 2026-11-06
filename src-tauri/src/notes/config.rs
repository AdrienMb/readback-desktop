//! The vault location, persisted in `settings.json` in the app data dir.
//!
//! The file only holds the vault path. It is written atomically with `0600` permissions.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{ErrorKind as IoErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::errors::{Error, Result};

pub const SETTINGS_FILE: &str = "settings.json";
/// The folder `create_default_vault` creates in `~/Documents`.
pub const DEFAULT_VAULT_NAME: &str = "Readback";
const MAX_SETTINGS_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub vault_path: Option<PathBuf>,
}

/// The state of the configured vault folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VaultStatus {
    NotSet,
    Ok,
    /// Deleted, renamed, replaced by a file, or on an unmounted drive.
    Missing,
    /// The folder exists but can't be listed (macOS privacy settings or permissions).
    Denied,
}

/// Checks the folder without creating anything.
pub fn status_of(path: Option<&Path>) -> VaultStatus {
    let Some(path) = path else {
        return VaultStatus::NotSet;
    };
    match fs::metadata(path) {
        Ok(meta) if meta.is_dir() => match fs::read_dir(path) {
            Ok(_) => VaultStatus::Ok,
            Err(error) if error.kind() == IoErrorKind::PermissionDenied => VaultStatus::Denied,
            Err(_) => VaultStatus::Missing,
        },
        Ok(_) => VaultStatus::Missing,
        Err(error) if error.kind() == IoErrorKind::PermissionDenied => VaultStatus::Denied,
        Err(_) => VaultStatus::Missing,
    }
}

pub struct Config {
    dir: PathBuf,
}

impl Config {
    pub fn new(app_data_dir: PathBuf) -> Self {
        Self { dir: app_data_dir }
    }

    fn file(&self) -> PathBuf {
        self.dir.join(SETTINGS_FILE)
    }

    /// Loads settings. A missing file means first launch; an unreadable one is an error so that
    /// we never silently forget the user's vault.
    pub fn load(&self) -> Result<Settings> {
        let path = self.file();
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == IoErrorKind::NotFound => return Ok(Settings::default()),
            Err(error) => return Err(Error::io(&error, "reading", SETTINGS_FILE)),
        };
        let mut bytes = Vec::new();
        use std::io::Read;
        file.take(MAX_SETTINGS_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|error| Error::io(&error, "reading", SETTINGS_FILE))?;
        serde_json::from_slice(&bytes).map_err(|_| {
            Error::other(
                "settings_corrupt",
                format!(
                    "Readback's settings file is damaged. Delete “{}” and restart Readback.",
                    path.display()
                ),
            )
        })
    }

    pub fn save(&self, settings: &Settings) -> Result<()> {
        let fail = |error: std::io::Error| Error::io(&error, "saving", SETTINGS_FILE);
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)
            .map_err(fail)?;
        let bytes = serde_json::to_vec_pretty(settings)
            .map_err(|_| Error::other("settings_encode", "Readback couldn't save its settings."))?;
        let tmp = self
            .dir
            .join(format!(".{SETTINGS_FILE}.{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&tmp, self.file())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result.map_err(fail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::test_support::TempDir;

    #[test]
    fn settings_round_trip_and_first_launch() {
        let dir = TempDir::new();
        let config = Config::new(dir.path().join("app-data"));
        assert_eq!(config.load().unwrap(), Settings::default());

        let settings = Settings {
            vault_path: Some(PathBuf::from("/Users/someone/Documents/Readback")),
        };
        config.save(&settings).unwrap();
        assert_eq!(config.load().unwrap(), settings);
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("app-data"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from(SETTINGS_FILE)]);
    }

    #[test]
    fn status_reports_missing_without_creating() {
        let dir = TempDir::new();
        assert_eq!(status_of(None), VaultStatus::NotSet);
        assert_eq!(status_of(Some(dir.path())), VaultStatus::Ok);
        let gone = dir.path().join("gone");
        assert_eq!(status_of(Some(&gone)), VaultStatus::Missing);
        assert!(!gone.exists(), "checking must never create the folder");
        let file = dir.path().join("file.md");
        fs::write(&file, "x").unwrap();
        assert_eq!(status_of(Some(&file)), VaultStatus::Missing);
    }

    #[test]
    fn corrupt_settings_is_an_error_not_a_reset() {
        let dir = TempDir::new();
        fs::write(dir.path().join(SETTINGS_FILE), "{not json").unwrap();
        let config = Config::new(dir.path().to_path_buf());
        assert_eq!(config.load().unwrap_err().code, "settings_corrupt");
    }
}

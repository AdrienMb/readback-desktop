//! A temporary directory for tests, removed on drop. Never under the real home directory.

use std::fs;
use std::path::{Path, PathBuf};

pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new() -> Self {
        // Canonical, so it matches the paths FSEvents reports (`/private/var/...`).
        let base = fs::canonicalize(std::env::temp_dir()).unwrap();
        let path = base.join(format!("readback-notes-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

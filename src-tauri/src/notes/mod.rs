//! Local notes: plain folders and Markdown files in a user-visible folder (the vault).
//!
//! The files are the source of truth. All file access happens here, in Rust; the webview sends
//! vault-relative paths and gets display data back. Nothing is hard-deleted (folders and notes
//! go to the Trash), a file that changed on disk since it was read is never overwritten, and
//! files that aren't notes are never touched (they move with their folder).

pub mod commands;
pub mod config;
pub mod errors;
mod index;
mod macos;
pub mod note;
pub mod paths;
pub mod scan;
pub mod watcher;
pub mod write;

#[cfg(test)]
mod test_support;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use serde::Serialize;

use config::{Config, Settings, VaultStatus};
use errors::{Error, Result};
use note::Loaded;
use paths::{EntryKind, Root};
use scan::{Entry, Tree};
use watcher::{ChangeSet, Vault, Watcher};

/// What the UI shows about the vault folder.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultView {
    pub status: VaultStatus,
    /// The configured folder, when there is one (also when it's missing).
    pub path: Option<String>,
    pub name: Option<String>,
    /// Where "Create a Readback folder in Documents" would create it.
    pub default_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteView {
    pub content: String,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WriteView {
    #[serde(rename_all = "camelCase")]
    Saved {
        revision: String,
        entry: Entry,
        /// Folders recreated to hold a recreated note.
        created_folders: Vec<Entry>,
    },
    #[serde(rename_all = "camelCase")]
    Conflict {
        /// `None` when the note was deleted on disk.
        disk_revision: Option<String>,
    },
}

/// How results reach the UI (Tauri events in the app).
pub trait Emit: Send + Sync + 'static {
    fn changed(&self, changes: &ChangeSet);
    fn rescanned(&self, tree: &Tree);
    fn status(&self, vault: &VaultView);
}

#[derive(Default)]
struct State {
    settings: Settings,
    vault: Option<Arc<Vault>>,
    watcher: Option<Watcher>,
}

struct Inner {
    config: Config,
    default_vault: Option<PathBuf>,
    emit: Box<dyn Emit>,
    state: Mutex<State>,
}

/// The notes service. Cheap to clone; every method blocks on file I/O, so commands call it
/// from a blocking thread.
#[derive(Clone)]
pub struct Notes {
    inner: Arc<Inner>,
}

fn name_of(rel: &str) -> &str {
    paths::file_name(rel)
}

impl Notes {
    pub fn new(
        app_data_dir: PathBuf,
        default_vault: Option<PathBuf>,
        emit: Box<dyn Emit>,
    ) -> Result<Self> {
        let config = Config::new(app_data_dir);
        let settings = config.load()?;
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                default_vault,
                emit,
                state: Mutex::new(State {
                    settings,
                    ..State::default()
                }),
            }),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn view_of(&self, settings: &Settings) -> VaultView {
        let path = settings.vault_path.as_deref();
        VaultView {
            status: config::status_of(path),
            path: path.map(|p| p.to_string_lossy().into_owned()),
            name: path
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().into_owned()),
            default_path: self
                .inner
                .default_vault
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
        }
    }

    pub fn vault_view(&self) -> VaultView {
        let state = self.state();
        self.view_of(&state.settings)
    }

    // ------------------------------------------------------------------------------------------
    // Choosing the vault
    // ------------------------------------------------------------------------------------------

    /// Creates `~/Documents/Readback` if needed and makes it the vault.
    pub fn create_default_vault(&self) -> Result<VaultView> {
        let Some(path) = self.inner.default_vault.clone() else {
            return Err(Error::other(
                "no_documents_folder",
                "Readback can't find your Documents folder. Choose a folder instead.",
            ));
        };
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => {}
            Err(error) => return Err(Error::io(&error, "creating", "Readback")),
        }
        self.set_vault(&path)
    }

    /// Makes `path` (an existing folder) the vault. Stops the old watcher first; the new one
    /// starts with the next command that needs the vault (normally `list_tree`).
    pub fn set_vault(&self, path: &Path) -> Result<VaultView> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match config::status_of(Some(path)) {
            VaultStatus::Ok => {}
            VaultStatus::Denied => return Err(Error::vault_denied()),
            _ => return Err(Error::not_found(&name)),
        }
        let root = Root::new(path).map_err(|error| Error::io(&error, "opening", &name))?;
        let mut state = self.state();
        let settings = Settings {
            vault_path: Some(root.path().to_path_buf()),
        };
        self.inner.config.save(&settings)?;
        Self::deactivate(&mut state);
        state.settings = settings;
        Ok(self.view_of(&state.settings))
    }

    fn deactivate(state: &mut State) {
        if let Some(vault) = state.vault.take() {
            vault.active.store(false, Ordering::SeqCst);
        }
        // Stops without waiting for the watcher thread.
        state.watcher = None;
    }

    /// The active vault, starting it (watcher, then a full scan) if needed. `true` when it was
    /// just started, so its index is fresh.
    fn vault(&self) -> Result<(Arc<Vault>, bool)> {
        let mut state = self.state();
        if let Some(vault) = &state.vault {
            return Ok((Arc::clone(vault), false));
        }
        let Some(path) = state.settings.vault_path.clone() else {
            return Err(Error::vault_not_set());
        };
        match config::status_of(Some(&path)) {
            VaultStatus::Ok => {}
            VaultStatus::Denied => return Err(Error::vault_denied()),
            VaultStatus::Missing | VaultStatus::NotSet => return Err(Error::vault_missing()),
        }
        let root = Root::new(&path).map_err(|_| Error::vault_missing())?;
        let vault = Arc::new(Vault::new(root));
        // Watch first, then scan, so nothing that happens in between is missed.
        let sink = Arc::new(WatchSink {
            notes: Arc::downgrade(&self.inner),
            vault: Arc::downgrade(&vault),
        });
        let watcher = watcher::start(Arc::clone(&vault), sink).map_err(|_| {
            Error::other(
                "watch_failed",
                "Readback couldn't watch your notes folder for changes.",
            )
        })?;
        let entries = scan::scan(&vault.root)
            .map_err(|error| Error::io(&error, "reading", "your notes folder"))?;
        vault.index().replace_all(entries);
        state.vault = Some(Arc::clone(&vault));
        state.watcher = Some(watcher);
        Ok((vault, true))
    }

    // ------------------------------------------------------------------------------------------
    // Reading
    // ------------------------------------------------------------------------------------------

    pub fn list_tree(&self) -> Result<Tree> {
        let (vault, fresh) = self.vault()?;
        if !fresh {
            rescan(&vault)?;
        }
        let entries = vault.index().entries().map(|i| i.entry.clone()).collect();
        Ok(scan::tree(&vault.root, entries))
    }

    pub fn read_note(&self, rel: &str) -> Result<NoteView> {
        let (vault, _) = self.vault()?;
        let note = vault.root.resolve_note(rel)?;
        let name = name_of(rel);
        match note::load(&note.abs).map_err(|error| Error::io(&error, "reading", name))? {
            Loaded::Text { bytes, text } => Ok(NoteView {
                content: text,
                revision: note::revision(&bytes),
            }),
            Loaded::Binary { .. } => Err(Error::unsupported_encoding(name)),
            Loaded::Skipped(reason) => Err(note::unreadable_error(reason, name)),
        }
    }

    // ------------------------------------------------------------------------------------------
    // Writing
    // ------------------------------------------------------------------------------------------

    pub fn write_note(
        &self,
        rel: &str,
        content: &str,
        base_revision: Option<&str>,
    ) -> Result<WriteView> {
        let (vault, _) = self.vault()?;
        let mut created_folders = Vec::new();
        if base_revision.is_none() {
            // Recreating a note whose folder was deleted too: recreate the folders.
            for folder in ensure_folders(&vault.root, paths::parent_rel(rel))? {
                created_folders.push(folder);
            }
        }
        let path = match vault.root.resolve_note_slot(rel) {
            Ok(path) => path,
            // The note's folder was deleted (or renamed) on disk: that's a deleted note.
            Err(error) if error.kind == errors::ErrorKind::NotFound && base_revision.is_some() => {
                return Ok(WriteView::Conflict {
                    disk_revision: None,
                });
            }
            Err(error) => return Err(error),
        };
        match write::write_note(&path, content, base_revision)? {
            write::Outcome::Conflict { disk_revision } => Ok(WriteView::Conflict { disk_revision }),
            write::Outcome::Saved { revision } => {
                let entry = self.reindex(&vault, rel)?;
                let mut index = vault.index();
                let created_folders: Vec<Entry> = created_folders
                    .into_iter()
                    .map(|folder| {
                        let entry = folder.entry.clone();
                        index.insert(folder);
                        entry
                    })
                    .collect();
                Ok(WriteView::Saved {
                    revision,
                    entry,
                    created_folders,
                })
            }
        }
    }

    /// Re-describes the note at `rel` and records it in the index, so the watcher event for
    /// this change is recognised as our own.
    fn reindex(&self, vault: &Vault, rel: &str) -> Result<Entry> {
        let abs = vault.root.path().join(rel);
        let indexed = scan::index_note(&abs, rel)
            .map_err(|error| Error::io(&error, "reading", name_of(rel)))?
            .ok_or_else(|| Error::not_found(name_of(rel)))?;
        let entry = indexed.entry.clone();
        vault.index().insert(indexed);
        Ok(entry)
    }

    pub fn create_note(&self, parent_rel: &str, title: &str) -> Result<Entry> {
        let (vault, _) = self.vault()?;
        let dir = vault.root.resolve_dir(parent_rel)?;
        let name = write::create_note(&dir, title)?;
        self.reindex(&vault, &paths::join_rel(parent_rel, &name))
    }

    pub fn create_folder(&self, parent_rel: &str, name: &str) -> Result<Entry> {
        let (vault, _) = self.vault()?;
        let dir = vault.root.resolve_dir(parent_rel)?;
        let name = write::create_folder(&dir, name)?;
        let rel = paths::join_rel(parent_rel, &name);
        let meta = fs::symlink_metadata(dir.join(&name))
            .map_err(|error| Error::io(&error, "creating", &name))?;
        let indexed = scan::index_folder(&rel, &meta, vec![]);
        let entry = indexed.entry.clone();
        vault.index().insert(indexed);
        Ok(entry)
    }

    // ------------------------------------------------------------------------------------------
    // Renaming, moving, trashing
    // ------------------------------------------------------------------------------------------

    /// Renames a note or folder in place. For a note, `.md` is added when `new_name` doesn't
    /// end with it. Refuses if the name is taken (ignoring case), except by the entry itself.
    pub fn rename_entry(&self, rel: &str, new_name: &str) -> Result<Entry> {
        let (vault, _) = self.vault()?;
        let source = vault.root.resolve_entry(rel)?;
        let old_name = name_of(rel);
        let typed = paths::nfc(new_name.trim()).into_owned();
        let new_name = match source.kind {
            EntryKind::Note if !paths::is_note_name(&typed) => {
                let extension = &old_name[old_name.len() - paths::NOTE_EXTENSION.len()..];
                format!("{typed}.{extension}")
            }
            _ => typed,
        };
        paths::validate_name(&new_name)?;
        if new_name == old_name {
            return self.entry_at(&vault, rel);
        }
        let parent = paths::parent_rel(rel);
        let dir = vault.root.resolve_dir(parent)?;
        let source_meta = fs::symlink_metadata(&source.abs)
            .map_err(|error| Error::io(&error, "renaming", old_name))?;
        let taken = paths::name_taken(&dir, &new_name, Some(&source_meta))
            .map_err(|error| Error::io(&error, "renaming", old_name))?;
        if taken {
            return Err(Error::already_exists(&new_name));
        }
        let target = dir.join(&new_name);
        let fail = |error: io::Error| Error::io(&error, "renaming", old_name);
        if paths::fold(&new_name) == paths::fold(old_name) {
            // Case- or normalisation-only rename: go through a temporary name, since the volume
            // may consider both names the same file.
            let temp = dir.join(format!(".readback-rename-{}", uuid::Uuid::new_v4()));
            macos::move_item(&source.abs, &temp).map_err(fail)?;
            if let Err(error) = macos::move_item(&temp, &target) {
                let _ = macos::move_item(&temp, &source.abs);
                return Err(fail(error));
            }
        } else {
            macos::move_item(&source.abs, &target).map_err(fail)?;
        }
        self.moved(&vault, rel, &paths::join_rel(parent, &new_name))
    }

    /// Moves a note or folder into another folder, keeping its name.
    pub fn move_entry(&self, rel: &str, new_parent_rel: &str) -> Result<Entry> {
        let (vault, _) = self.vault()?;
        let source = vault.root.resolve_entry(rel)?;
        let name = name_of(rel);
        let target_dir = vault.root.resolve_dir(new_parent_rel)?;
        if paths::fold(paths::parent_rel(rel)) == paths::fold(new_parent_rel) {
            return self.entry_at(&vault, rel);
        }
        if source.kind == EntryKind::Folder
            && paths::is_within(&paths::fold(new_parent_rel), &paths::fold(rel))
        {
            return Err(Error::invalid_operation(
                "move_into_itself",
                format!("“{name}” can't be moved into itself or one of its own folders."),
            ));
        }
        let taken = paths::name_taken(&target_dir, name, None)
            .map_err(|error| Error::io(&error, "moving", name))?;
        if taken {
            return Err(Error::already_exists(name));
        }
        macos::move_item(&source.abs, &target_dir.join(name))
            .map_err(|error| Error::io(&error, "moving", name))?;
        self.moved(&vault, rel, &paths::join_rel(new_parent_rel, name))
    }

    fn entry_at(&self, vault: &Vault, rel: &str) -> Result<Entry> {
        vault
            .index()
            .get(rel)
            .map(|indexed| indexed.entry.clone())
            .ok_or_else(|| Error::not_found(name_of(rel)))
    }

    /// Updates the index after the app moved `from` to `to`.
    fn moved(&self, vault: &Vault, from: &str, to: &str) -> Result<Entry> {
        let fresh = scan::index_path(&vault.root, to)
            .map_err(|error| Error::io(&error, "reading", name_of(to)))?;
        let mut index = vault.index();
        index.remove_subtree(from);
        let entry = fresh
            .first()
            .map(|indexed| indexed.entry.clone())
            .ok_or_else(|| Error::not_found(name_of(to)))?;
        index.insert_all(fresh);
        Ok(entry)
    }

    /// Moves a note or folder (with everything in it) to the Trash.
    pub fn trash_entry(&self, rel: &str) -> Result<()> {
        let (vault, _) = self.vault()?;
        let entry = vault.root.resolve_entry(rel)?;
        macos::trash(&entry.abs)
            .map_err(|error| Error::io(&error, "moving to the Trash", name_of(rel)))?;
        vault.index().remove_subtree(rel);
        Ok(())
    }

    /// The absolute path of an existing note or folder (`""` is the vault folder), for the
    /// opener plugin.
    pub fn locate(&self, rel: &str) -> Result<PathBuf> {
        let (vault, _) = self.vault()?;
        if rel.is_empty() {
            return Ok(vault.root.path().to_path_buf());
        }
        Ok(vault.root.resolve_entry(rel)?.abs)
    }

    /// Called from the watcher thread when a batch needs a full rescan.
    fn rescan_from_watcher(&self, vault: &Arc<Vault>) {
        if !vault.is_active() {
            return;
        }
        let view = self.vault_view();
        if view.status != VaultStatus::Ok {
            self.inner.emit.status(&view);
            return;
        }
        if rescan(vault).is_ok() && vault.is_active() {
            let entries = vault.index().entries().map(|i| i.entry.clone()).collect();
            self.inner.emit.status(&view);
            self.inner.emit.rescanned(&scan::tree(&vault.root, entries));
        }
    }
}

fn rescan(vault: &Vault) -> Result<()> {
    let entries = scan::scan(&vault.root)
        .map_err(|error| Error::io(&error, "reading", "your notes folder"))?;
    vault.index().replace_all(entries);
    Ok(())
}

/// Creates the missing folders of `rel` (inside the vault, never through a link). Returns the
/// folders it created, outermost first.
fn ensure_folders(root: &Root, rel: &str) -> Result<Vec<scan::Indexed>> {
    let mut created = Vec::new();
    let mut current = String::new();
    for segment in Root::segments(rel)? {
        current = paths::join_rel(&current, segment);
        let abs = root.path().join(&current);
        match fs::symlink_metadata(&abs) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => return Err(Error::invalid_path()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&abs).map_err(|error| Error::io(&error, "creating", segment))?;
                let meta = fs::symlink_metadata(&abs)
                    .map_err(|error| Error::io(&error, "creating", segment))?;
                created.push(scan::index_folder(&current, &meta, vec![]));
            }
            Err(error) => return Err(Error::io(&error, "opening", segment)),
        }
    }
    // Re-check through the normal resolver (canonicalisation inside the root).
    root.resolve_dir(rel)?;
    Ok(created)
}

/// Connects a vault's watcher to the service without keeping either alive.
struct WatchSink {
    notes: Weak<Inner>,
    vault: Weak<Vault>,
}

impl watcher::Sink for WatchSink {
    fn changed(&self, changes: &ChangeSet) {
        if let Some(inner) = self.notes.upgrade() {
            inner.emit.changed(changes);
        }
    }

    fn rescan_needed(&self) {
        if let (Some(inner), Some(vault)) = (self.notes.upgrade(), self.vault.upgrade()) {
            Notes { inner }.rescan_from_watcher(&vault);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::errors::ErrorKind;
    use crate::notes::test_support::TempDir;

    #[derive(Default)]
    struct NullEmit;
    impl Emit for NullEmit {
        fn changed(&self, _: &ChangeSet) {}
        fn rescanned(&self, _: &Tree) {}
        fn status(&self, _: &VaultView) {}
    }

    fn service(dir: &TempDir) -> Notes {
        let notes = Notes::new(
            dir.path().join("app-data"),
            Some(dir.path().join("Documents/Readback")),
            Box::new(NullEmit),
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("Documents")).unwrap();
        notes
    }

    fn saved(view: WriteView) -> (String, Entry) {
        match view {
            WriteView::Saved {
                revision, entry, ..
            } => (revision, entry),
            WriteView::Conflict { .. } => panic!("conflict"),
        }
    }

    #[test]
    fn first_launch_then_default_vault() {
        let dir = TempDir::new();
        let notes = service(&dir);
        assert_eq!(notes.vault_view().status, VaultStatus::NotSet);
        assert_eq!(notes.list_tree().unwrap_err().kind, ErrorKind::VaultNotSet);
        let view = notes.create_default_vault().unwrap();
        assert_eq!(view.status, VaultStatus::Ok);
        assert_eq!(view.name.as_deref(), Some("Readback"));
        assert!(notes.list_tree().unwrap().entries.is_empty());
    }

    #[test]
    fn missing_vault_is_reported_and_never_recreated() {
        let dir = TempDir::new();
        let notes = service(&dir);
        let vault = dir.path().join("Vault");
        fs::create_dir(&vault).unwrap();
        notes.set_vault(&vault).unwrap();
        notes.list_tree().unwrap();
        fs::rename(&vault, dir.path().join("Renamed")).unwrap();

        // A fresh service, as on the next launch.
        let notes = service(&dir);
        assert_eq!(notes.vault_view().status, VaultStatus::Missing);
        assert_eq!(notes.list_tree().unwrap_err().kind, ErrorKind::VaultMissing);
        assert!(!vault.exists());
    }

    #[test]
    fn create_edit_rename_move_round_trip() {
        let dir = TempDir::new();
        let notes = service(&dir);
        notes.create_default_vault().unwrap();
        let folder = notes.create_folder("", "Projects").unwrap();
        let note = notes.create_note("", "Ideas: today").unwrap();
        assert_eq!(note.rel_path, "Ideas today.md");

        let read = notes.read_note(&note.rel_path).unwrap();
        assert_eq!(read.content, "");
        let (revision, entry) = saved(
            notes
                .write_note(&note.rel_path, "# Big idea\n", Some(&read.revision))
                .unwrap(),
        );
        assert_eq!(entry.title.as_deref(), Some("Big idea"));

        let renamed = notes.rename_entry(&note.rel_path, "Idea").unwrap();
        assert_eq!(renamed.rel_path, "Idea.md");
        let case_only = notes.rename_entry("Idea.md", "IDEA").unwrap();
        assert_eq!(case_only.rel_path, "IDEA.md");
        let moved = notes.move_entry("IDEA.md", &folder.rel_path).unwrap();
        assert_eq!(moved.rel_path, "Projects/IDEA.md");
        assert_eq!(
            notes.read_note("Projects/IDEA.md").unwrap().revision,
            revision
        );

        let rels: Vec<String> = notes
            .list_tree()
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.rel_path)
            .collect();
        assert_eq!(rels, vec!["Projects", "Projects/IDEA.md"]);
    }

    #[test]
    fn rename_and_move_refuse_existing_names() {
        let dir = TempDir::new();
        let notes = service(&dir);
        notes.create_default_vault().unwrap();
        notes.create_note("", "a").unwrap();
        notes.create_note("", "b").unwrap();
        notes.create_folder("", "F").unwrap();
        fs::write(dir.path().join("Documents/Readback/F/A.md"), "keep").unwrap();
        assert_eq!(
            notes.rename_entry("a.md", "B").unwrap_err().kind,
            ErrorKind::AlreadyExists
        );
        assert_eq!(
            notes.move_entry("a.md", "F").unwrap_err().kind,
            ErrorKind::AlreadyExists
        );
        assert_eq!(
            fs::read(dir.path().join("Documents/Readback/F/A.md")).unwrap(),
            b"keep"
        );
        assert_eq!(
            notes.rename_entry("a.md", ".hidden").unwrap_err().kind,
            ErrorKind::InvalidName
        );
        assert_eq!(
            notes.move_entry("F", "F").unwrap_err().kind,
            ErrorKind::InvalidOperation
        );
    }

    #[test]
    fn recreate_after_folder_deletion() {
        let dir = TempDir::new();
        let notes = service(&dir);
        notes.create_default_vault().unwrap();
        notes.create_folder("", "F").unwrap();
        let note = notes.create_note("F", "n").unwrap();
        let base = notes.read_note(&note.rel_path).unwrap().revision;
        fs::remove_dir_all(dir.path().join("Documents/Readback/F")).unwrap();
        let conflict = notes
            .write_note(&note.rel_path, "mine", Some(&base))
            .unwrap();
        assert!(matches!(
            conflict,
            WriteView::Conflict {
                disk_revision: None
            }
        ));
        match notes.write_note(&note.rel_path, "mine", None).unwrap() {
            WriteView::Saved {
                created_folders, ..
            } => {
                assert_eq!(created_folders.len(), 1);
                assert_eq!(created_folders[0].rel_path, "F");
            }
            WriteView::Conflict { .. } => panic!("conflict"),
        }
        assert_eq!(
            fs::read(dir.path().join("Documents/Readback/F/n.md")).unwrap(),
            b"mine"
        );
    }

    #[test]
    fn switching_vaults_deactivates_the_old_one() {
        let dir = TempDir::new();
        let notes = service(&dir);
        let one = dir.path().join("One");
        let two = dir.path().join("Two");
        fs::create_dir(&one).unwrap();
        fs::create_dir(&two).unwrap();
        fs::write(two.join("t.md"), "").unwrap();
        notes.set_vault(&one).unwrap();
        notes.list_tree().unwrap();
        let old = notes.state().vault.clone().unwrap();
        notes.set_vault(&two).unwrap();
        assert!(!old.is_active());
        assert_eq!(notes.list_tree().unwrap().entries[0].rel_path, "t.md");
    }
}

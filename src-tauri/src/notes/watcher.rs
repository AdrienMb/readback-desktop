//! Live updates: a debounced FSEvents watcher on the vault, reconciled against the index.
//!
//! FSEvents coalesces flags (one event can say created, modified and removed, in no order), so
//! event kinds are only hints. Every path an event names is marked dirty and then checked
//! against the disk:
//!
//! - gone, and in the index → `removed` (a folder takes everything under it with it);
//! - present, and not in the index → `created` (a new folder is scanned, contents included);
//! - present, in the index, different hash → `modified`;
//! - present, in the index, same hash → nothing. That covers the app's own writes (echo
//!   suppression) and saves that didn't change the bytes.
//!
//! A removed and a created entry with the same file identity (device and inode) are one
//! `renamed { from, to }`. An outside editor's atomic save (write a temp file, rename it over
//! the note) keeps the note's path, so it comes out as `modified`, never as a delete.
//!
//! Anything the reconciliation can't handle (a rescan request from FSEvents, watcher errors, an
//! event on the vault folder itself or above it) leads to a full rescan, which is always
//! correct.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use serde::Serialize;

use super::index::Index;
use super::paths::{self, EntryKind, Root};
use super::scan::{self, Entry, Indexed};

/// How long the watcher waits for a burst of events to settle.
pub const DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Renamed {
    pub from: String,
    pub to: String,
    /// The entry at its new path. A folder rename moves everything under `from` to `to`.
    pub entry: Entry,
}

/// The payload of `vault://changed`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSet {
    /// New entries, a new folder's contents included.
    pub created: Vec<Entry>,
    pub modified: Vec<Entry>,
    /// Removed paths. A folder path removes everything under it.
    pub removed: Vec<String>,
    pub renamed: Vec<Renamed>,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.created.is_empty()
            && self.modified.is_empty()
            && self.removed.is_empty()
            && self.renamed.is_empty()
    }
}

/// What a batch of events requires.
#[derive(Debug, PartialEq, Eq)]
pub enum Batch {
    Paths(BTreeSet<String>),
    /// Rescan everything (and check the vault folder still exists).
    Rescan,
}

/// Turns debounced events into dirty relative paths, or a rescan.
pub fn classify(root: &Root, result: &DebounceEventResult) -> Batch {
    let events = match result {
        Ok(events) => events,
        Err(_) => return Batch::Rescan,
    };
    let mut dirty = BTreeSet::new();
    for event in events {
        if event.need_rescan() {
            return Batch::Rescan;
        }
        if matches!(event.kind, EventKind::Access(_)) {
            continue;
        }
        for path in &event.paths {
            match root.rel_of(path) {
                Some(rel) => {
                    if !rel.split('/').any(paths::is_hidden) {
                        dirty.insert(rel);
                    }
                }
                // The vault folder itself, or a folder above it (renamed, unmounted…).
                None if root.path().starts_with(path) => return Batch::Rescan,
                // Siblings seen through the parent watch.
                None => {}
            }
        }
    }
    Batch::Paths(dirty)
}

/// Compares the dirty paths with the disk, updates the index, and says what changed.
pub fn reconcile(root: &Root, index: &mut Index, dirty: &BTreeSet<String>) -> ChangeSet {
    // Parents first, so a folder's subtree is handled once.
    let mut queue: Vec<String> = dirty.iter().cloned().collect();
    queue.sort_by_key(|rel| rel.matches('/').count());

    let mut handled: Vec<String> = Vec::new();
    let mut removed: Vec<Vec<Indexed>> = Vec::new();
    let mut created: Vec<Vec<Indexed>> = Vec::new();
    let mut modified: Vec<Indexed> = Vec::new();

    let mut i = 0;
    while i < queue.len() {
        let rel = queue[i].clone();
        i += 1;
        if handled.iter().any(|done| paths::is_within(&rel, done)) {
            continue;
        }
        let abs = root.path().join(&rel);
        let before = index.get(&rel).cloned();
        let is_folder_now = std::fs::symlink_metadata(&abs).is_ok_and(|meta| meta.is_dir());

        // A folder that is still a folder: compare its direct children only, so entries that
        // changed without an event of their own are caught without rescanning the subtree.
        if is_folder_now
            && !rel.split('/').any(paths::is_hidden)
            && before
                .as_ref()
                .is_some_and(|old| old.entry.kind == EntryKind::Folder)
        {
            let Ok(names) = scan::visible_children(&abs) else {
                continue;
            };
            let on_disk: BTreeSet<String> = names
                .iter()
                .map(|name| paths::fold(&paths::join_rel(&rel, name)))
                .collect();
            for child in index.children(&rel) {
                if !on_disk.contains(&paths::fold(&child.entry.rel_path)) {
                    queue.push(child.entry.rel_path.clone());
                }
            }
            for name in names {
                let child = paths::join_rel(&rel, &name);
                if index.get(&child).is_none() {
                    queue.push(child);
                }
            }
            continue;
        }

        let now = match scan::index_path(root, &rel) {
            Ok(now) => now,
            // Can't look right now (permissions…): leave the index as it is.
            Err(_) => continue,
        };
        let now_top = now.first().filter(|top| top.entry.rel_path == rel);

        match (before, now_top) {
            (None, None) => {}
            (Some(_), None) => {
                removed.push(index.remove_subtree(&rel));
                handled.push(rel);
            }
            (None, Some(_)) => {
                index.insert_all(now.iter().cloned());
                created.push(now);
                handled.push(rel);
            }
            (Some(old), Some(new)) if old.entry.kind != new.entry.kind => {
                removed.push(index.remove_subtree(&rel));
                index.insert_all(now.iter().cloned());
                created.push(now);
                handled.push(rel);
            }
            (Some(old), Some(new)) => {
                // A note that is still a note.
                if old.fingerprint != new.fingerprint {
                    modified.push(new.clone());
                }
                if old != *new {
                    index.insert(new.clone());
                }
            }
        }
    }

    pair_renames(removed, created, modified)
}

/// Matches removed and created entries that are the same file (same device and inode).
fn pair_renames(
    removed: Vec<Vec<Indexed>>,
    mut created: Vec<Vec<Indexed>>,
    modified: Vec<Indexed>,
) -> ChangeSet {
    let mut changes = ChangeSet {
        modified: modified.into_iter().map(|i| i.entry).collect(),
        ..ChangeSet::default()
    };
    for gone in removed {
        let Some(old) = gone.first() else { continue };
        let partner = created.iter().position(|group| {
            group.first().is_some_and(|new| {
                new.dev == old.dev && new.ino == old.ino && new.entry.kind == old.entry.kind
            })
        });
        match partner {
            Some(position) => {
                let group = created.remove(position);
                let new = group[0].clone();
                if new.entry.kind == EntryKind::Note && new.fingerprint != old.fingerprint {
                    changes.modified.push(new.entry.clone());
                }
                changes.renamed.push(Renamed {
                    from: old.entry.rel_path.clone(),
                    to: new.entry.rel_path.clone(),
                    entry: new.entry,
                });
            }
            None => changes.removed.push(old.entry.rel_path.clone()),
        }
    }
    changes.created = created
        .into_iter()
        .flatten()
        .map(|indexed| indexed.entry)
        .collect();
    changes
}

/// Where watcher results go (Tauri events in the app, a channel in tests).
pub trait Sink: Send + Sync + 'static {
    fn changed(&self, changes: &ChangeSet);
    /// Something needs a full rescan; the receiver performs it.
    fn rescan_needed(&self);
}

/// The vault being watched, shared by commands and the watcher thread.
pub struct Vault {
    pub root: Root,
    pub index: std::sync::Mutex<Index>,
    /// Cleared when the user switches vaults, so a late batch from the old watcher is dropped.
    pub active: AtomicBool,
}

impl Vault {
    pub fn new(root: Root) -> Self {
        Self {
            root,
            index: std::sync::Mutex::new(Index::default()),
            active: AtomicBool::new(true),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    /// Locks the index, recovering from a panic in another holder (the index is rebuilt by
    /// the next rescan anyway).
    pub fn index(&self) -> std::sync::MutexGuard<'_, Index> {
        self.index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A running watcher. Dropping it stops it without waiting for its thread.
pub struct Watcher {
    debouncer: Option<Debouncer<RecommendedWatcher, RecommendedCache>>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        if let Some(debouncer) = self.debouncer.take() {
            debouncer.stop_nonblocking();
        }
    }
}

/// Watches the vault recursively, plus its parent folder (not recursively) so that renaming,
/// deleting or unmounting the vault folder itself is noticed.
pub fn start(vault: Arc<Vault>, sink: Arc<dyn Sink>) -> notify::Result<Watcher> {
    let handler_vault = Arc::clone(&vault);
    let mut debouncer = new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
        let vault = &handler_vault;
        if !vault.is_active() {
            return;
        }
        match classify(&vault.root, &result) {
            Batch::Rescan => sink.rescan_needed(),
            Batch::Paths(dirty) if dirty.is_empty() => {}
            Batch::Paths(dirty) => {
                let changes = reconcile(&vault.root, &mut vault.index(), &dirty);
                if !changes.is_empty() && vault.is_active() {
                    sink.changed(&changes);
                }
            }
        }
    })?;
    debouncer.watch(vault.root.path(), RecursiveMode::Recursive)?;
    let parent: Option<PathBuf> = vault.root.path().parent().map(PathBuf::from);
    if let Some(parent) = parent.filter(|p| p.parent().is_some()) {
        // Best effort: without it, renaming the vault folder is noticed on the next command.
        let _ = debouncer.watch(&parent, RecursiveMode::NonRecursive);
    }
    Ok(Watcher {
        debouncer: Some(debouncer),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::note;
    use crate::notes::test_support::TempDir;
    use crate::notes::write::{self, Outcome};
    use std::fs;

    struct Fixture {
        _dir: TempDir,
        root: Root,
        index: Index,
    }

    impl Fixture {
        fn new(files: &[(&str, &str)]) -> Self {
            let dir = TempDir::new();
            for (rel, content) in files {
                let path = dir.path().join(rel);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                if rel.ends_with('/') {
                    fs::create_dir_all(&path).unwrap();
                } else {
                    fs::write(&path, content).unwrap();
                }
            }
            let root = Root::new(dir.path()).unwrap();
            let mut index = Index::default();
            index.replace_all(scan::scan(&root).unwrap());
            Self {
                _dir: dir,
                root,
                index,
            }
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.root.path().join(rel)
        }

        fn reconcile(&mut self, dirty: &[&str]) -> ChangeSet {
            let dirty = dirty.iter().map(|s| (*s).to_owned()).collect();
            reconcile(&self.root, &mut self.index, &dirty)
        }

        fn paths(entries: &[Entry]) -> Vec<&str> {
            entries.iter().map(|e| e.rel_path.as_str()).collect()
        }
    }

    #[test]
    fn own_write_is_suppressed() {
        let mut f = Fixture::new(&[("n.md", "one\n")]);
        let base = note::revision(b"one\n");
        let Outcome::Saved { .. } =
            write::write_note(&f.path("n.md"), "two\n", Some(&base)).unwrap()
        else {
            panic!("not saved")
        };
        // What the service does after a successful write:
        let fresh = scan::index_note(&f.path("n.md"), "n.md").unwrap().unwrap();
        f.index.insert(fresh);
        assert!(f.reconcile(&["n.md"]).is_empty());
    }

    #[test]
    fn outside_edit_is_modified_with_new_title() {
        let mut f = Fixture::new(&[("n.md", "# Old\n")]);
        fs::write(f.path("n.md"), "# New\n").unwrap();
        let changes = f.reconcile(&["n.md"]);
        assert_eq!(Fixture::paths(&changes.modified), vec!["n.md"]);
        assert_eq!(changes.modified[0].title.as_deref(), Some("New"));
        assert!(changes.removed.is_empty() && changes.created.is_empty());
        // A second look finds nothing new.
        assert!(f.reconcile(&["n.md"]).is_empty());
    }

    #[test]
    fn outside_atomic_save_is_modified_not_removed() {
        let mut f = Fixture::new(&[("n.md", "before\n")]);
        // What VS Code / vim do: write a temp file, then rename it over the note.
        fs::write(f.path(".n.md.swp"), "after\n").unwrap();
        fs::rename(f.path(".n.md.swp"), f.path("n.md")).unwrap();
        // FSEvents reports both paths, with removed/created/renamed flags in any order.
        let changes = f.reconcile(&["n.md", ".n.md.swp"]);
        assert_eq!(Fixture::paths(&changes.modified), vec!["n.md"]);
        assert!(changes.removed.is_empty() && changes.created.is_empty());
        assert!(changes.renamed.is_empty());
    }

    #[test]
    fn identical_resave_is_not_an_event() {
        let mut f = Fixture::new(&[("n.md", "same\n")]);
        fs::remove_file(f.path("n.md")).unwrap();
        fs::write(f.path("n.md"), "same\n").unwrap();
        assert!(f.reconcile(&["n.md"]).is_empty());
    }

    #[test]
    fn finder_rename_and_move_are_renames() {
        let mut f = Fixture::new(&[("a.md", "# A\n"), ("Folder/", "")]);
        fs::rename(f.path("a.md"), f.path("Folder/b.md")).unwrap();
        let changes = f.reconcile(&["a.md", "Folder/b.md"]);
        assert_eq!(
            changes.renamed,
            vec![Renamed {
                from: "a.md".into(),
                to: "Folder/b.md".into(),
                entry: f.index.get("Folder/b.md").unwrap().entry.clone(),
            }]
        );
        assert!(changes.created.is_empty() && changes.removed.is_empty());
        assert!(changes.modified.is_empty());
    }

    #[test]
    fn folder_rename_moves_its_contents() {
        let mut f = Fixture::new(&[("Old/x.md", "x"), ("Old/Sub/y.md", "y")]);
        fs::rename(f.path("Old"), f.path("New")).unwrap();
        let changes = f.reconcile(&["Old", "New"]);
        assert_eq!(changes.renamed.len(), 1);
        assert_eq!(changes.renamed[0].from, "Old");
        assert_eq!(changes.renamed[0].to, "New");
        assert!(changes.created.is_empty() && changes.removed.is_empty());
        assert!(f.index.get("New/Sub/y.md").is_some());
        assert!(f.index.get("Old/Sub/y.md").is_none());
    }

    #[test]
    fn deleted_folder_is_one_removal() {
        let mut f = Fixture::new(&[("Gone/x.md", "x"), ("Gone/Sub/y.md", "y"), ("keep.md", "")]);
        fs::remove_dir_all(f.path("Gone")).unwrap();
        // FSEvents may name any subset of the removed paths.
        let changes = f.reconcile(&["Gone/Sub/y.md", "Gone"]);
        assert_eq!(changes.removed, vec!["Gone"]);
        assert_eq!(f.index.len(), 1);
    }

    #[test]
    fn copied_in_folder_is_created_with_contents() {
        let mut f = Fixture::new(&[]);
        fs::create_dir_all(f.path("Imported/Deep")).unwrap();
        fs::write(f.path("Imported/Deep/n.md"), "# N\n").unwrap();
        fs::write(f.path("Imported/pic.png"), "png").unwrap();
        let changes = f.reconcile(&["Imported"]);
        assert_eq!(
            Fixture::paths(&changes.created),
            vec!["Imported", "Imported/Deep", "Imported/Deep/n.md"]
        );
    }

    #[test]
    fn ignored_files_produce_nothing() {
        let mut f = Fixture::new(&[("n.md", "")]);
        fs::write(f.path("pic.png"), "png").unwrap();
        fs::create_dir(f.path(".obsidian")).unwrap();
        fs::write(f.path(".obsidian/workspace.json"), "{}").unwrap();
        fs::write(f.path(".DS_Store"), "").unwrap();
        assert!(
            f.reconcile(&["pic.png", ".obsidian/workspace.json", ".DS_Store"])
                .is_empty()
        );
    }

    #[test]
    fn note_renamed_to_non_note_is_removed() {
        let mut f = Fixture::new(&[("n.md", "")]);
        fs::rename(f.path("n.md"), f.path("n.txt")).unwrap();
        let changes = f.reconcile(&["n.md", "n.txt"]);
        assert_eq!(changes.removed, vec!["n.md"]);
    }

    #[test]
    fn folder_event_finds_children_without_their_own_events() {
        let mut f = Fixture::new(&[("F/a.md", "a"), ("F/b.md", "b")]);
        fs::remove_file(f.path("F/a.md")).unwrap();
        fs::write(f.path("F/c.md"), "c").unwrap();
        let changes = f.reconcile(&["F"]);
        assert_eq!(changes.removed, vec!["F/a.md"]);
        assert_eq!(Fixture::paths(&changes.created), vec!["F/c.md"]);
    }

    #[test]
    fn rescan_requests_and_root_events_force_a_rescan() {
        let f = Fixture::new(&[]);
        let rescan_event =
            notify::Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan);
        let wrap = |event: notify::Event| {
            Ok(vec![notify_debouncer_full::DebouncedEvent::new(
                event,
                std::time::Instant::now(),
            )])
        };
        assert_eq!(classify(&f.root, &wrap(rescan_event)), Batch::Rescan);
        let root_event = notify::Event::new(EventKind::Remove(notify::event::RemoveKind::Folder))
            .add_path(f.root.path().to_path_buf());
        assert_eq!(classify(&f.root, &wrap(root_event)), Batch::Rescan);
        assert_eq!(classify(&f.root, &Err(vec![])), Batch::Rescan);
        let sibling = notify::Event::new(EventKind::Any)
            .add_path(f.root.path().parent().unwrap().join("sibling.md"));
        assert_eq!(
            classify(&f.root, &wrap(sibling)),
            Batch::Paths(BTreeSet::new())
        );
        let hidden = notify::Event::new(EventKind::Any)
            .add_path(f.path(".git/index"))
            .add_path(f.path("n.md"));
        assert_eq!(
            classify(&f.root, &wrap(hidden)),
            Batch::Paths(BTreeSet::from(["n.md".to_owned()]))
        );
    }

    // -----------------------------------------------------------------------------------------
    // Timing-dependent: these run the real FSEvents watcher. They wait for events with generous
    // timeouts (and, for "no event", a fixed quiet window), so they are deterministic in
    // outcome but slower; FSEvents delivery can't be driven by a fake clock.
    // -----------------------------------------------------------------------------------------

    mod timing {
        use super::*;
        use std::sync::mpsc;
        use std::time::Instant;

        struct ChannelSink(std::sync::Mutex<mpsc::Sender<Option<ChangeSet>>>);

        impl Sink for ChannelSink {
            fn changed(&self, changes: &ChangeSet) {
                let _ = self.0.lock().unwrap().send(Some(changes.clone()));
            }
            fn rescan_needed(&self) {
                let _ = self.0.lock().unwrap().send(None);
            }
        }

        fn start_watching(
            dir: &TempDir,
        ) -> (Arc<Vault>, Watcher, mpsc::Receiver<Option<ChangeSet>>) {
            let vault = Arc::new(Vault::new(Root::new(dir.path()).unwrap()));
            vault.index().replace_all(scan::scan(&vault.root).unwrap());
            let (tx, rx) = mpsc::channel();
            let watcher = start(
                Arc::clone(&vault),
                Arc::new(ChannelSink(std::sync::Mutex::new(tx))),
            )
            .unwrap();
            // FSEvents streams start asynchronously; let it settle before changing files.
            std::thread::sleep(Duration::from_millis(500));
            (vault, watcher, rx)
        }

        /// Collects change sets until `done` holds for the merged result, or panics after 10 s.
        fn wait_for(
            rx: &mpsc::Receiver<Option<ChangeSet>>,
            done: impl Fn(&ChangeSet) -> bool,
        ) -> ChangeSet {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut merged = ChangeSet::default();
            while Instant::now() < deadline {
                if let Ok(Some(changes)) = rx.recv_timeout(Duration::from_millis(100)) {
                    merged.created.extend(changes.created);
                    merged.modified.extend(changes.modified);
                    merged.removed.extend(changes.removed);
                    merged.renamed.extend(changes.renamed);
                }
                if done(&merged) {
                    return merged;
                }
            }
            panic!("timed out waiting for watcher events; got {merged:?}");
        }

        #[test]
        fn timing_outside_atomic_save_is_reported_as_modified() {
            let dir = TempDir::new();
            fs::write(dir.path().join("n.md"), "# Before\n").unwrap();
            let (_vault, _watcher, rx) = start_watching(&dir);

            fs::write(dir.path().join(".n.md.tmp"), "# After\n").unwrap();
            fs::rename(dir.path().join(".n.md.tmp"), dir.path().join("n.md")).unwrap();

            let changes = wait_for(&rx, |c| !c.modified.is_empty());
            assert_eq!(changes.modified[0].rel_path, "n.md");
            assert_eq!(changes.modified[0].title.as_deref(), Some("After"));
            assert!(changes.removed.is_empty(), "{changes:?}");
        }

        #[test]
        fn timing_own_write_sends_no_event() {
            let dir = TempDir::new();
            fs::write(dir.path().join("mine.md"), "one\n").unwrap();
            let (vault, _watcher, rx) = start_watching(&dir);

            let path = vault.root.path().join("mine.md");
            let base = note::revision(b"one\n");
            write::write_note(&path, "two\n", Some(&base)).unwrap();
            vault
                .index()
                .insert(scan::index_note(&path, "mine.md").unwrap().unwrap());

            // Then an outside change, which must arrive, and must be the only event.
            std::thread::sleep(Duration::from_millis(1500));
            fs::write(dir.path().join("other.md"), "x").unwrap();
            let changes = wait_for(&rx, |c| !c.created.is_empty());
            assert!(changes.modified.is_empty(), "own write leaked: {changes:?}");
            assert_eq!(changes.created[0].rel_path, "other.md");
        }
    }
}

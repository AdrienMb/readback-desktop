//! What the app last saw in the vault: paths, titles, sizes, hashes and file identities.
//!
//! No note content is kept. The index is rebuilt by every full scan, updated by the app's own
//! operations as they happen, and compared with the disk by the watcher. Because the app's own
//! writes, creates, renames and trashes update it first, the watcher finds nothing new when
//! their events arrive: that is the echo suppression.

use std::collections::BTreeMap;

use super::paths::{self, EntryKind};
use super::scan::Indexed;

#[derive(Debug, Default)]
pub struct Index {
    /// Keyed by `paths::fold(rel_path)`, so lookups ignore case and Unicode normalisation.
    entries: BTreeMap<String, Indexed>,
}

impl Index {
    pub fn replace_all(&mut self, entries: Vec<Indexed>) {
        self.entries = entries
            .into_iter()
            .map(|indexed| (paths::fold(&indexed.entry.rel_path), indexed))
            .collect();
    }

    pub fn get(&self, rel: &str) -> Option<&Indexed> {
        self.entries.get(&paths::fold(rel))
    }

    pub fn insert(&mut self, indexed: Indexed) {
        self.entries
            .insert(paths::fold(&indexed.entry.rel_path), indexed);
    }

    pub fn insert_all(&mut self, entries: impl IntoIterator<Item = Indexed>) {
        for indexed in entries {
            self.insert(indexed);
        }
    }

    /// Removes `rel` and, for a folder, everything under it. Returns what was removed, the
    /// entry itself first.
    pub fn remove_subtree(&mut self, rel: &str) -> Vec<Indexed> {
        let key = paths::fold(rel);
        let mut removed = Vec::new();
        if let Some(indexed) = self.entries.remove(&key) {
            let is_folder = indexed.entry.kind == EntryKind::Folder;
            removed.push(indexed);
            if !is_folder {
                return removed;
            }
        }
        let prefix = format!("{key}/");
        let below: Vec<String> = self
            .entries
            .range(prefix.clone()..)
            .take_while(|(k, _)| k.starts_with(&prefix))
            .map(|(k, _)| k.clone())
            .collect();
        for k in below {
            if let Some(indexed) = self.entries.remove(&k) {
                removed.push(indexed);
            }
        }
        removed
    }

    /// Direct children of the folder `rel` (`""` is the root).
    pub fn children(&self, rel: &str) -> Vec<&Indexed> {
        let prefix = if rel.is_empty() {
            String::new()
        } else {
            format!("{}/", paths::fold(rel))
        };
        self.entries
            .range(prefix.clone()..)
            .take_while(|(key, _)| key.starts_with(&prefix))
            .filter(|(key, _)| !key[prefix.len()..].contains('/'))
            .map(|(_, indexed)| indexed)
            .collect()
    }

    pub fn entries(&self) -> impl Iterator<Item = &Indexed> {
        self.entries.values()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

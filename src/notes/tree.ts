// Pure helpers for the vault tree: applying watcher changes and building the nested view.
import type { TreeNodeData } from '@mantine/core';
import type { ChangeSet, Entry } from './api';

export type Entries = Map<string, Entry>;

export function toEntries(list: Entry[]): Entries {
  return new Map(list.map((entry) => [entry.relPath, entry]));
}

export function parentOf(relPath: string): string {
  const slash = relPath.lastIndexOf('/');
  return slash === -1 ? '' : relPath.slice(0, slash);
}

/** True when `relPath` is `ancestor` or inside it. */
export function isWithin(relPath: string, ancestor: string): boolean {
  return ancestor === '' || relPath === ancestor || relPath.startsWith(`${ancestor}/`);
}

/** Where `relPath` ends up after `from` was renamed to `to` (unchanged if unaffected). */
export function renamedPath(relPath: string, from: string, to: string): string {
  if (relPath === from) return to;
  if (relPath.startsWith(`${from}/`)) return to + relPath.slice(from.length);
  return relPath;
}

function moveSubtree(entries: Entries, from: string, to: string) {
  for (const [path, entry] of [...entries]) {
    if (!isWithin(path, from)) continue;
    entries.delete(path);
    const next = renamedPath(path, from, to);
    entries.set(next, { ...entry, relPath: next });
  }
}

export function removeSubtree(entries: Entries, relPath: string) {
  for (const path of [...entries.keys()]) {
    if (isWithin(path, relPath)) entries.delete(path);
  }
}

/** Applies a watcher change set (renames, then removals, then creations and modifications). */
export function applyChanges(current: Entries, changes: ChangeSet): Entries {
  const next = new Map(current);
  for (const { from, to, entry } of changes.renamed) {
    moveSubtree(next, from, to);
    next.set(to, entry);
  }
  for (const path of changes.removed) removeSubtree(next, path);
  for (const entry of [...changes.created, ...changes.modified]) next.set(entry.relPath, entry);
  return next;
}

export function label(entry: Entry): string {
  return entry.kind === 'note' ? entry.title || entry.name : entry.name;
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });

function compare(a: Entry, b: Entry): number {
  if (a.kind !== b.kind) return a.kind === 'folder' ? -1 : 1;
  return collator.compare(label(a), label(b)) || collator.compare(a.name, b.name);
}

/** Nested data for Mantine's Tree: folders first, then notes, by title. */
export function buildTree(entries: Entries): TreeNodeData[] {
  const children = new Map<string, Entry[]>();
  for (const entry of entries.values()) {
    // Attach to the nearest known folder, so a late event can't orphan an entry.
    let parent = parentOf(entry.relPath);
    while (parent !== '' && entries.get(parent)?.kind !== 'folder') parent = parentOf(parent);
    const list = children.get(parent) ?? [];
    list.push(entry);
    children.set(parent, list);
  }
  const build = (parent: string): TreeNodeData[] =>
    (children.get(parent) ?? []).sort(compare).map((entry) => ({
      value: entry.relPath,
      label: label(entry),
      children: entry.kind === 'folder' ? build(entry.relPath) : undefined,
      nodeProps: { entry },
    }));
  return build('');
}

/** Folders a note or folder could move into ('' is the top level). */
export function moveTargets(entries: Entries, moving: Entry): string[] {
  const current = parentOf(moving.relPath);
  const targets = [...entries.values()]
    .filter((entry) => entry.kind === 'folder')
    .map((entry) => entry.relPath)
    .filter((path) => !(moving.kind === 'folder' && isWithin(path, moving.relPath)))
    .sort((a, b) => collator.compare(a, b));
  return ['', ...targets].filter((path) => path !== current);
}

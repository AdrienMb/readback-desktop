// Typed wrappers around the notes commands. Paths are always relative to the vault, with `/`
// separators; the backend resolves and checks them. Everything returned is display data.
import { invoke } from '@tauri-apps/api/core';
import { type UnlistenFn, listen } from '@tauri-apps/api/event';

export type VaultStatus = 'not_set' | 'ok' | 'missing' | 'denied';

export interface Vault {
  status: VaultStatus;
  path: string | null;
  name: string | null;
  /** Where "Create a Readback folder in Documents" creates it. */
  defaultPath: string | null;
}

export type EntryKind = 'folder' | 'note';
export type EntryFlag = 'too_large' | 'unsupported_encoding' | 'unreadable' | 'offloaded';

export interface Entry {
  kind: EntryKind;
  relPath: string;
  name: string;
  title?: string;
  mtimeMs: number | null;
  size: number;
  flags?: EntryFlag[];
}

export interface Tree {
  rootName: string;
  rootPath: string;
  entries: Entry[];
}

export interface NoteContent {
  content: string;
  revision: string;
}

export type WriteResult =
  | { status: 'saved'; revision: string; entry: Entry; createdFolders: Entry[] }
  | { status: 'conflict'; diskRevision: string | null };

export interface ChangeSet {
  created: Entry[];
  modified: Entry[];
  /** A folder path removes everything under it. */
  removed: string[];
  /** A folder rename moves everything under `from` to `to`. */
  renamed: { from: string; to: string; entry: Entry }[];
}

export type NoteErrorKind =
  | 'vault_not_set'
  | 'vault_missing'
  | 'vault_denied'
  | 'not_found'
  | 'already_exists'
  | 'permission_denied'
  | 'too_large'
  | 'unsupported_encoding'
  | 'offloaded'
  | 'timeout'
  | 'invalid_path'
  | 'invalid_name'
  | 'invalid_operation'
  | 'disk_full'
  | 'other';

export interface NoteError {
  kind: NoteErrorKind;
  code: string;
  message: string;
}

function isNoteError(value: unknown): value is NoteError {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as NoteError).kind === 'string' &&
    typeof (value as NoteError).message === 'string'
  );
}

/** Normalizes a rejected command into a NoteError. */
export function toNoteError(error: unknown): NoteError {
  if (isNoteError(error)) return error;
  return {
    kind: 'other',
    code: 'ipc_error',
    message:
      typeof error === 'string' && error.length > 0
        ? `Readback's backend reported: ${error}`
        : "Readback's window couldn't reach its backend. Restart the app.",
  };
}

export const getVault = () => invoke<Vault>('get_vault');
export const createDefaultVault = () => invoke<Vault>('create_default_vault');
/** Opens the folder picker. `null` when cancelled. */
export const chooseVault = () => invoke<Vault | null>('choose_vault');
export const listTree = () => invoke<Tree>('list_tree');
export const readNote = (relPath: string) => invoke<NoteContent>('read_note', { relPath });
/** `baseRevision: null` creates the note (it must not exist). */
export const writeNote = (relPath: string, content: string, baseRevision: string | null) =>
  invoke<WriteResult>('write_note', { relPath, content, baseRevision });
export const createNote = (parentRel: string, title: string) =>
  invoke<Entry>('create_note', { parentRel, title });
export const createFolder = (parentRel: string, name: string) =>
  invoke<Entry>('create_folder', { parentRel, name });
export const renameEntry = (relPath: string, newName: string) =>
  invoke<Entry>('rename_entry', { relPath, newName });
export const moveEntry = (relPath: string, newParentRel: string) =>
  invoke<Entry>('move_entry', { relPath, newParentRel });
export const trashEntry = (relPath: string) => invoke<void>('trash_entry', { relPath });
/** `''` reveals the vault folder. */
export const revealInFinder = (relPath: string) => invoke<void>('reveal_in_finder', { relPath });
export const openInDefaultApp = (relPath: string) =>
  invoke<void>('open_in_default_app', { relPath });

export const onVaultChanged = (handler: (changes: ChangeSet) => void): Promise<UnlistenFn> =>
  listen<ChangeSet>('vault://changed', (event) => handler(event.payload));
export const onVaultRescanned = (handler: (tree: Tree) => void): Promise<UnlistenFn> =>
  listen<Tree>('vault://rescanned', (event) => handler(event.payload));
export const onVaultStatus = (handler: (vault: Vault) => void): Promise<UnlistenFn> =>
  listen<Vault>('vault://status', (event) => handler(event.payload));

// Quitting: the backend asks before quitting or closing the window, so a pending save runs.
export const onQuitRequested = (handler: (id: number) => void): Promise<UnlistenFn> =>
  listen<number>('app://quit-requested', (event) => handler(event.payload));
export const quitAck = (id: number) => invoke<void>('quit_ack', { id });
export const quitResponse = (id: number, proceed: boolean) =>
  invoke<void>('quit_response', { id, proceed });
export const quitNow = () => invoke<void>('quit_now');

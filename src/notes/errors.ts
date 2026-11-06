// Short titles for note errors. The message from the backend says what happened.
import type { NoteError, NoteErrorKind } from './api';

const TITLES: Record<NoteErrorKind, string> = {
  vault_not_set: 'No notes folder yet',
  vault_missing: 'Notes folder not found',
  vault_denied: 'No access to your notes folder',
  not_found: 'No longer there',
  already_exists: 'That name is already used',
  permission_denied: 'Permission denied',
  too_large: 'Note too large to open',
  unsupported_encoding: 'Unsupported text encoding',
  offloaded: 'Not downloaded',
  timeout: 'Reading took too long',
  invalid_path: 'Not in your notes folder',
  invalid_name: 'Can’t use that name',
  invalid_operation: 'Can’t do that',
  disk_full: 'Can’t save to this drive',
  other: 'Readback couldn’t finish that',
};

export function errorTitle(error: NoteError): string {
  return TITLES[error.kind] ?? TITLES.other;
}

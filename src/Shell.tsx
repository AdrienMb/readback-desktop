import { Alert, Box, Center, Container, Flex, Loader, Stack, Text } from '@mantine/core';
import { useCallback, useEffect, useRef, useState } from 'react';
import { App } from './App';
import {
  type Entry,
  type NoteError,
  type Vault,
  chooseVault,
  createDefaultVault,
  createFolder,
  createNote,
  getVault,
  listTree,
  moveEntry,
  onQuitRequested,
  onVaultChanged,
  onVaultRescanned,
  onVaultStatus,
  openInDefaultApp,
  quitAck,
  quitNow,
  quitResponse,
  renameEntry,
  revealInFinder,
  toNoteError,
  trashEntry,
} from './notes/api';
import { ConfirmDialog, MoveDialog, NameDialog } from './notes/EntryDialogs';
import { NoteEditor, type NoteEditorHandle } from './notes/NoteEditor';
import { NotesErrorNotice } from './notes/NotesErrorNotice';
import { Sidebar } from './notes/Sidebar';
import {
  type Entries,
  applyChanges,
  isWithin,
  moveTargets,
  removeSubtree,
  renamedPath,
  toEntries,
} from './notes/tree';
import { Onboarding, VaultProblem } from './notes/VaultSetup';

type Dialog =
  | { type: 'new_note'; parent: string }
  | { type: 'new_folder'; parent: string }
  | { type: 'rename'; entry: Entry }
  | { type: 'move'; entry: Entry };

interface Confirm {
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel: string;
  resolve: (confirmed: boolean) => void;
}

const SIDEBAR_WIDTH = 280;

function remapExpanded(state: Record<string, boolean>, from: string, to: string) {
  const next: Record<string, boolean> = {};
  for (const [path, open] of Object.entries(state)) next[renamedPath(path, from, to)] = open;
  return next;
}

/** The window: notes sidebar on the left, the open note (or ChatGPT) on the right. */
export function Shell() {
  const [vault, setVault] = useState<Vault | null>(null);
  const [vaultError, setVaultError] = useState<NoteError | null>(null);
  const [vaultBusy, setVaultBusy] = useState<'create' | 'choose' | null>(null);
  const [rootPath, setRootPath] = useState<string | null>(null);
  const [rootName, setRootName] = useState('');
  const [entries, setEntries] = useState<Entries>(new Map());
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [openPath, setOpenPath] = useState<string | null>(null);
  const [docId, setDocId] = useState(0);
  const [view, setView] = useState<'notes' | 'chatgpt'>('notes');
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [actionError, setActionError] = useState<NoteError | null>(null);

  const editorRef = useRef<NoteEditorHandle | null>(null);
  const openPathRef = useRef<string | null>(null);
  const vaultRef = useRef<Vault | null>(null);

  const setOpen = useCallback((path: string | null) => {
    openPathRef.current = path;
    setOpenPath(path);
  }, []);

  const applyVault = useCallback((next: Vault) => {
    vaultRef.current = next;
    setVault(next);
  }, []);

  const ask = useCallback(
    (options: Omit<Confirm, 'resolve'>) =>
      new Promise<boolean>((resolve) => {
        setConfirm({
          ...options,
          resolve: (confirmed) => {
            setConfirm(null);
            resolve(confirmed);
          },
        });
      }),
    [],
  );

  /** Saves the open note. If that isn't possible, asks before its edits are thrown away. */
  const confirmLeave = useCallback(async (): Promise<boolean> => {
    const editor = editorRef.current;
    if (!openPathRef.current || !editor) return true;
    if (await editor.flush()) return true;
    return ask({
      title: 'Unsaved changes',
      message:
        'Your latest changes to this note couldn’t be saved. Resolve the message above the ' +
        'note, or discard the changes.',
      confirmLabel: 'Discard changes',
      cancelLabel: 'Keep editing',
    });
  }, [ask]);

  const loadTree = useCallback(async () => {
    try {
      const tree = await listTree();
      setRootPath(tree.rootPath);
      setRootName(tree.rootName);
      setEntries(toEntries(tree.entries));
      setActionError(null);
    } catch (raw) {
      const error = toNoteError(raw);
      if (error.kind.startsWith('vault_')) applyVault(await getVault());
      else setActionError(error);
    }
  }, [applyVault]);

  const refreshVault = useCallback(
    () =>
      getVault()
        .then(async (next) => {
          const before = vaultRef.current;
          applyVault(next);
          if (next.status === 'ok' && (before?.status !== 'ok' || before.path !== next.path)) {
            await loadTree();
          }
        })
        .catch((raw: unknown) => setVaultError(toNoteError(raw))),
    [applyVault, loadTree],
  );

  useEffect(() => {
    void refreshVault();
    const onFocus = () => void refreshVault();
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [refreshVault]);

  // Live updates from the watcher, and quit requests.
  useEffect(() => {
    const unlisteners = [
      onVaultChanged((changes) => {
        setEntries((current) => applyChanges(current, changes));
        const editor = editorRef.current;
        for (const { from, to } of changes.renamed) {
          setExpanded((state) => remapExpanded(state, from, to));
          const open = openPathRef.current;
          if (open && isWithin(open, from)) {
            const next = renamedPath(open, from, to);
            setOpen(next);
            editor?.follow(next);
          }
        }
        const open = openPathRef.current;
        if (!open) return;
        if (changes.removed.some((path) => isWithin(open, path))) editor?.diskDeleted();
        else if (changes.modified.some((entry) => entry.relPath === open)) editor?.diskModified();
      }),
      onVaultRescanned((tree) => {
        setRootPath(tree.rootPath);
        setRootName(tree.rootName);
        const next = toEntries(tree.entries);
        setEntries(next);
        const open = openPathRef.current;
        if (open) {
          if (next.has(open)) editorRef.current?.diskModified();
          else editorRef.current?.diskDeleted();
        }
      }),
      onVaultStatus((next) => applyVault(next)),
      onQuitRequested((id) => {
        void quitAck(id);
        void (async () => {
          const saved = (await editorRef.current?.flush()) ?? true;
          await quitResponse(id, saved);
          if (!saved) {
            const quit = await ask({
              title: 'Quit with unsaved changes?',
              message:
                'Your latest changes to the open note couldn’t be saved. Quit anyway and lose ' +
                'them, or keep editing to resolve the message above the note.',
              confirmLabel: 'Quit anyway',
              cancelLabel: 'Keep editing',
            });
            if (quit) await quitNow();
          }
        })();
      }),
    ];
    return () => {
      for (const unlisten of unlisteners) void unlisten.then((stop) => stop());
    };
  }, [applyVault, ask, setOpen]);

  // ---------------------------------------------------------------------------------------------
  // Vault choice
  // ---------------------------------------------------------------------------------------------

  async function switchVault(action: () => Promise<Vault | null>, busy: 'create' | 'choose') {
    if (!(await confirmLeave())) return;
    setVaultBusy(busy);
    setVaultError(null);
    try {
      const next = await action();
      if (!next) return;
      setOpen(null);
      setEntries(new Map());
      setExpanded({});
      applyVault(next);
      await loadTree();
      setView('notes');
    } catch (raw) {
      setVaultError(toNoteError(raw));
    } finally {
      setVaultBusy(null);
    }
  }

  // ---------------------------------------------------------------------------------------------
  // Entry actions
  // ---------------------------------------------------------------------------------------------

  async function openNote(entry: Entry) {
    setView('notes');
    if (entry.relPath === openPathRef.current) return;
    if (!(await confirmLeave())) return;
    setOpen(entry.relPath);
    setDocId((id) => id + 1);
  }

  async function closeNote() {
    if (!(await confirmLeave())) return;
    setOpen(null);
  }

  function addEntries(list: Entry[]) {
    setEntries((current) => {
      const next = new Map(current);
      for (const entry of list) next.set(entry.relPath, entry);
      return next;
    });
  }

  /** Before renaming, moving or trashing `entry`: save the open note if it's inside. */
  async function saveIfAffected(entry: Entry): Promise<boolean> {
    const open = openPathRef.current;
    if (!open || !isWithin(open, entry.relPath)) return true;
    return confirmLeave();
  }

  function followMove(from: string, moved: Entry) {
    setEntries((current) => {
      const next = applyChanges(current, {
        created: [],
        modified: [],
        removed: [],
        renamed: [{ from, to: moved.relPath, entry: moved }],
      });
      return next;
    });
    setExpanded((state) => remapExpanded(state, from, moved.relPath));
    const open = openPathRef.current;
    if (open && isWithin(open, from)) {
      const next = renamedPath(open, from, moved.relPath);
      setOpen(next);
      editorRef.current?.follow(next);
    }
  }

  async function submitDialog(value: string) {
    if (!dialog) return;
    switch (dialog.type) {
      case 'new_note': {
        const entry = await createNote(dialog.parent, value);
        addEntries([entry]);
        if (dialog.parent) setExpanded((state) => ({ ...state, [dialog.parent]: true }));
        await openNote(entry);
        break;
      }
      case 'new_folder': {
        const entry = await createFolder(dialog.parent, value);
        addEntries([entry]);
        if (dialog.parent) setExpanded((state) => ({ ...state, [dialog.parent]: true }));
        break;
      }
      case 'rename': {
        if (!(await saveIfAffected(dialog.entry))) return;
        const moved = await renameEntry(dialog.entry.relPath, value);
        followMove(dialog.entry.relPath, moved);
        break;
      }
      case 'move': {
        if (!(await saveIfAffected(dialog.entry))) return;
        const moved = await moveEntry(dialog.entry.relPath, value);
        followMove(dialog.entry.relPath, moved);
        if (value) setExpanded((state) => ({ ...state, [value]: true }));
        break;
      }
    }
  }

  async function trash(entry: Entry) {
    setActionError(null);
    if (!(await saveIfAffected(entry))) return;
    try {
      await trashEntry(entry.relPath);
      setEntries((current) => {
        const next = new Map(current);
        removeSubtree(next, entry.relPath);
        return next;
      });
      const open = openPathRef.current;
      if (open && isWithin(open, entry.relPath)) setOpen(null);
      setNotice(`Moved “${entry.name}” to the Trash.`);
    } catch (raw) {
      setActionError(toNoteError(raw));
    }
  }

  async function run(action: () => Promise<void>) {
    setActionError(null);
    try {
      await action();
    } catch (raw) {
      setActionError(toNoteError(raw));
    }
  }

  // ---------------------------------------------------------------------------------------------
  // Layout
  // ---------------------------------------------------------------------------------------------

  const ready = vault?.status === 'ok';
  const problem = vault && (vault.status === 'missing' || vault.status === 'denied');
  const openEntry = openPath ? entries.get(openPath) : undefined;

  return (
    <Flex h="100vh" style={{ overflow: 'hidden' }}>
      <Box
        w={SIDEBAR_WIDTH}
        style={{ flexShrink: 0, borderRight: '1px solid var(--mantine-color-default-border)' }}
      >
        <Sidebar
          vault={vault}
          rootPath={ready ? rootPath : null}
          entries={ready ? entries : new Map()}
          openPath={openPath}
          expanded={expanded}
          onExpandedChange={setExpanded}
          view={view}
          onShowChatGpt={() => setView('chatgpt')}
          onChangeVault={() => void switchVault(chooseVault, 'choose')}
          changingVault={vaultBusy === 'choose'}
          onOpen={(entry) => void openNote(entry)}
          onNewNote={(parent) => setDialog({ type: 'new_note', parent })}
          onNewFolder={(parent) => setDialog({ type: 'new_folder', parent })}
          onRename={(entry) => setDialog({ type: 'rename', entry })}
          onMove={(entry) => setDialog({ type: 'move', entry })}
          onTrash={(entry) => void trash(entry)}
          onReveal={(relPath) => void run(() => revealInFinder(relPath))}
          onOpenInDefaultApp={(entry) =>
            void run(async () => {
              if (await saveIfAffected(entry)) await openInDefaultApp(entry.relPath);
            })
          }
        />
      </Box>

      <Box style={{ flex: 1, minWidth: 0, overflow: 'auto' }}>
        {/* Kept mounted so the ChatGPT panel keeps its state while a note is open. */}
        <Box display={view === 'chatgpt' ? 'block' : 'none'}>
          <App />
        </Box>

        <Stack gap={0} h="100%" display={view === 'notes' ? 'flex' : 'none'}>
          {(notice || actionError || problem || vaultError) && (
            <Stack gap="xs" p="md" pb={0}>
              {notice && (
                <Alert color="gray" variant="light" withCloseButton onClose={() => setNotice(null)}>
                  {notice}
                </Alert>
              )}
              {actionError && (
                <NotesErrorNotice error={actionError} onClose={() => setActionError(null)} />
              )}
              {problem && vault && (
                <VaultProblem
                  vault={vault}
                  busy={vaultBusy !== null}
                  error={vaultError}
                  onLocate={() => void switchVault(chooseVault, 'choose')}
                  onChooseAnother={() => void switchVault(chooseVault, 'choose')}
                  onRetry={() => void refreshVault()}
                />
              )}
              {vaultError && !problem && vault?.status !== 'not_set' && (
                <NotesErrorNotice error={vaultError} onClose={() => setVaultError(null)} />
              )}
            </Stack>
          )}

          {!vault && !vaultError && (
            <Center h="100%">
              <Loader />
            </Center>
          )}

          {vault?.status === 'not_set' && (
            <Container size="sm" py="xl" w="100%">
              <Onboarding
                vault={vault}
                busy={vaultBusy}
                error={vaultError}
                onCreate={() => void switchVault(createDefaultVault, 'create')}
                onChoose={() => void switchVault(chooseVault, 'choose')}
              />
            </Container>
          )}

          {openPath && (
            <Box style={{ flex: 1, minHeight: 0 }}>
              <NoteEditor
                key={docId}
                ref={editorRef}
                relPath={openPath}
                entry={openEntry}
                onEntries={addEntries}
                onClose={() => void closeNote()}
                onDiscard={() => setOpen(null)}
              />
            </Box>
          )}

          {ready && !openPath && (
            <Center style={{ flex: 1 }} p="xl">
              <Text c="dimmed" size="sm" ta="center">
                Select a note, or create one with New note.
                <br />
                Changes you make in other apps show up here automatically.
              </Text>
            </Center>
          )}
        </Stack>
      </Box>

      {(dialog?.type === 'new_note' || dialog?.type === 'new_folder') && (
        <NameDialog
          title={dialog.type === 'new_note' ? 'New note' : 'New folder'}
          label={dialog.type === 'new_note' ? 'Title' : 'Folder name'}
          placeholder={dialog.type === 'new_note' ? 'Untitled' : 'New folder'}
          description={
            dialog.parent ? `In ${dialog.parent}` : `In ${rootName || 'your notes folder'}`
          }
          submitLabel="Create"
          initialValue=""
          onSubmit={(value) =>
            submitDialog(value || (dialog.type === 'new_note' ? 'Untitled' : 'New folder'))
          }
          onClose={() => setDialog(null)}
        />
      )}
      {dialog?.type === 'rename' && (
        <NameDialog
          title={`Rename “${dialog.entry.name}”`}
          label="Name"
          description={dialog.entry.kind === 'note' ? '“.md” is added for you.' : undefined}
          submitLabel="Rename"
          initialValue={
            dialog.entry.kind === 'note'
              ? dialog.entry.name.replace(/\.md$/i, '')
              : dialog.entry.name
          }
          onSubmit={submitDialog}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog?.type === 'move' && (
        <MoveDialog
          name={dialog.entry.name}
          rootName={rootName}
          targets={moveTargets(entries, dialog.entry)}
          onSubmit={submitDialog}
          onClose={() => setDialog(null)}
        />
      )}
      {confirm && (
        <ConfirmDialog
          title={confirm.title}
          message={confirm.message}
          confirmLabel={confirm.confirmLabel}
          cancelLabel={confirm.cancelLabel}
          onConfirm={() => confirm.resolve(true)}
          onCancel={() => confirm.resolve(false)}
        />
      )}
    </Flex>
  );
}

import { Alert, Box, Button, Group, Loader, Stack, Text, Tooltip } from '@mantine/core';
import { useCallback, useEffect, useImperativeHandle, useRef, useState } from 'react';
import { type Entry, type NoteError, readNote, toNoteError, writeNote } from './api';
import { errorTitle } from './errors';
import { MarkdownEditor, type MarkdownEditorHandle } from './MarkdownEditor';
import { MarkdownView } from './MarkdownView';
import classes from './NoteEditor.module.css';
import { NotesErrorNotice } from './NotesErrorNotice';

/** How long typing must pause before the note is saved. */
export const AUTOSAVE_DELAY_MS = 800;

export interface NoteEditorHandle {
  /** Saves any pending edit. Resolves `true` when nothing unsaved remains. */
  flush: () => Promise<boolean>;
  /** The note was renamed or moved: keep editing it at its new path. */
  follow: (relPath: string) => void;
  /** The watcher reported the note changed on disk. */
  diskModified: () => void;
  /** The watcher reported the note (or its folder) was removed. */
  diskDeleted: () => void;
}

export type NoteMode = 'view' | 'edit';

type Banner =
  | { kind: 'changed'; revision: string; content: string }
  | { kind: 'unreadable'; error: NoteError }
  | { kind: 'deleted' };

interface Props {
  relPath: string;
  entry: Entry | undefined;
  /** Rendered Markdown, or the CodeMirror editor. */
  mode: NoteMode;
  ref: React.Ref<NoteEditorHandle>;
  /** Entries the editor created or changed (after a save: new title, recreated folders). */
  onEntries: (entries: Entry[]) => void;
  /** Close without saving (the user chose to). */
  onDiscard: () => void;
}

/**
 * Edits one note. The editor's text is the only copy of unsaved edits; it is written back
 * (with the revision it started from) after a pause in typing, and before the note is closed,
 * another note is opened, or the app quits.
 */
export function NoteEditor({
  relPath: initialPath,
  entry,
  mode,
  ref,
  onEntries,
  onDiscard,
}: Props) {
  const [text, setText] = useState('');
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<NoteError | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<NoteError | null>(null);
  const [banner, setBannerState] = useState<Banner | null>(null);
  const [showBoth, setShowBoth] = useState(false);
  const [path, setPath] = useState(initialPath);
  const [savedText, setSavedText] = useState('');

  // The logic works on refs, so event handlers and timers never see stale values.
  const pathRef = useRef(initialPath);
  const textRef = useRef('');
  /** Text matching `baseRef` on disk. Dirty when it differs from `textRef`. */
  const savedRef = useRef('');
  const baseRef = useRef<string | null>(null);
  const loadedRef = useRef(false);
  const bannerRef = useRef<Banner | null>(null);
  const savingRef = useRef<Promise<void> | null>(null);
  const timerRef = useRef<number | null>(null);
  const editorRef = useRef<MarkdownEditorHandle>(null);
  const modeRef = useRef(mode);
  const onEntriesRef = useRef(onEntries);
  useEffect(() => {
    onEntriesRef.current = onEntries;
  }, [onEntries]);

  /** Records `content` as what's on disk at `revision`. */
  const setSaved = useCallback((content: string, revision: string | null) => {
    savedRef.current = content;
    baseRef.current = revision;
    setSavedText(content);
  }, []);

  const setBanner = useCallback((next: Banner | null) => {
    bannerRef.current = next;
    setBannerState(next);
    if (!next) setShowBoth(false);
  }, []);

  /** Replaces the editor text (from disk). The editor maps the cursor through the change. */
  const replaceText = useCallback((next: string) => {
    textRef.current = next;
    setText(next);
  }, []);

  const cancelTimer = () => {
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  };

  const waitForSave = async () => {
    while (savingRef.current) await savingRef.current;
  };

  const isDirty = () => textRef.current !== savedRef.current;

  /** Reads the disk version and decides: nothing changed, reload in place, or ask. */
  const decideFromDisk = useCallback(async () => {
    let disk;
    try {
      disk = await readNote(pathRef.current);
    } catch (raw) {
      const error = toNoteError(raw);
      setBanner(error.kind === 'not_found' ? { kind: 'deleted' } : { kind: 'unreadable', error });
      return;
    }
    if (disk.revision === baseRef.current) return;
    if (!isDirty() && !bannerRef.current) {
      setSaved(disk.content, disk.revision);
      replaceText(disk.content);
      return;
    }
    setBanner({ kind: 'changed', revision: disk.revision, content: disk.content });
  }, [replaceText, setBanner, setSaved]);

  const checkDisk = useCallback(async () => {
    await waitForSave();
    await decideFromDisk();
  }, [decideFromDisk]);

  const save = useCallback(async (): Promise<void> => {
    await waitForSave();
    if (!loadedRef.current || bannerRef.current || !isDirty()) return;
    const snapshot = textRef.current;
    const path = pathRef.current;
    const base = baseRef.current;
    setSaving(true);
    const run = (async () => {
      try {
        const result = await writeNote(path, snapshot, base);
        if (result.status === 'saved') {
          setSaved(snapshot, result.revision);
          setSaveError(null);
          onEntriesRef.current([...result.createdFolders, result.entry]);
        } else if (result.diskRevision === null) {
          setBanner({ kind: 'deleted' });
        } else {
          await decideFromDisk();
        }
      } catch (raw) {
        setSaveError(toNoteError(raw));
      }
    })();
    savingRef.current = run;
    await run;
    if (savingRef.current === run) savingRef.current = null;
    setSaving(false);
  }, [decideFromDisk, setBanner, setSaved]);

  /** Saves after a pause in typing. Every edit reschedules, so text typed during a save is
   * saved after the next pause. */
  const scheduleSave = useCallback(() => {
    cancelTimer();
    timerRef.current = window.setTimeout(() => {
      timerRef.current = null;
      void save();
    }, AUTOSAVE_DELAY_MS);
  }, [save]);

  const load = useCallback(
    () =>
      readNote(pathRef.current)
        .then((note) => {
          setSaved(note.content, note.revision);
          textRef.current = note.content;
          setText(note.content);
          loadedRef.current = true;
        })
        .catch((raw: unknown) => setLoadError(toNoteError(raw)))
        .finally(() => setLoading(false)),
    [setSaved],
  );

  function retryLoad() {
    setLoading(true);
    setLoadError(null);
    void load();
  }

  useEffect(() => {
    void load();
    return cancelTimer;
  }, [load]);

  // Switching to view saves any pending edit; switching to edit puts the cursor in the note.
  useEffect(() => {
    if (modeRef.current === mode) return;
    modeRef.current = mode;
    if (mode === 'edit') {
      editorRef.current?.focus();
    } else if (isDirty() && !bannerRef.current) {
      cancelTimer();
      void save();
    }
  }, [mode, save]);

  useImperativeHandle(
    ref,
    () => ({
      flush: async () => {
        cancelTimer();
        if (!loadedRef.current) return true;
        await save();
        await waitForSave();
        return !isDirty() && !bannerRef.current;
      },
      follow: (relPath) => {
        pathRef.current = relPath;
        setPath(relPath);
        // A save that raced with an outside rename may have reported the note as deleted.
        if (bannerRef.current?.kind === 'deleted') {
          setBanner(null);
          void checkDisk().then(() => {
            if (isDirty() && !bannerRef.current) scheduleSave();
          });
        }
      },
      diskModified: () => {
        if (loadedRef.current) void checkDisk();
      },
      diskDeleted: () => {
        void waitForSave().then(() => setBanner({ kind: 'deleted' }));
      },
    }),
    [checkDisk, save, scheduleSave, setBanner],
  );

  function onChange(next: string) {
    textRef.current = next;
    setText(next);
    if (!bannerRef.current) scheduleSave();
  }

  async function keepMine() {
    if (bannerRef.current?.kind !== 'changed') return;
    // Overwrite: the disk version becomes the base.
    setSaved(bannerRef.current.content, bannerRef.current.revision);
    setBanner(null);
    await save();
  }

  function loadDisk() {
    if (bannerRef.current?.kind !== 'changed') return;
    const { content, revision } = bannerRef.current;
    setSaved(content, revision);
    replaceText(content);
    setBanner(null);
  }

  async function recreate() {
    await waitForSave();
    const snapshot = textRef.current;
    try {
      const result = await writeNote(pathRef.current, snapshot, null);
      if (result.status === 'saved') {
        setSaved(snapshot, result.revision);
        setSaveError(null);
        setBanner(null);
        onEntriesRef.current([...result.createdFolders, result.entry]);
      } else {
        // Something is at that path again: compare with it.
        setBanner(null);
        await checkDisk();
      }
    } catch (raw) {
      setSaveError(toNoteError(raw));
    }
  }

  const title = entry?.title ?? path.split('/').pop() ?? '';
  const dirty = text !== savedText;
  let status: React.ReactNode;
  if (loading) status = 'Opening…';
  else if (banner) status = 'Not saved';
  else if (saveError) status = `Not saved: ${errorTitle(saveError)}`;
  else if (saving || dirty) status = 'Saving…';
  else status = 'Saved';

  return (
    <div className={classes.pane}>
      <div className={classes.status}>
        {saveError && (
          <>
            <Tooltip label={saveError.message} disabled={!saveError} multiline w={260}>
              <Text size="xs" c={'red'} aria-live="polite">
                {status}
              </Text>
            </Tooltip>
            <Button size="compact-xs" variant="default" onClick={() => void save()}>
              Retry
            </Button>
          </>
        )}
      </div>

      <div className={classes.scroller}>
        <Stack
          gap="md"
          className={classes.column}
          data-wide={(showBoth && banner?.kind === 'changed') || undefined}
        >
          {banner?.kind === 'changed' && (
            <Alert color="yellow" variant="light" title="This note changed on disk.">
              <Stack gap="xs">
                <Text size="sm">
                  Another app saved a new version while you had unsaved edits here. Choose which to
                  keep.
                </Text>
                <Group gap="xs">
                  <Button size="xs" onClick={() => void keepMine()}>
                    Keep mine
                  </Button>
                  <Button size="xs" variant="default" onClick={loadDisk}>
                    Load disk version
                  </Button>
                  <Button size="xs" variant="subtle" onClick={() => setShowBoth((v) => !v)}>
                    {showBoth ? 'Hide disk version' : 'Show both'}
                  </Button>
                </Group>
              </Stack>
            </Alert>
          )}
          {banner?.kind === 'deleted' && (
            <Alert color="red" variant="light" title="This note was deleted on disk.">
              <Stack gap="xs">
                <Text size="sm">
                  It was deleted or moved out of your notes folder by another app. Your text is
                  still here: you can save it again at the same place, or close it.
                </Text>
                <Group gap="xs">
                  <Button size="xs" onClick={() => void recreate()}>
                    Recreate from my copy
                  </Button>
                  <Button size="xs" variant="default" color="red" onClick={onDiscard}>
                    Close
                  </Button>
                </Group>
              </Stack>
            </Alert>
          )}
          {banner?.kind === 'unreadable' && (
            <Alert color="red" variant="light" title="This note changed on disk.">
              <Stack gap="xs">
                <Text size="sm">
                  Readback can’t read the new version, so your edits here aren’t saved.{' '}
                  {banner.error.message}
                </Text>
                <Group gap="xs">
                  <Button
                    size="xs"
                    variant="default"
                    onClick={() => {
                      setBanner(null);
                      void checkDisk();
                    }}
                  >
                    Check again
                  </Button>
                  <Button size="xs" variant="default" color="red" onClick={onDiscard}>
                    Close
                  </Button>
                </Group>
              </Stack>
            </Alert>
          )}

          {loading && (
            <Box py="xl" ta="center">
              <Loader size="sm" />
            </Box>
          )}
          {loadError && <NotesErrorNotice error={loadError} onRetry={retryLoad} />}

          {!loading && !loadError && (
            <Group gap="xl" align="flex-start" wrap="nowrap">
              <Box style={{ flex: 1, minWidth: 0 }}>
                {/* Kept mounted in view mode so undo history and the cursor survive a toggle. */}
                <Box display={mode === 'edit' ? 'block' : 'none'}>
                  <MarkdownEditor
                    ref={editorRef}
                    ariaLabel={`Text of ${title}`}
                    placeholder="Start writing…"
                    value={text}
                    onChange={onChange}
                    onBlur={() => {
                      if (isDirty() && !bannerRef.current) {
                        cancelTimer();
                        void save();
                      }
                    }}
                  />
                </Box>
                {mode === 'view' && <MarkdownView content={text} ariaLabel={title} />}
              </Box>
              {showBoth && banner?.kind === 'changed' && (
                <Stack gap={4} style={{ flex: 1, minWidth: 0 }}>
                  <Text size="xs" c="dimmed">
                    On disk (read-only)
                  </Text>
                  {mode === 'edit' ? (
                    <MarkdownEditor readOnly ariaLabel="Disk version" value={banner.content} />
                  ) : (
                    <MarkdownView content={banner.content} ariaLabel="Disk version" muted />
                  )}
                </Stack>
              )}
            </Group>
          )}
        </Stack>
      </div>
    </div>
  );
}

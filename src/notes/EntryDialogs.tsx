import { Button, Group, Modal, Stack, Text, TextInput } from '@mantine/core';
import { useState } from 'react';
import { type NoteError, toNoteError } from './api';
import { NotesErrorNotice } from './NotesErrorNotice';

interface NameDialogProps {
  title: string;
  label: string;
  submitLabel: string;
  initialValue: string;
  placeholder?: string;
  description?: string;
  onSubmit: (value: string) => Promise<void>;
  onClose: () => void;
}

/** Asks for a name (new note, new folder, rename). Errors stay in the dialog. */
export function NameDialog({
  title,
  label,
  submitLabel,
  initialValue,
  placeholder,
  description,
  onSubmit,
  onClose,
}: NameDialogProps) {
  const [value, setValue] = useState(initialValue);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<NoteError | null>(null);

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await onSubmit(value);
      onClose();
    } catch (raw) {
      setError(toNoteError(raw));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal opened onClose={onClose} title={title} centered>
      <form onSubmit={(event) => void submit(event)}>
        <Stack gap="md">
          <TextInput
            label={label}
            description={description}
            placeholder={placeholder}
            value={value}
            onChange={(event) => setValue(event.currentTarget.value)}
            data-autofocus
            onFocus={(event) => event.currentTarget.select()}
          />
          {error && <NotesErrorNotice error={error} />}
          <Group justify="flex-end">
            <Button variant="default" onClick={onClose} disabled={busy}>
              Cancel
            </Button>
            <Button type="submit" loading={busy}>
              {submitLabel}
            </Button>
          </Group>
        </Stack>
      </form>
    </Modal>
  );
}

interface ConfirmDialogProps {
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  title,
  message,
  confirmLabel,
  cancelLabel,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  return (
    <Modal opened onClose={onCancel} title={title} centered>
      <Stack gap="md">
        <Text size="sm">{message}</Text>
        <Group justify="flex-end">
          <Button variant="default" onClick={onCancel} data-autofocus>
            {cancelLabel}
          </Button>
          <Button color="red" onClick={onConfirm}>
            {confirmLabel}
          </Button>
        </Group>
      </Stack>
    </Modal>
  );
}

import { Alert, Button, Card, Code, Group, Stack, Text, Title } from '@mantine/core';
import type { NoteError, Vault } from './api';
import { NotesErrorNotice } from './NotesErrorNotice';

interface OnboardingProps {
  vault: Vault;
  busy: 'create' | 'choose' | null;
  error: NoteError | null;
  onCreate: () => void;
  onChoose: () => void;
}

/** First launch: no notes folder configured. */
export function Onboarding({ vault, busy, error, onCreate, onChoose }: OnboardingProps) {
  return (
    <Card withBorder radius="md" padding="lg">
      <Stack gap="md">
        <Title order={3}>Choose where your notes live</Title>
        <Text size="sm">
          Your notes are plain Markdown files in a folder on this Mac. You can edit them with any
          app, and Readback picks up the changes.
        </Text>
        <Stack gap={6} align="flex-start">
          <Button onClick={onCreate} loading={busy === 'create'} disabled={busy !== null}>
            Create a Readback folder in Documents
          </Button>
          {vault.defaultPath && (
            <Text size="xs" c="dimmed">
              Creates <Code>{vault.defaultPath}</Code>
            </Text>
          )}
        </Stack>
        <Stack gap={6} align="flex-start">
          <Button
            variant="default"
            onClick={onChoose}
            loading={busy === 'choose'}
            disabled={busy !== null}
          >
            Use an existing folder
          </Button>
          <Text size="xs" c="dimmed">
            Any folder of Markdown files works, such as an Obsidian vault. Readback doesn’t change
            files it doesn’t need to.
          </Text>
        </Stack>
        {error && <NotesErrorNotice error={error} />}
      </Stack>
    </Card>
  );
}

interface ProblemProps {
  vault: Vault;
  busy: boolean;
  error: NoteError | null;
  onLocate: () => void;
  onChooseAnother: () => void;
  onRetry: () => void;
}

/** The configured folder is missing (moved, renamed, unmounted) or not readable. */
export function VaultProblem({
  vault,
  busy,
  error,
  onLocate,
  onChooseAnother,
  onRetry,
}: ProblemProps) {
  const denied = vault.status === 'denied';
  return (
    <Alert
      color="yellow"
      variant="light"
      title={denied ? 'Readback can’t open your notes folder' : 'Your notes folder is missing'}
    >
      <Stack gap="sm">
        <Text size="sm">
          {denied
            ? 'macOS isn’t letting Readback read this folder. Allow access in System Settings → ' +
              'Privacy & Security → Files and Folders, or choose another folder.'
            : 'It may have been moved, renamed or deleted, or it’s on a drive that isn’t ' +
              'connected. Readback won’t create a new empty folder in its place.'}
        </Text>
        {vault.path && <Code block>{vault.path}</Code>}
        <Group gap="sm">
          <Button size="xs" onClick={onLocate} loading={busy}>
            Locate folder
          </Button>
          <Button size="xs" variant="default" onClick={onChooseAnother} disabled={busy}>
            Choose another folder
          </Button>
          <Button size="xs" variant="subtle" onClick={onRetry} disabled={busy}>
            Check again
          </Button>
        </Group>
        {error && <NotesErrorNotice error={error} />}
      </Stack>
    </Alert>
  );
}

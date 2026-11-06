import { Alert, Button, Group, Stack, Text } from '@mantine/core';
import type { NoteError } from './api';
import { errorTitle } from './errors';

interface Props {
  error: NoteError;
  onRetry?: () => void;
  onClose?: () => void;
}

export function NotesErrorNotice({ error, onRetry, onClose }: Props) {
  const color = error.kind === 'offloaded' || error.kind === 'timeout' ? 'yellow' : 'red';
  return (
    <Alert
      color={color}
      title={errorTitle(error)}
      variant="light"
      withCloseButton={Boolean(onClose)}
      onClose={onClose}
    >
      <Stack gap="xs">
        <Text size="sm">{error.message}</Text>
        {onRetry && (
          <Group>
            <Button size="xs" variant="default" onClick={onRetry}>
              Try again
            </Button>
          </Group>
        )}
      </Stack>
    </Alert>
  );
}

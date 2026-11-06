import {
  Badge,
  Button,
  Card,
  Group,
  Loader,
  Paper,
  Select,
  Stack,
  Text,
  Textarea,
} from '@mantine/core';
import { useCallback, useEffect, useState } from 'react';
import {
  type AppError,
  type Model,
  cancelPrompt,
  listModels,
  sendPrompt,
  toAppError,
} from '../api';
import { ErrorNotice } from './ErrorNotice';
import { ManageUsageButton } from './ManageUsageButton';

interface Props {
  /** Called after an error that may have changed the session (e.g. expired sign-in). */
  onSessionMaybeChanged: () => void;
  onSignIn: () => void;
  onEnableSharing: () => void;
}

export function PromptPanel({ onSessionMaybeChanged, onSignIn, onEnableSharing }: Props) {
  const [models, setModels] = useState<Model[] | null>(null);
  const [modelsError, setModelsError] = useState<AppError | null>(null);
  const [model, setModel] = useState<string | null>(null);
  const [prompt, setPrompt] = useState('');
  const [response, setResponse] = useState('');
  const [streaming, setStreaming] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const [cancelled, setCancelled] = useState(false);

  const handleError = useCallback(
    (raw: unknown, set: (error: AppError) => void) => {
      const appError = toAppError(raw);
      set(appError);
      if (appError.kind === 'reauth' || appError.kind === 'sharing_disabled') {
        onSessionMaybeChanged();
      }
    },
    [onSessionMaybeChanged],
  );

  const loadModels = useCallback(() => {
    listModels()
      .then((list) => {
        setModelsError(null);
        setModels(list);
        setModel((current) =>
          current && list.some((m) => m.slug === current) ? current : (list[0]?.slug ?? null),
        );
      })
      .catch((raw: unknown) => handleError(raw, setModelsError));
  }, [handleError]);

  useEffect(loadModels, [loadModels]);

  async function send() {
    if (!model || !prompt.trim()) return;
    setResponse('');
    setError(null);
    setCancelled(false);
    setStreaming(true);
    try {
      await sendPrompt(model, prompt, (text) => setResponse((current) => current + text));
    } catch (raw) {
      if (toAppError(raw).kind === 'cancelled') setCancelled(true);
      else handleError(raw, setError);
    } finally {
      setStreaming(false);
    }
  }

  const retryModels = () => {
    setModelsError(null);
    setModels(null);
    loadModels();
  };

  return (
    <Card withBorder radius="md" padding="lg">
      <Stack gap="md">
        <Group justify="space-between" align="flex-end" wrap="wrap">
          <Select
            label="Model"
            placeholder={models ? 'Choose a model' : 'Loading models…'}
            data={(models ?? []).map((m) => ({ value: m.slug, label: m.displayName }))}
            value={model}
            onChange={setModel}
            disabled={!models || models.length === 0 || streaming}
            rightSection={!models && !modelsError ? <Loader size="xs" /> : undefined}
            allowDeselect={false}
            w={260}
          />
          <Group gap="xs">
            <Badge variant="light" color="gray" size="lg" radius="sm" tt="none" fw={500}>
              Using ChatGPT plan
            </Badge>
            <ManageUsageButton />
          </Group>
        </Group>

        {modelsError && (
          <ErrorNotice
            error={modelsError}
            onRetry={retryModels}
            onSignIn={onSignIn}
            onEnableSharing={onEnableSharing}
          />
        )}
        {models && models.length === 0 && (
          <Text size="sm" c="dimmed">
            No models are available for this ChatGPT account.
          </Text>
        )}

        <Textarea
          label="Prompt"
          placeholder="Reply with: Token sharing works."
          autosize
          minRows={3}
          maxRows={10}
          value={prompt}
          onChange={(event) => setPrompt(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) void send();
          }}
          disabled={streaming}
        />

        <Group gap="xs">
          <Button onClick={() => void send()} disabled={!model || !prompt.trim() || streaming}>
            Send
          </Button>
          <Button variant="default" onClick={() => void cancelPrompt()} disabled={!streaming}>
            Cancel
          </Button>
          {streaming && <Loader size="sm" type="dots" />}
        </Group>

        {error && (
          <ErrorNotice
            error={error}
            onRetry={() => void send()}
            onSignIn={onSignIn}
            onEnableSharing={onEnableSharing}
          />
        )}
        {cancelled && (
          <Text size="sm" c="dimmed">
            Response cancelled.
          </Text>
        )}
        {(response || streaming) && (
          <Paper withBorder radius="md" p="md" bg="var(--mantine-color-default-hover)">
            <Text size="sm" style={{ whiteSpace: 'pre-wrap' }} aria-live="polite">
              {response || '…'}
            </Text>
          </Paper>
        )}
      </Stack>
    </Card>
  );
}

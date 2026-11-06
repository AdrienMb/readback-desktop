import { Alert, Button, Group, Stack, Text } from '@mantine/core';
import type { AppError, ErrorKind } from '../api';
import { ManageUsageButton } from './ManageUsageButton';

interface Props {
  error: AppError;
  /** Sign in again (reauth). */
  onSignIn?: () => void;
  /** Sign in again asking for plan usage consent. */
  onEnableSharing?: () => void;
  onRetry?: () => void;
}

const TITLES: Record<ErrorKind, string> = {
  usage_limit: 'Usage limit reached',
  sharing_disabled: 'ChatGPT plan usage isn’t enabled',
  not_eligible: 'ChatGPT plan usage isn’t available',
  reauth: 'Sign in again',
  network: 'Can’t reach ChatGPT',
  temporary: 'ChatGPT is temporarily unavailable',
  cancelled: 'Cancelled',
  sign_in: 'Sign-in didn’t complete',
  request: 'ChatGPT rejected the request',
  storage: 'Readback couldn’t access its saved sign-in',
  other: 'ChatGPT couldn’t complete the request',
};

export function ErrorNotice({ error, onSignIn, onEnableSharing, onRetry }: Props) {
  const color = error.kind === 'usage_limit' || error.kind === 'temporary' ? 'yellow' : 'red';
  const body =
    error.kind === 'usage_limit'
      ? 'Review your plan or this app’s limit in ChatGPT settings.'
      : error.message;

  let actions = null;
  if (error.kind === 'usage_limit') {
    actions = <ManageUsageButton variant="filled" />;
  } else if (error.kind === 'sharing_disabled' && onEnableSharing) {
    actions = (
      <Button size="xs" onClick={onEnableSharing}>
        Enable plan usage
      </Button>
    );
  } else if (error.kind === 'reauth' && onSignIn) {
    actions = (
      <Button size="xs" onClick={onSignIn}>
        Sign in again
      </Button>
    );
  } else if ((error.kind === 'network' || error.retryable) && onRetry) {
    actions = (
      <Button size="xs" variant="default" onClick={onRetry}>
        Try again
      </Button>
    );
  }

  return (
    <Alert color={color} title={TITLES[error.kind]} variant="light">
      <Stack gap="xs">
        <Text size="sm">{body}</Text>
        {actions && <Group gap="xs">{actions}</Group>}
        {error.requestId && (
          <Text size="xs" c="dimmed">
            Request ID: {error.requestId}
          </Text>
        )}
      </Stack>
    </Alert>
  );
}

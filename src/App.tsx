import {
  Alert,
  Badge,
  Button,
  Card,
  Center,
  Container,
  Group,
  Loader,
  Stack,
  Text,
  Title,
} from '@mantine/core';
import { useCallback, useEffect, useState } from 'react';
import {
  type AppError,
  type Session,
  cancelSignIn,
  dismissWelcome,
  getSession,
  signIn,
  signOut,
  toAppError,
} from './api';
import { ContinueWithChatGPT } from './components/ContinueWithChatGPT';
import { ErrorNotice } from './components/ErrorNotice';
import { ManageUsageButton } from './components/ManageUsageButton';
import { PromptPanel } from './components/PromptPanel';
import { WelcomeModal } from './components/WelcomeModal';

type Busy = 'signing_in' | 'signing_out' | null;

export function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [loadError, setLoadError] = useState<AppError | null>(null);
  const [busy, setBusy] = useState<Busy>(null);
  const [accountError, setAccountError] = useState<AppError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const refreshSession = useCallback(() => {
    getSession()
      .then((next) => {
        setLoadError(null);
        setSession(next);
      })
      .catch((raw: unknown) => setLoadError(toAppError(raw)));
  }, []);

  useEffect(refreshSession, [refreshSession]);

  const startSignIn = useCallback(
    async (reconsent = false) => {
      setBusy('signing_in');
      setAccountError(null);
      setNotice(null);
      try {
        setSession(await signIn(reconsent));
      } catch (raw) {
        const error = toAppError(raw);
        if (error.kind !== 'cancelled') setAccountError(error);
        refreshSession();
      } finally {
        setBusy(null);
      }
    },
    [refreshSession],
  );

  async function handleSignOut() {
    setBusy('signing_out');
    setAccountError(null);
    try {
      const result = await signOut();
      setSession(result.session);
      setNotice(
        result.revocationConfirmed
          ? null
          : 'You’re signed out on this Mac, but ChatGPT didn’t confirm the disconnection. ' +
              'You can disconnect Readback in ChatGPT Settings.',
      );
    } catch (raw) {
      setAccountError(toAppError(raw));
    } finally {
      setBusy(null);
    }
  }

  async function handleDismissWelcome() {
    try {
      setSession(await dismissWelcome());
    } catch {
      setSession((current) => (current ? { ...current, showWelcome: false } : current));
    }
  }

  if (loadError) {
    return (
      <Container size="sm" py="xl">
        <ErrorNotice error={loadError} onRetry={refreshSession} />
      </Container>
    );
  }
  if (!session) {
    return (
      <Center h="100%" py="xl">
        <Loader />
      </Center>
    );
  }

  const connected = session.status === 'connected';

  return (
    <Container size="sm" py="xl">
      <Stack gap="lg">
        <Title order={1}>Readback</Title>

        {notice && (
          <Alert color="yellow" variant="light" withCloseButton onClose={() => setNotice(null)}>
            {notice}
          </Alert>
        )}

        {!connected && (
          <Card withBorder radius="md" padding="lg">
            <Stack gap="md" align="flex-start">
              {session.status === 'reauth_required' && (
                <Alert color="yellow" variant="light" title="Sign in again" w="100%">
                  Your ChatGPT session has expired. Sign in again to keep using your plan.
                </Alert>
              )}
              <Text>
                Readback uses your ChatGPT plan. No API key, and your credentials stay on this Mac.
              </Text>
              <Group gap="sm">
                <ContinueWithChatGPT
                  onClick={() => void startSignIn()}
                  loading={busy === 'signing_in'}
                />
                {busy === 'signing_in' && (
                  <Button variant="subtle" onClick={() => void cancelSignIn()}>
                    Cancel
                  </Button>
                )}
              </Group>
              {busy === 'signing_in' && (
                <Text size="sm" c="dimmed">
                  Finish signing in in your browser, then come back here.
                </Text>
              )}
            </Stack>
          </Card>
        )}

        {connected && (
          <Card withBorder radius="md" padding="lg">
            <Stack gap="sm">
              <Group justify="space-between" wrap="nowrap">
                <Stack gap={2}>
                  <Text fw={600}>{session.email ?? 'ChatGPT account'}</Text>
                  <Group gap={6}>
                    <Badge color="green" variant="light" tt="none">
                      Connected
                    </Badge>
                  </Group>
                </Stack>
                <Button
                  variant="default"
                  onClick={() => void handleSignOut()}
                  loading={busy === 'signing_out'}
                  disabled={busy === 'signing_in'}
                >
                  Sign out
                </Button>
              </Group>
              {session.sharing ? (
                <Group justify="space-between">
                  <Text size="sm" c="dimmed">
                    ChatGPT plan usage is enabled.
                  </Text>
                  <ManageUsageButton />
                </Group>
              ) : (
                <Alert color="yellow" variant="light" title="ChatGPT plan usage wasn’t granted">
                  <Stack gap="xs" align="flex-start">
                    <Text size="sm">
                      You’re signed in, but Readback can’t use your ChatGPT plan until you allow it.
                      Sign in again and allow plan usage.
                    </Text>
                    <Button
                      size="xs"
                      onClick={() => void startSignIn(true)}
                      loading={busy === 'signing_in'}
                    >
                      Enable plan usage
                    </Button>
                  </Stack>
                </Alert>
              )}
            </Stack>
          </Card>
        )}

        {accountError && (
          <ErrorNotice
            error={accountError}
            onSignIn={() => void startSignIn()}
            onEnableSharing={() => void startSignIn(true)}
            onRetry={() => void startSignIn()}
          />
        )}

        {connected && session.sharing && (
          <PromptPanel
            key={session.email ?? 'account'}
            onSessionMaybeChanged={refreshSession}
            onSignIn={() => void startSignIn()}
            onEnableSharing={() => void startSignIn(true)}
          />
        )}
      </Stack>

      <WelcomeModal
        opened={connected && session.showWelcome}
        onDismiss={() => void handleDismissWelcome()}
      />
    </Container>
  );
}

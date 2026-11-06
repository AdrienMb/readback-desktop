import { Button, Group, Modal, Text } from '@mantine/core';
import { ManageUsageButton } from './ManageUsageButton';

interface Props {
  opened: boolean;
  onDismiss: () => void;
}

/** Shown once, the first time someone signs in with ChatGPT plan usage enabled. */
export function WelcomeModal({ opened, onDismiss }: Props) {
  return (
    <Modal opened={opened} onClose={onDismiss} title="You’re using your ChatGPT plan" centered>
      <Text size="sm">
        Readback’s AI requests now use your ChatGPT plan, with no API key. You can review and manage
        your usage anytime in ChatGPT settings.
      </Text>
      <Group justify="flex-end" mt="lg" gap="xs">
        <ManageUsageButton />
        <Button onClick={onDismiss}>Got it</Button>
      </Group>
    </Modal>
  );
}

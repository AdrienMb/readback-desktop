import { Container, Stack, Text, Title } from '@mantine/core';

export function App() {
  return (
    <Container size="sm" py="xl">
      <Stack gap="xs">
        <Title order={1}>Readback</Title>
        <Text c="dimmed">Sign in with ChatGPT spike. Nothing to see yet.</Text>
      </Stack>
    </Container>
  );
}

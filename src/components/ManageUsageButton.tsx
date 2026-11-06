import { Button, type ButtonProps } from '@mantine/core';
import { openUsageSettings } from '../api';

function ExternalLinkIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path
        d="M9 3h4v4M13 3 7.5 8.5M11 9.5V13H3V5h3.5"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/** "Manage usage": opens https://chatgpt.com/settings/usage in the browser (from Rust). */
export function ManageUsageButton(props: ButtonProps) {
  return (
    <Button
      variant="default"
      size="xs"
      radius="xl"
      rightSection={<ExternalLinkIcon />}
      aria-label="Manage ChatGPT usage (opens in browser)"
      onClick={() => void openUsageSettings().catch(() => undefined)}
      {...props}
    >
      Manage usage
    </Button>
  );
}

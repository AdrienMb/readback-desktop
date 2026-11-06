import {
  Badge,
  Box,
  Button,
  Group,
  Menu,
  NavLink,
  ScrollArea,
  Stack,
  Text,
  Tooltip,
  Tree,
  useTree,
} from '@mantine/core';
import { useMemo } from 'react';

type TreeExpandedState = Record<string, boolean>;
import type { Entry, EntryFlag, Vault } from './api';
import { ChatIcon, ChevronIcon, FolderIcon, NoteIcon } from './icons';
import { type Entries, buildTree } from './tree';

const FLAG_LABELS: Record<EntryFlag, string> = {
  too_large: 'Too large',
  unsupported_encoding: 'Not UTF-8',
  unreadable: 'No access',
  offloaded: 'In iCloud',
};

export interface EntryActions {
  onOpen: (entry: Entry) => void;
  onNewNote: (parentRel: string) => void;
  onNewFolder: (parentRel: string) => void;
  onRename: (entry: Entry) => void;
  onMove: (entry: Entry) => void;
  onTrash: (entry: Entry) => void;
  onReveal: (relPath: string) => void;
  onOpenInDefaultApp: (entry: Entry) => void;
}

interface Props extends EntryActions {
  vault: Vault | null;
  rootPath: string | null;
  entries: Entries;
  openPath: string | null;
  expanded: TreeExpandedState;
  onExpandedChange: (state: TreeExpandedState) => void;
  view: 'notes' | 'chatgpt';
  onShowChatGpt: () => void;
  onChangeVault: () => void;
  changingVault: boolean;
}

export function Sidebar(props: Props) {
  const { vault, rootPath, entries, openPath, expanded, onExpandedChange, view } = props;
  const data = useMemo(() => buildTree(entries), [entries]);
  const tree = useTree({
    expandedState: expanded,
    onExpandedStateChange: onExpandedChange,
    selectedState: openPath ? [openPath] : [],
  });
  const ready = vault?.status === 'ok';

  return (
    <Stack gap={0} h="100%">
      <Box p="xs" pb={0}>
        <NavLink
          label="ChatGPT"
          leftSection={<ChatIcon />}
          active={view === 'chatgpt'}
          onClick={props.onShowChatGpt}
          variant="light"
        />
      </Box>

      <Stack gap={4} p="sm" pb="xs">
        <Group justify="space-between" wrap="nowrap" gap="xs">
          <Text fw={600} size="sm" truncate="end">
            {vault?.name ?? 'Notes'}
          </Text>
          {vault && vault.status !== 'not_set' && (
            <Button
              size="compact-xs"
              variant="subtle"
              onClick={props.onChangeVault}
              loading={props.changingVault}
            >
              Change
            </Button>
          )}
        </Group>
        {(rootPath ?? vault?.path) && (
          <Text size="xs" c="dimmed" truncate="start" title={rootPath ?? vault?.path ?? ''}>
            {rootPath ?? vault?.path}
          </Text>
        )}
        {ready && (
          <Group gap="xs" mt={4}>
            <Button size="compact-xs" variant="default" onClick={() => props.onNewNote('')}>
              New note
            </Button>
            <Button size="compact-xs" variant="default" onClick={() => props.onNewFolder('')}>
              New folder
            </Button>
          </Group>
        )}
      </Stack>

      <ScrollArea style={{ flex: 1 }} px="xs" pb="sm">
        {!ready && (
          <Text size="xs" c="dimmed" px="xs">
            {vault?.status === 'not_set'
              ? 'Choose a notes folder to get started.'
              : vault
                ? 'Your notes folder isn’t available.'
                : ''}
          </Text>
        )}
        {ready && data.length === 0 && (
          <Text size="xs" c="dimmed" px="xs">
            No notes yet. Create one here, or add Markdown files to this folder in Finder.
          </Text>
        )}
        {ready && data.length > 0 && (
          <Tree
            data={data}
            tree={tree}
            levelOffset="md"
            expandOnClick
            renderNode={({ node, expanded: open, elementProps }) => {
              const entry = node.nodeProps?.entry as Entry;
              const selected = entry.relPath === openPath;
              return (
                <EntryMenu entry={entry} actions={props}>
                  <Group
                    gap={6}
                    wrap="nowrap"
                    {...elementProps}
                    onClick={(event) => {
                      elementProps.onClick(event);
                      if (entry.kind === 'note') props.onOpen(entry);
                    }}
                    style={{
                      ...elementProps.style,
                      cursor: 'pointer',
                      borderRadius: 'var(--mantine-radius-sm)',
                      padding: '3px 6px',
                      background: selected ? 'var(--mantine-color-default-hover)' : undefined,
                    }}
                  >
                    {entry.kind === 'folder' ? (
                      <>
                        <ChevronIcon open={open} />
                        <FolderIcon />
                      </>
                    ) : (
                      <>
                        <span style={{ width: 14, flexShrink: 0 }} />
                        <NoteIcon />
                      </>
                    )}
                    <Tooltip label={entry.name} openDelay={600} withinPortal>
                      <Text size="sm" truncate="end" style={{ flex: 1, minWidth: 0 }}>
                        {node.label}
                      </Text>
                    </Tooltip>
                    {entry.flags?.map((flag) => (
                      <Badge key={flag} size="xs" variant="light" color="gray" tt="none">
                        {FLAG_LABELS[flag]}
                      </Badge>
                    ))}
                  </Group>
                </EntryMenu>
              );
            }}
          />
        )}
      </ScrollArea>
    </Stack>
  );
}

function EntryMenu({
  entry,
  actions,
  children,
}: {
  entry: Entry;
  actions: EntryActions;
  children: React.ReactElement;
}) {
  const isFolder = entry.kind === 'folder';
  return (
    <Menu withinPortal position="bottom-start" shadow="md" width={200}>
      <Menu.ContextMenu>{children}</Menu.ContextMenu>
      <Menu.Dropdown>
        {isFolder && (
          <>
            <Menu.Item onClick={() => actions.onNewNote(entry.relPath)}>New note here</Menu.Item>
            <Menu.Item onClick={() => actions.onNewFolder(entry.relPath)}>
              New folder here
            </Menu.Item>
            <Menu.Divider />
          </>
        )}
        <Menu.Item onClick={() => actions.onRename(entry)}>Rename…</Menu.Item>
        <Menu.Item onClick={() => actions.onMove(entry)}>Move…</Menu.Item>
        <Menu.Item onClick={() => actions.onReveal(entry.relPath)}>Reveal in Finder</Menu.Item>
        <Menu.Item onClick={() => actions.onOpenInDefaultApp(entry)}>Open in default app</Menu.Item>
        <Menu.Divider />
        <Menu.Item color="red" onClick={() => actions.onTrash(entry)}>
          Move to Trash
        </Menu.Item>
      </Menu.Dropdown>
    </Menu>
  );
}

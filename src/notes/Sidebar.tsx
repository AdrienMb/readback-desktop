import {
  ActionIcon,
  Badge,
  Group,
  Menu,
  ScrollArea,
  Stack,
  Tooltip,
  Tree,
  UnstyledButton,
  useTree,
} from '@mantine/core';
import { useMemo } from 'react';
import type { Entry, EntryFlag, Vault } from './api';
import { ChevronDownIcon, ComposeIcon, FolderIcon, FolderOpenIcon, FolderPlusIcon } from './icons';
import classes from './Sidebar.module.css';
import { type Entries, buildTree } from './tree';

type TreeExpandedState = Record<string, boolean>;

const FLAG_LABELS: Record<EntryFlag, string> = {
  too_large: 'Too large',
  unsupported_encoding: 'Not UTF-8',
  unreadable: 'No access',
  offloaded: 'In iCloud',
};

/** Indent per tree level, so a note's text lines up under its folder's name. */
const LEVEL_OFFSET = 19;

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
  onChangeVault: () => void;
}

export function Sidebar(props: Props) {
  const { vault, rootPath, entries, openPath, expanded, onExpandedChange } = props;
  const data = useMemo(() => buildTree(entries), [entries]);
  const tree = useTree({
    expandedState: expanded,
    onExpandedStateChange: onExpandedChange,
    selectedState: openPath ? [openPath] : [],
  });
  const ready = vault?.status === 'ok';
  const path = rootPath ?? vault?.path ?? null;

  return (
    <Stack gap={0} h="100%">
      <div className={classes.header}>
        {vault && vault.status !== 'not_set' ? (
          <Menu position="bottom-start" shadow="md" width={240} withinPortal>
            <Menu.Target>
              <UnstyledButton className={classes.vaultButton} title={path ?? undefined}>
                <span className={classes.vaultName}>{vault.name ?? 'Notes'}/</span>
                <span className={classes.vaultChevron}>
                  <ChevronDownIcon size={15} />
                </span>
              </UnstyledButton>
            </Menu.Target>
            <Menu.Dropdown>
              {path && (
                <Menu.Label style={{ overflowWrap: 'anywhere' }} title={path}>
                  {path}
                </Menu.Label>
              )}
              {ready && <Menu.Item onClick={() => props.onReveal('')}>Reveal in Finder</Menu.Item>}
              <Menu.Item onClick={props.onChangeVault}>Change notes folder…</Menu.Item>
            </Menu.Dropdown>
          </Menu>
        ) : (
          <span className={classes.vaultButton}>Notes</span>
        )}

        {ready && (
          <Group gap={2} wrap="nowrap">
            <Tooltip label="New folder" openDelay={400}>
              <ActionIcon
                variant="subtle"
                size={28}
                className={classes.iconButton}
                aria-label="New folder"
                onClick={() => props.onNewFolder('')}
              >
                <FolderPlusIcon size={18} />
              </ActionIcon>
            </Tooltip>
            <Tooltip label="New note" openDelay={400}>
              <ActionIcon
                variant="subtle"
                size={28}
                className={classes.iconButton}
                aria-label="New note"
                onClick={() => props.onNewNote('')}
              >
                <ComposeIcon size={18} />
              </ActionIcon>
            </Tooltip>
          </Group>
        )}
      </div>

      <ScrollArea style={{ flex: 1 }} px={8} pb="sm">
        {!ready && (
          <div className={classes.hint}>
            {vault?.status === 'not_set'
              ? 'Choose a notes folder to get started.'
              : vault
                ? 'Your notes folder isn’t available.'
                : ''}
          </div>
        )}
        {ready && data.length === 0 && (
          <div className={classes.hint}>
            No notes yet. Create one here, or add Markdown files to this folder in Finder.
          </div>
        )}
        {ready && data.length > 0 && (
          <Tree
            data={data}
            tree={tree}
            levelOffset={LEVEL_OFFSET}
            expandOnClick
            classNames={{ subtree: classes.subtree }}
            renderNode={({ node, expanded: open, elementProps }) => {
              const entry = node.nodeProps?.entry as Entry;
              const isFolder = entry.kind === 'folder';
              return (
                <EntryMenu entry={entry} actions={props}>
                  <div
                    {...elementProps}
                    className={`${elementProps.className} ${classes.row}`}
                    data-kind={entry.kind}
                    data-active={entry.relPath === openPath || undefined}
                    onClick={(event) => {
                      elementProps.onClick(event);
                      if (!isFolder) props.onOpen(entry);
                    }}
                  >
                    {isFolder && (
                      <span className={classes.folderIcon}>
                        {open ? <FolderOpenIcon size={17} /> : <FolderIcon size={17} />}
                      </span>
                    )}
                    <Tooltip label={entry.name} openDelay={600} withinPortal>
                      <span className={classes.label}>{node.label}</span>
                    </Tooltip>
                    {entry.flags?.map((flag) => (
                      <Badge key={flag} size="xs" variant="light" color="gray" tt="none">
                        {FLAG_LABELS[flag]}
                      </Badge>
                    ))}
                  </div>
                </EntryMenu>
              );
            }}
          />
        )}
      </ScrollArea>
    </Stack>
  );
}

/** The actions on one note or folder, shared by the tree's context menu and the title menu. */
export function EntryMenuItems({ entry, actions }: { entry: Entry; actions: EntryActions }) {
  return (
    <>
      {entry.kind === 'folder' && (
        <>
          <Menu.Item onClick={() => actions.onNewNote(entry.relPath)}>New note here</Menu.Item>
          <Menu.Item onClick={() => actions.onNewFolder(entry.relPath)}>New folder here</Menu.Item>
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
    </>
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
  return (
    <Menu withinPortal position="bottom-start" shadow="md" width={200}>
      <Menu.ContextMenu>{children}</Menu.ContextMenu>
      <Menu.Dropdown>
        <EntryMenuItems entry={entry} actions={actions} />
      </Menu.Dropdown>
    </Menu>
  );
}

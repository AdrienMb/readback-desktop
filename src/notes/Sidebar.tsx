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
import {
  type Collision,
  type CollisionDetection,
  DndContext,
  type DragEndEvent,
  type DragOverEvent,
  DragOverlay,
  type DragStartEvent,
  MeasuringStrategy,
  PointerSensor,
  useDraggable,
  useDroppable,
  useSensor,
  useSensors,
} from '@dnd-kit/core';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import type { Entry, EntryFlag, Vault } from './api';
import { ChevronDownIcon, ComposeIcon, FolderIcon, FolderOpenIcon, FolderPlusIcon } from './icons';
import classes from './Sidebar.module.css';
import { type Entries, buildTree, canMoveTo, label, parentOf } from './tree';

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
  onExpandedChange: React.Dispatch<React.SetStateAction<TreeExpandedState>>;
  onChangeVault: () => void;
  /** A note or folder was dropped on a folder ('' is the top level). Only valid moves. */
  onMoveTo: (entry: Entry, parentRel: string) => void;
}

/** What a row (or the empty space around the tree) moves a dropped entry into. */
interface DropData {
  folder: string;
  /** The row is that folder itself, so hovering it can expand it. */
  isFolderRow: boolean;
}

const ROOT_DROP_ID = 'root';

const entryOf = (active: DragStartEvent['active']) =>
  (active.data.current as { entry: Entry }).entry;
/** How long a collapsed folder has to be hovered during a drag before it opens. */
const EXPAND_DELAY_MS = 600;

/**
 * Hit-tests rows by their vertical band only, so the indent and the 1px gaps between rows still
 * count as the row. Outside every row but inside the sidebar is the top level.
 */
const collideRows: CollisionDetection = ({
  pointerCoordinates,
  droppableContainers,
  droppableRects,
}) => {
  if (!pointerCoordinates) return [];
  const { x, y } = pointerCoordinates;
  const root = droppableRects.get(ROOT_DROP_ID);
  if (!root || x < root.left || x > root.right || y < root.top || y > root.bottom) return [];
  let hit: Collision = { id: ROOT_DROP_ID };
  for (const container of droppableContainers) {
    if (container.id === ROOT_DROP_ID) continue;
    const rect = droppableRects.get(container.id);
    if (rect && y >= rect.top - 1 && y < rect.bottom + 1) {
      hit = { id: container.id };
      break;
    }
  }
  return [hit];
};

export function Sidebar(props: Props) {
  const { vault, rootPath, entries, openPath, expanded, onExpandedChange, onMoveTo } = props;
  const data = useMemo(() => buildTree(entries), [entries]);
  const tree = useTree({
    expandedState: expanded,
    onExpandedStateChange: onExpandedChange,
    selectedState: openPath ? [openPath] : [],
  });
  const ready = vault?.status === 'ok';
  const path = rootPath ?? vault?.path ?? null;

  // Drag and drop: a press that moves 5px starts a drag, so clicks and the context menu still work.
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 5 } }));
  const [dragging, setDragging] = useState<Entry | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const expandTimer = useRef<{ folder: string; timer: number } | null>(null);

  const cancelExpand = useCallback(() => {
    if (expandTimer.current) window.clearTimeout(expandTimer.current.timer);
    expandTimer.current = null;
  }, []);
  useEffect(() => cancelExpand, [cancelExpand]);

  function targetOf(over: DragOverEvent['over'], entry: Entry): string | null {
    const drop = over?.data.current as DropData | undefined;
    if (!drop) return null;
    return canMoveTo(entry, drop.folder) ? drop.folder : null;
  }

  function onDragStart({ active }: DragStartEvent) {
    setDragging(entryOf(active));
  }

  function onDragOver({ active, over }: DragOverEvent) {
    const target = targetOf(over, entryOf(active));
    setDropTarget(target);
    const drop = over?.data.current as DropData | undefined;
    const folder = target !== null && drop?.isFolderRow && !expanded[target] ? target : null;
    if (expandTimer.current?.folder === folder) return;
    cancelExpand();
    if (folder === null) return;
    const timer = window.setTimeout(() => {
      expandTimer.current = null;
      onExpandedChange((state) => ({ ...state, [folder]: true }));
    }, EXPAND_DELAY_MS);
    expandTimer.current = { folder, timer };
  }

  function endDrag() {
    cancelExpand();
    setDragging(null);
    setDropTarget(null);
  }

  function onDragEnd({ active, over }: DragEndEvent) {
    endDrag();
    const entry = entryOf(active);
    const target = targetOf(over, entry);
    if (target !== null) onMoveTo(entry, target);
  }

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

      <DndContext
        sensors={sensors}
        collisionDetection={collideRows}
        // Rows shift when a folder opens mid-drag, so keep measuring.
        measuring={{ droppable: { strategy: MeasuringStrategy.Always } }}
        onDragStart={onDragStart}
        onDragOver={onDragOver}
        onDragEnd={onDragEnd}
        onDragCancel={endDrag}
      >
        <RootDropZone active={dragging !== null && dropTarget === ''}>
          <ScrollArea h="100%" px={4} pb="sm">
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
                  return (
                    <EntryMenu entry={entry} actions={props}>
                      <TreeRow
                        {...elementProps}
                        entry={entry}
                        open={open}
                        label={node.label}
                        active={entry.relPath === openPath}
                        dropTarget={dragging !== null && entry.relPath === dropTarget}
                        dragActive={dragging !== null}
                        onClick={(event) => {
                          elementProps.onClick(event);
                          if (entry.kind === 'note') props.onOpen(entry);
                        }}
                      />
                    </EntryMenu>
                  );
                }}
              />
            )}
          </ScrollArea>
        </RootDropZone>
        {createPortal(
          <DragOverlay dropAnimation={null}>
            {dragging && (
              <div className={classes.dragPreview}>
                {dragging.kind === 'folder' && (
                  <span className={classes.folderIcon}>
                    <FolderIcon size={17} />
                  </span>
                )}
                <span className={classes.label}>{label(dragging)}</span>
              </div>
            )}
          </DragOverlay>,
          document.body,
        )}
      </DndContext>
    </Stack>
  );
}

/** The sidebar's scroll area: dropping here (outside any row) moves to the top level. */
function RootDropZone({ active, children }: { active: boolean; children: React.ReactNode }) {
  const { setNodeRef } = useDroppable({
    id: ROOT_DROP_ID,
    data: { folder: '', isFolderRow: false } satisfies DropData,
  });
  return (
    <div ref={setNodeRef} className={classes.dropRoot} data-drop-target={active || undefined}>
      {children}
    </div>
  );
}

interface TreeRowProps extends React.ComponentPropsWithoutRef<'div'> {
  entry: Entry;
  open: boolean;
  label: React.ReactNode;
  active: boolean;
  dropTarget: boolean;
  dragActive: boolean;
}

/**
 * One row: draggable, and a drop target for its folder (a note's row stands for the folder it's
 * in). Other props (Tree's element props, the context menu's handlers) go on the element.
 */
function TreeRow({
  entry,
  open,
  label: text,
  active,
  dropTarget,
  dragActive,
  className,
  ...rest
}: TreeRowProps) {
  const isFolder = entry.kind === 'folder';
  const drag = useDraggable({ id: entry.relPath, data: { entry } });
  const drop = useDroppable({
    id: `row:${entry.relPath}`,
    data: {
      folder: isFolder ? entry.relPath : parentOf(entry.relPath),
      isFolderRow: isFolder,
    } satisfies DropData,
  });
  const setDragRef = drag.setNodeRef;
  const setDropRef = drop.setNodeRef;
  const ref = useCallback(
    (node: HTMLDivElement | null) => {
      setDragRef(node);
      setDropRef(node);
    },
    [setDragRef, setDropRef],
  );

  return (
    <div
      {...rest}
      {...drag.listeners}
      ref={ref}
      className={`${className ?? ''} ${classes.row}`}
      data-kind={entry.kind}
      data-active={active || undefined}
      data-dragging={drag.isDragging || undefined}
      data-drop-target={dropTarget || undefined}
    >
      {isFolder && (
        <span className={classes.folderIcon}>
          {open ? <FolderOpenIcon size={17} /> : <FolderIcon size={17} />}
        </span>
      )}
      <Tooltip label={entry.name} openDelay={600} withinPortal disabled={dragActive}>
        <span className={classes.label}>{text}</span>
      </Tooltip>
      {entry.flags?.map((flag) => (
        <Badge key={flag} size="xs" variant="light" color="gray" tt="none">
          {FLAG_LABELS[flag]}
        </Badge>
      ))}
    </div>
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

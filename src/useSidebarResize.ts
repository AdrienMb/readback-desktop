import {
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent,
  type RefObject,
  useEffect,
  useRef,
  useState,
} from 'react';

export const SIDEBAR_DEFAULT_WIDTH = 270;
const MIN_WIDTH = 180;
const MAX_WIDTH = 560;
/** Releasing a drag below this width hides the sidebar (its width is kept for next time). */
const CLOSE_BELOW = 140;
/** The note area keeps at least this much room. */
const MIN_CONTENT_WIDTH = 360;
/** The card's right margin, outside the sidebar and the note area. */
const CARD_MARGIN = 6;
const KEY_STEP = 16;
const STORAGE_KEY = 'readback.sidebarWidth';

/** The CSS variable on the window element that the sidebar and the title bar read. */
export const SIDEBAR_WIDTH_VAR = '--rb-sidebar-w';

function readStoredWidth(): number {
  try {
    const value = Number(localStorage.getItem(STORAGE_KEY));
    if (Number.isFinite(value) && value >= MIN_WIDTH && value <= MAX_WIDTH) return value;
  } catch {
    // Storage unavailable: use the default.
  }
  return SIDEBAR_DEFAULT_WIDTH;
}

function storeWidth(width: number) {
  try {
    localStorage.setItem(STORAGE_KEY, String(width));
  } catch {
    // Not remembered; nothing else depends on it.
  }
}

interface Drag {
  pointerId: number;
  startX: number;
  startWidth: number;
  raw: number;
}

/**
 * A resizable notes sidebar. Returns the window style (the width variable) and the props for the
 * drag handle on the sidebar's right edge. While dragging, the variable is written straight onto
 * the window element so React doesn't re-render the tree and editor on every pointer move; the
 * width is committed (and remembered) on release.
 */
export function useSidebarResize(
  windowRef: RefObject<HTMLElement | null>,
  railWidth: number,
  onCollapse: () => void,
) {
  const [preferred, setPreferred] = useState(readStoredWidth);
  const [viewport, setViewport] = useState(() => window.innerWidth);
  const drag = useRef<Drag | null>(null);

  useEffect(() => {
    const onResize = () => setViewport(window.innerWidth);
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);

  const maxWidth = Math.max(
    MIN_WIDTH,
    Math.min(MAX_WIDTH, viewport - railWidth - CARD_MARGIN - MIN_CONTENT_WIDTH),
  );
  const clamp = (value: number) => Math.round(Math.min(maxWidth, Math.max(MIN_WIDTH, value)));
  // The preferred width is kept as is; a narrow window only limits what is shown.
  const width = clamp(preferred);

  function setLive(value: number) {
    windowRef.current?.style.setProperty(SIDEBAR_WIDTH_VAR, `${value}px`);
  }

  function commit(value: number) {
    const next = clamp(value);
    setLive(next);
    setPreferred(next);
    storeWidth(next);
  }

  function endDrag(apply: boolean) {
    const current = drag.current;
    if (!current) return;
    drag.current = null;
    const el = windowRef.current;
    if (el) {
      delete el.dataset.resizing;
      delete el.dataset.collapsing;
    }
    if (apply && current.raw < CLOSE_BELOW) {
      // React's value hasn't changed, so put the live variable back by hand.
      setLive(current.startWidth);
      onCollapse();
    } else if (apply) {
      commit(current.raw);
    } else {
      setLive(current.startWidth);
    }
  }

  function onPointerDown(event: PointerEvent<HTMLDivElement>) {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    event.currentTarget.setPointerCapture(event.pointerId);
    drag.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startWidth: width,
      raw: width,
    };
    if (windowRef.current) windowRef.current.dataset.resizing = '';
  }

  function onPointerMove(event: PointerEvent<HTMLDivElement>) {
    const current = drag.current;
    if (!current || current.pointerId !== event.pointerId) return;
    current.raw = current.startWidth + event.clientX - current.startX;
    setLive(clamp(current.raw));
    const el = windowRef.current;
    if (el) {
      if (current.raw < CLOSE_BELOW) el.dataset.collapsing = '';
      else delete el.dataset.collapsing;
    }
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    let next: number | null = null;
    if (event.key === 'ArrowLeft') next = width - KEY_STEP;
    else if (event.key === 'ArrowRight') next = width + KEY_STEP;
    else if (event.key === 'Home') next = MIN_WIDTH;
    else if (event.key === 'End') next = maxWidth;
    if (next === null) return;
    event.preventDefault();
    commit(next);
  }

  const windowStyle = { [SIDEBAR_WIDTH_VAR]: `${width}px` } as CSSProperties;

  const handleProps = {
    role: 'separator',
    tabIndex: 0,
    'aria-orientation': 'vertical',
    'aria-label': 'Resize sidebar',
    'aria-valuenow': width,
    'aria-valuemin': MIN_WIDTH,
    'aria-valuemax': maxWidth,
    onPointerDown,
    onPointerMove,
    onPointerUp: () => endDrag(true),
    onPointerCancel: () => endDrag(false),
    onDoubleClick: () => commit(SIDEBAR_DEFAULT_WIDTH),
    onKeyDown,
  } as const;

  return { windowStyle, handleProps };
}

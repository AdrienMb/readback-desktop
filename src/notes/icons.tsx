// Small inline icons (no icon library dependency). They inherit the text colour.
interface IconProps {
  size?: number;
}

function Svg({ size = 16, children }: IconProps & { children: React.ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      style={{ flexShrink: 0 }}
    >
      {children}
    </svg>
  );
}

export function FolderIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
    </Svg>
  );
}

export function NoteIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
      <path d="M14 3v5h5M9 13h6M9 17h4" />
    </Svg>
  );
}

export function ChevronIcon({ size = 14, open }: IconProps & { open: boolean }) {
  return (
    <span
      style={{
        display: 'inline-flex',
        transform: open ? 'rotate(90deg)' : undefined,
        transition: 'transform 120ms ease',
      }}
    >
      <Svg size={size}>
        <path d="M9 6l6 6-6 6" />
      </Svg>
    </span>
  );
}

export function DotsIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <circle cx="5" cy="12" r="1" />
      <circle cx="12" cy="12" r="1" />
      <circle cx="19" cy="12" r="1" />
    </Svg>
  );
}

export function ChatIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M4 5h16v11H8l-4 4z" />
    </Svg>
  );
}

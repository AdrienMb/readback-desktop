import { Text, Typography } from '@mantine/core';
import Markdown, { type Components } from 'react-markdown';
import remarkFrontmatter from 'remark-frontmatter';
import remarkGfm from 'remark-gfm';
import classes from './MarkdownView.module.css';

/**
 * Links and images stay inside the note: the webview must not navigate away, and it has no
 * permission to open URLs. Raw HTML in notes is not rendered (react-markdown escapes it).
 */
const components: Components = {
  a: ({ href, children }) => (
    <a
      href={href}
      title={href}
      className={classes.link}
      onClick={(event) => event.preventDefault()}
      onAuxClick={(event) => event.preventDefault()}
    >
      {children}
    </a>
  ),
  img: ({ alt, src }) => (
    <span className={classes.image} title={typeof src === 'string' ? src : undefined}>
      Image{alt ? `: ${alt}` : ''}
    </span>
  ),
};

interface Props {
  content: string;
  ariaLabel: string;
  /** Faded, for a read-only comparison copy. */
  muted?: boolean;
}

/** Renders a note's Markdown (CommonMark + GFM). YAML/TOML frontmatter is hidden. */
export function MarkdownView({ content, ariaLabel, muted }: Props) {
  if (!content.trim()) {
    return (
      <Text size="sm" c="dimmed" fs="italic">
        This note is empty. Switch to Edit to write in it.
      </Text>
    );
  }
  return (
    <Typography
      className={classes.root}
      data-muted={muted || undefined}
      aria-label={ariaLabel}
      role="document"
    >
      <Markdown
        remarkPlugins={[remarkGfm, [remarkFrontmatter, ['yaml', 'toml']]]}
        components={components}
      >
        {content}
      </Markdown>
    </Typography>
  );
}

import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { markdown, markdownKeymap, markdownLanguage } from '@codemirror/lang-markdown';
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { Annotation, EditorState, type Extension } from '@codemirror/state';
import { EditorView, keymap, placeholder as placeholderText } from '@codemirror/view';
import { tags } from '@lezer/highlight';
import { useEffect, useImperativeHandle, useRef } from 'react';

export interface MarkdownEditorHandle {
  focus: () => void;
}

interface Props {
  /** The text to show. A new value that differs from the editor's text replaces it in place
   * (the cursor and scroll position follow the change). */
  value: string;
  onChange?: (value: string) => void;
  onBlur?: () => void;
  readOnly?: boolean;
  ariaLabel: string;
  placeholder?: string;
  ref?: React.Ref<MarkdownEditorHandle>;
}

/** Marks transactions that come from the `value` prop rather than from typing. */
const external = Annotation.define<boolean>();

const theme = EditorView.theme({
  '&': {
    color: 'var(--rb-text-body)',
    backgroundColor: 'transparent',
    fontSize: '14.5px',
  },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': {
    fontFamily: 'var(--mantine-font-family)',
    lineHeight: '1.7',
    letterSpacing: '0.005em',
    overflow: 'visible',
  },
  '.cm-content': { padding: '0', minHeight: '280px', caretColor: 'var(--rb-text)' },
  '.cm-line': { padding: '0' },
  '.cm-placeholder': { color: 'var(--rb-text-faint)' },
  '&.rb-readonly .cm-content': { color: 'var(--rb-text-faint)' },
});

const highlight = HighlightStyle.define([
  { tag: tags.heading1, fontSize: '1.6em', fontWeight: '650', color: 'var(--rb-text)' },
  { tag: tags.heading2, fontSize: '1.35em', fontWeight: '650', color: 'var(--rb-text)' },
  { tag: tags.heading3, fontSize: '1.15em', fontWeight: '650', color: 'var(--rb-text)' },
  {
    tag: [tags.heading4, tags.heading5, tags.heading6],
    fontWeight: '650',
    color: 'var(--rb-text)',
  },
  { tag: tags.strong, fontWeight: '650' },
  { tag: tags.emphasis, fontStyle: 'italic' },
  { tag: tags.strikethrough, textDecoration: 'line-through' },
  { tag: tags.link, color: 'var(--mantine-color-anchor)' },
  { tag: tags.url, color: 'var(--rb-text-faint)' },
  {
    tag: tags.monospace,
    fontFamily: 'var(--mantine-font-family-monospace)',
    fontSize: '0.9em',
  },
  { tag: tags.quote, color: 'var(--rb-text-muted)', fontStyle: 'italic' },
  // Markup characters: #, *, _, `, >, list markers, link brackets.
  { tag: [tags.processingInstruction, tags.contentSeparator], color: 'var(--rb-text-faint)' },
  { tag: tags.meta, color: 'var(--rb-text-faint)' },
]);

/** CodeMirror 6 editor for one Markdown note. The text lives in CodeMirror; changes go out
 * through `onChange`. */
export function MarkdownEditor({
  value,
  onChange,
  onBlur,
  readOnly = false,
  ariaLabel,
  placeholder,
  ref,
}: Props) {
  const parentRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  const onBlurRef = useRef(onBlur);
  useEffect(() => {
    onChangeRef.current = onChange;
    onBlurRef.current = onBlur;
  }, [onChange, onBlur]);

  // Created once. `value` changes are applied by the effect below.
  const initialValue = useRef(value);
  const options = useRef({ readOnly, ariaLabel, placeholder });

  useEffect(() => {
    const parent = parentRef.current;
    if (!parent) return;
    const { readOnly, ariaLabel, placeholder } = options.current;
    const extensions: Extension[] = [
      markdown({ base: markdownLanguage }),
      syntaxHighlighting(highlight),
      theme,
      EditorView.lineWrapping,
      EditorView.contentAttributes.of({
        'aria-label': ariaLabel,
        spellcheck: readOnly ? 'false' : 'true',
        autocorrect: 'on',
        autocapitalize: 'sentences',
      }),
    ];
    if (readOnly) {
      extensions.push(
        EditorState.readOnly.of(true),
        EditorView.editable.of(false),
        EditorView.editorAttributes.of({ class: 'rb-readonly' }),
      );
    } else {
      extensions.push(
        history(),
        keymap.of([...markdownKeymap, ...defaultKeymap, ...historyKeymap, indentWithTab]),
        EditorView.updateListener.of((update) => {
          if (!update.docChanged) return;
          if (update.transactions.every((tr) => tr.annotation(external))) return;
          onChangeRef.current?.(update.state.doc.toString());
        }),
        EditorView.domEventHandlers({
          blur: () => {
            onBlurRef.current?.();
          },
        }),
      );
      if (placeholder) extensions.push(placeholderText(placeholder));
    }
    const view = new EditorView({
      parent,
      state: EditorState.create({ doc: initialValue.current, extensions }),
    });
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
  }, []);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const current = view.state.doc.toString();
    if (current === value) return;
    // Replace only the part that differs, so the selection maps through the change.
    let start = 0;
    const max = Math.min(current.length, value.length);
    while (start < max && current.charCodeAt(start) === value.charCodeAt(start)) start++;
    let end = 0;
    while (
      end < max - start &&
      current.charCodeAt(current.length - 1 - end) === value.charCodeAt(value.length - 1 - end)
    ) {
      end++;
    }
    view.dispatch({
      changes: {
        from: start,
        to: current.length - end,
        insert: value.slice(start, value.length - end),
      },
      annotations: external.of(true),
    });
  }, [value]);

  useImperativeHandle(
    ref,
    () => ({
      focus: () => {
        const view = viewRef.current;
        if (!view) return;
        view.requestMeasure();
        view.focus();
      },
    }),
    [],
  );

  return <div ref={parentRef} />;
}

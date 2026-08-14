import { describe, expect, it } from 'vitest';
import {
  backspace,
  emptyEditor,
  insertText,
  moveDown,
  moveEnd,
  moveHome,
  moveLeft,
  moveUp,
  splitAtCursor,
  type EditorState,
} from './editing.js';

function editor(text: string, cursor = text.length): EditorState {
  return { text, cursor };
}

describe('composer editing', () => {
  it('inserts text at the cursor', () => {
    let e = insertText(emptyEditor, 'hello');
    e = { ...e, cursor: 0 };
    e = insertText(e, '> ');
    expect(e.text).toBe('> hello');
    expect(e.cursor).toBe(2);
  });

  it('normalizes pasted CRLF to newlines (paste support)', () => {
    const e = insertText(emptyEditor, 'line one\r\nline two\rline three');
    expect(e.text).toBe('line one\nline two\nline three');
  });

  it('backspace joins lines', () => {
    const e = backspace(editor('ab\ncd', 3));
    expect(e.text).toBe('abcd');
    expect(e.cursor).toBe(2);
  });

  it('vertical movement preserves column where possible', () => {
    const e = editor('long line here\nab\nanother', 5);
    const down = moveDown(e);
    // Column 5 clamps to the end of "ab".
    expect(down.cursor).toBe('long line here\n'.length + 2);
    const backUp = moveUp(down);
    expect(backUp.cursor).toBe(2);
  });

  it('home and end are line-scoped', () => {
    const e = editor('one\ntwo three', 8);
    expect(moveHome(e).cursor).toBe(4);
    expect(moveEnd(e).cursor).toBe('one\ntwo three'.length);
  });

  it('left at position zero stays put', () => {
    expect(moveLeft(editor('x', 0)).cursor).toBe(0);
  });

  it('splitAtCursor renders a space cursor at the end and on newlines', () => {
    expect(splitAtCursor(editor('ab', 2))).toEqual({ before: 'ab', at: ' ', after: '' });
    expect(splitAtCursor(editor('a\nb', 1))).toEqual({ before: 'a', at: ' ', after: '\nb' });
    expect(splitAtCursor(editor('abc', 1))).toEqual({ before: 'a', at: 'b', after: 'c' });
  });
});

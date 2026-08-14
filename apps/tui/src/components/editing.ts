/**
 * Pure text-editing operations for the multiline composer, isolated from
 * React so they can be tested directly.
 */

export interface EditorState {
  text: string;
  cursor: number;
}

export const emptyEditor: EditorState = { text: '', cursor: 0 };

export function insertText(state: EditorState, input: string): EditorState {
  const clean = input.replace(/\r\n?/g, '\n');
  return {
    text: state.text.slice(0, state.cursor) + clean + state.text.slice(state.cursor),
    cursor: state.cursor + clean.length,
  };
}

export function backspace(state: EditorState): EditorState {
  if (state.cursor === 0) return state;
  return {
    text: state.text.slice(0, state.cursor - 1) + state.text.slice(state.cursor),
    cursor: state.cursor - 1,
  };
}

export function deleteForward(state: EditorState): EditorState {
  if (state.cursor >= state.text.length) return state;
  return {
    text: state.text.slice(0, state.cursor) + state.text.slice(state.cursor + 1),
    cursor: state.cursor,
  };
}

export function moveLeft(state: EditorState): EditorState {
  return { ...state, cursor: Math.max(0, state.cursor - 1) };
}

export function moveRight(state: EditorState): EditorState {
  return { ...state, cursor: Math.min(state.text.length, state.cursor + 1) };
}

export function moveHome(state: EditorState): EditorState {
  const lineStart = state.text.lastIndexOf('\n', state.cursor - 1) + 1;
  return { ...state, cursor: lineStart };
}

export function moveEnd(state: EditorState): EditorState {
  const lineEnd = state.text.indexOf('\n', state.cursor);
  return { ...state, cursor: lineEnd === -1 ? state.text.length : lineEnd };
}

interface Position {
  line: number;
  column: number;
}

function positionOf(state: EditorState): Position {
  const before = state.text.slice(0, state.cursor);
  const line = (before.match(/\n/g) ?? []).length;
  const column = state.cursor - (before.lastIndexOf('\n') + 1);
  return { line, column };
}

function cursorAt(text: string, line: number, column: number): number {
  const lines = text.split('\n');
  const target = Math.max(0, Math.min(line, lines.length - 1));
  let offset = 0;
  for (let i = 0; i < target; i++) offset += lines[i]!.length + 1;
  return offset + Math.min(column, lines[target]!.length);
}

export function moveUp(state: EditorState): EditorState {
  const { line, column } = positionOf(state);
  if (line === 0) return { ...state, cursor: 0 };
  return { ...state, cursor: cursorAt(state.text, line - 1, column) };
}

export function moveDown(state: EditorState): EditorState {
  const { line, column } = positionOf(state);
  const lastLine = (state.text.match(/\n/g) ?? []).length;
  if (line === lastLine) return { ...state, cursor: state.text.length };
  return { ...state, cursor: cursorAt(state.text, line + 1, column) };
}

/** Split for rendering: text before the cursor, the character under the
 * cursor (space when at end / on newline), and the rest. */
export function splitAtCursor(state: EditorState): {
  before: string;
  at: string;
  after: string;
} {
  const { text, cursor } = state;
  const char = text[cursor];
  if (char === undefined || char === '\n') {
    return { before: text.slice(0, cursor), at: ' ', after: text.slice(cursor) };
  }
  return { before: text.slice(0, cursor), at: char, after: text.slice(cursor + 1) };
}

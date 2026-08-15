/**
 * Mouse selection over the rendered line buffer: highlight styling and
 * text extraction. Positions address the conversation's absolute line
 * index plus a 0-based column; the selection is inclusive of the head
 * cell, matching what the user sees under the pointer.
 */
import { Line, Span, lineText, span } from './lines.js';

export interface SelPos {
  line: number;
  col: number;
}

export interface Selection {
  anchor: SelPos;
  head: SelPos;
}

export function isEmptySelection(sel: Selection): boolean {
  return sel.anchor.line === sel.head.line && sel.anchor.col === sel.head.col;
}

export function orderSelection(sel: Selection): { start: SelPos; end: SelPos } {
  const { anchor, head } = sel;
  if (anchor.line < head.line || (anchor.line === head.line && anchor.col <= head.col)) {
    return { start: anchor, end: head };
  }
  return { start: head, end: anchor };
}

/** Column range [startCol, endCol] selected on `line`, or null. */
function lineRange(sel: Selection, line: number, width: number): [number, number] | null {
  const { start, end } = orderSelection(sel);
  if (line < start.line || line > end.line) return null;
  const from = line === start.line ? start.col : 0;
  const to = line === end.line ? end.col : Math.max(0, width - 1);
  if (from > to) return null;
  return [from, to];
}

/** Re-style a line so columns [from, to] render inverse (the highlight). */
export function applyHighlight(line: Line, from: number, to: number): Line {
  const out: Span[] = [];
  let col = 0;
  for (const s of line) {
    const start = col;
    const end = col + s.text.length; // exclusive
    col = end;
    if (end <= from || start > to) {
      out.push(s);
      continue;
    }
    const cutA = Math.max(from - start, 0);
    const cutB = Math.min(to + 1 - start, s.text.length);
    if (cutA > 0) out.push({ ...s, text: s.text.slice(0, cutA) });
    out.push({ ...s, text: s.text.slice(cutA, cutB), inverse: true });
    if (cutB < s.text.length) out.push({ ...s, text: s.text.slice(cutB) });
  }
  // Selecting past the end of a short line highlights a phantom cell so
  // full-line sweeps read continuously.
  if (col <= to && col >= from) {
    out.push(span(' ', { inverse: true }));
  }
  return out;
}

/** Apply the selection highlight to the full line buffer. */
export function highlightLines(lines: Line[], sel: Selection | null, width: number): Line[] {
  if (!sel || isEmptySelection(sel)) return lines;
  return lines.map((line, i) => {
    const range = lineRange(sel, i, width);
    return range ? applyHighlight(line, range[0], range[1]) : line;
  });
}

/** The selected text, one string per line, trailing whitespace trimmed. */
export function extractSelection(lines: Line[], sel: Selection): string {
  const { start, end } = orderSelection(sel);
  const parts: string[] = [];
  for (let i = start.line; i <= end.line; i++) {
    const text = lineText(lines[i] ?? []);
    const from = i === start.line ? start.col : 0;
    const to = i === end.line ? end.col + 1 : text.length;
    parts.push(text.slice(from, to).trimEnd());
  }
  return parts.join('\n');
}

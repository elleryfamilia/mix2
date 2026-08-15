import { describe, expect, it } from 'vitest';
import { span, type Line } from './lines.js';
import { applyHighlight, extractSelection, highlightLines } from './selection.js';

const lines: Line[] = [
  [span('  ❯ hello team')],
  [span('')],
  [span('  the answer is '), span('42', { bold: true }), span(' obviously')],
];

describe('selection', () => {
  it('extracts single-line text inclusively', () => {
    const text = extractSelection(lines, {
      anchor: { line: 0, col: 4 },
      head: { line: 0, col: 8 },
    });
    expect(text).toBe('hello');
  });

  it('extracts multi-line text and trims trailing whitespace', () => {
    const text = extractSelection(lines, {
      anchor: { line: 0, col: 2 },
      head: { line: 2, col: 17 },
    });
    expect(text).toBe('❯ hello team\n\n  the answer is 42');
  });

  it('normalizes a backwards drag', () => {
    const text = extractSelection(lines, {
      anchor: { line: 2, col: 17 },
      head: { line: 0, col: 2 },
    });
    expect(text.startsWith('❯ hello team')).toBe(true);
  });

  it('applies inverse styling across span boundaries', () => {
    const line = lines[2]!;
    const highlighted = applyHighlight(line, 14, 17);
    const inverse = highlighted.filter((s) => s.inverse).map((s) => s.text).join('');
    expect(inverse).toBe('s 42');
    // Styling is preserved on the split spans.
    expect(highlighted.find((s) => s.text === '42')?.bold).toBe(true);
    // Text content is unchanged.
    expect(highlighted.map((s) => s.text).join('')).toBe('  the answer is 42 obviously');
  });

  it('highlights whole middle lines and phantom cells on empty lines', () => {
    const out = highlightLines(
      lines,
      { anchor: { line: 0, col: 0 }, head: { line: 2, col: 3 } },
      80,
    );
    // The empty middle line gets a phantom highlighted cell.
    expect(out[1]!.some((s) => s.inverse)).toBe(true);
  });

  it('empty selection changes nothing', () => {
    const out = highlightLines(lines, { anchor: { line: 1, col: 2 }, head: { line: 1, col: 2 } }, 80);
    expect(out).toEqual(lines);
  });
});

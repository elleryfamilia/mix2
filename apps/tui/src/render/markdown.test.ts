import { describe, expect, it } from 'vitest';
import { lineText } from './lines.js';
import { inlineSpans, markdownLines, wrapSpans } from './markdown.js';

function texts(md: string, width = 60): string[] {
  return markdownLines(md, width).map(lineText);
}

describe('inline markdown', () => {
  it('renders bold without the asterisks', () => {
    const spans = inlineSpans('fix the **startup tail** now');
    expect(spans.map((s) => s.text).join('')).toBe('fix the startup tail now');
    expect(spans.find((s) => s.text === 'startup tail')?.bold).toBe(true);
  });

  it('renders inline code distinctly', () => {
    const spans = inlineSpans('see `App.tsx:66` for details');
    const code = spans.find((s) => s.text === 'App.tsx:66');
    expect(code).toBeDefined();
    expect(code?.bgColor).toBeDefined();
  });

  it('renders italics', () => {
    const spans = inlineSpans('this is *emphasis* here');
    expect(spans.find((s) => s.text === 'emphasis')?.italic).toBe(true);
  });

  it('passes plain text through untouched', () => {
    const spans = inlineSpans('2 * 3 equals 6, a_b_c stays');
    expect(spans.map((s) => s.text).join('')).toBe('2 * 3 equals 6, a_b_c stays');
  });
});

describe('block markdown', () => {
  it('strips heading markers and keeps the title', () => {
    const lines = texts('## Short answer\n\nkeep the split');
    expect(lines).toContain('Short answer');
    expect(lines.join('\n')).not.toContain('##');
  });

  it('renders numbered lists with hanging indent', () => {
    const lines = texts(
      '1. **Fix the startup tail.** The probe blocks ready for a long time on slow machines.',
      40,
    );
    expect(lines[0]).toMatch(/^1 {2}Fix the startup tail\./);
    // Continuation lines align under the text, not the number.
    expect(lines[1]).toMatch(/^ {3}\S/);
  });

  it('renders bullets with a dim glyph', () => {
    const lines = texts('- first point\n- second point');
    expect(lines[0]).toBe('• first point');
    expect(lines[1]).toBe('• second point');
  });

  it('renders fenced code with a gutter and no fence markers', () => {
    const lines = texts('```rust\nlet x = 1;\n```\nafter');
    expect(lines).toContain('│ let x = 1;');
    expect(lines.join('\n')).not.toContain('```');
    expect(lines).toContain('after');
  });

  it('renders blockquotes quietly', () => {
    const lines = texts('> a quoted thought');
    expect(lines[0]).toBe('▏ a quoted thought');
  });

  it('never exceeds the width for prose', () => {
    const md = 'word '.repeat(50) + '\n\n1. ' + 'item '.repeat(30);
    for (const line of texts(md, 40)) {
      expect(line.length).toBeLessThanOrEqual(40);
    }
  });

  it('never loses content', () => {
    const md = '## H\n\npara **bold** `code`\n\n- a\n- b\n\n1. one\n\n> q\n\n```\ncode line\n```';
    const joined = texts(md).join('\n');
    for (const needle of ['H', 'para', 'bold', 'code', 'a', 'b', 'one', 'q', 'code line']) {
      expect(joined).toContain(needle);
    }
  });
});

describe('wrapSpans', () => {
  it('preserves style across wraps', () => {
    const lines = wrapSpans([{ text: 'aaa bbb ccc ddd', bold: true }], 7);
    expect(lines.length).toBeGreaterThan(1);
    for (const line of lines) {
      for (const s of line) {
        if (s.text.trim()) expect(s.bold).toBe(true);
      }
    }
  });
});

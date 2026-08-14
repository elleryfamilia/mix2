/**
 * Minimal markdown → styled-line renderer for agent responses.
 *
 * Agents write markdown; dumping it raw reads as noise. This renders the
 * subset that matters in a terminal — headings, lists, inline bold/italic/
 * code, fenced code blocks, blockquotes, rules — using the design system's
 * quiet vocabulary (dim bullets and numbers, hairline gutters, no boxes).
 * Anything unrecognized falls through as plain text; rendering must never
 * lose content.
 */
import { theme } from '../theme/theme.js';
import { Line, Span, span } from './lines.js';

/** Inline tokens: **bold**, *italic* / _italic_, `code`. */
export function inlineSpans(text: string, base: Omit<Span, 'text'> = {}): Span[] {
  const spans: Span[] = [];
  // Underscore emphasis only at word boundaries, so identifiers like
  // `a_b_c` pass through untouched.
  const pattern = /(\*\*[^*]+\*\*|\*[^*\s][^*]*\*|(?<![\w])_[^_\s][^_]*_(?![\w])|`[^`]+`)/g;
  let last = 0;
  for (const match of text.matchAll(pattern)) {
    const index = match.index ?? 0;
    if (index > last) spans.push({ text: text.slice(last, index), ...base });
    const token = match[0];
    if (token.startsWith('**')) {
      spans.push({ ...base, text: token.slice(2, -2), bold: true });
    } else if (token.startsWith('`')) {
      spans.push({
        text: token.slice(1, -1),
        color: theme.text.secondary,
        bgColor: theme.status.barBg,
      });
    } else {
      spans.push({ ...base, text: token.slice(1, -1), italic: true });
    }
    last = index + token.length;
  }
  if (last < text.length) spans.push({ text: text.slice(last), ...base });
  return spans.filter((s) => s.text.length > 0);
}

/** Wrap styled spans to a width, preserving styles across line breaks. */
export function wrapSpans(spans: Span[], width: number, hangIndent = 0): Line[] {
  const lines: Line[] = [];
  let current: Span[] = [];
  let currentLen = 0;
  const indentSpan = () => (lines.length > 0 && hangIndent > 0 ? [span(' '.repeat(hangIndent))] : []);

  const pushLine = () => {
    lines.push([...indentSpan(), ...current]);
    current = [];
    currentLen = 0;
  };

  for (const s of spans) {
    const words = s.text.split(/(\s+)/);
    for (const word of words) {
      if (word.length === 0) continue;
      const avail = width - (lines.length > 0 ? hangIndent : 0);
      if (currentLen + word.length > avail && currentLen > 0 && word.trim().length > 0) {
        pushLine();
      }
      if (word.trim().length === 0 && currentLen === 0) continue; // no leading spaces
      current.push({ ...s, text: word });
      currentLen += word.length;
    }
  }
  if (current.length > 0 || lines.length === 0) pushLine();
  // Merge adjacent spans with identical style for cleaner output.
  return lines.map(mergeLine);
}

function mergeLine(line: Line): Line {
  const out: Span[] = [];
  for (const s of line) {
    const prev = out[out.length - 1];
    if (
      prev &&
      prev.color === s.color &&
      prev.bgColor === s.bgColor &&
      prev.bold === s.bold &&
      prev.italic === s.italic &&
      prev.inverse === s.inverse
    ) {
      prev.text += s.text;
    } else {
      out.push({ ...s });
    }
  }
  return out;
}

/**
 * Render a markdown block of text to lines at `width`. Every line is
 * unindented; the caller applies the conversation inset.
 */
export function markdownLines(text: string, width: number): Line[] {
  const out: Line[] = [];
  const lines = text.split('\n');
  let inFence = false;

  for (const raw of lines) {
    if (raw.trim().startsWith('```')) {
      inFence = !inFence;
      continue; // the fence markers themselves are noise
    }
    if (inFence) {
      // Code lines: hairline gutter, no wrap (truncate), secondary color.
      const code = raw.length > width - 2 ? raw.slice(0, width - 3) + '…' : raw;
      out.push([
        span('│ ', { color: theme.border.bridge }),
        span(code, { color: theme.text.secondary }),
      ]);
      continue;
    }

    const line = raw.trimEnd();
    if (line.trim() === '') {
      out.push([span('')]);
      continue;
    }

    // Horizontal rule
    if (/^\s*(-{3,}|\*{3,}|_{3,})\s*$/.test(line)) {
      out.push([span('─'.repeat(Math.min(width, 40)), { color: theme.border.bridge })]);
      continue;
    }

    // Headings: bold, blank line before, level shown by weight only.
    const heading = line.match(/^(#{1,4})\s+(.*)$/);
    if (heading) {
      if (out.length > 0 && (out[out.length - 1]?.[0]?.text ?? '') !== '') {
        out.push([span('')]);
      }
      out.push(
        ...wrapSpans(
          inlineSpans(heading[2] ?? '', { bold: true, color: theme.text.primary }),
          width,
        ).map((l) => l.map((s) => ({ ...s, bold: true }))),
      );
      continue;
    }

    // Blockquote
    const quote = line.match(/^\s*>\s?(.*)$/);
    if (quote) {
      const body = wrapSpans(
        inlineSpans(quote[1] ?? '', { color: theme.text.muted, italic: true }),
        width - 2,
      );
      out.push(...body.map((l) => [span('▏ ', { color: theme.border.bridge }), ...l]));
      continue;
    }

    // Bullets: -, *, + (dim glyph, hanging indent)
    const bullet = line.match(/^(\s*)[-*+]\s+(.*)$/);
    if (bullet) {
      const depth = Math.min(Math.floor((bullet[1]?.length ?? 0) / 2), 3);
      const indent = '  '.repeat(depth);
      const body = wrapSpans(
        inlineSpans(bullet[2] ?? '', { color: theme.text.primary }),
        width - indent.length - 2,
        2,
      );
      out.push(
        ...body.map((l, i) =>
          i === 0
            ? [span(indent), span('• ', { color: theme.text.muted }), ...l]
            : [span(indent), ...l],
        ),
      );
      continue;
    }

    // Numbered lists: dim number, hanging indent (design's dim-number style)
    const numbered = line.match(/^(\s*)(\d{1,2})[.)]\s+(.*)$/);
    if (numbered) {
      const indent = numbered[1] ?? '';
      const num = numbered[2] ?? '';
      const marker = `${num}  `;
      const body = wrapSpans(
        inlineSpans(numbered[3] ?? '', { color: theme.text.primary }),
        width - indent.length - marker.length,
        marker.length,
      );
      out.push(
        ...body.map((l, i) =>
          i === 0
            ? [span(indent), span(marker, { color: theme.text.muted }), ...l]
            : [span(indent), ...l],
        ),
      );
      continue;
    }

    // Plain paragraph line
    out.push(...wrapSpans(inlineSpans(line, { color: theme.text.primary }), width));
  }

  // Trim trailing blank lines
  while (out.length > 0 && (out[out.length - 1]?.map((s) => s.text).join('') ?? '') === '') {
    out.pop();
  }
  return out;
}

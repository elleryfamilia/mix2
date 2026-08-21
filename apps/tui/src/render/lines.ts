/**
 * Line-based rendering primitives. Conversation content is produced as
 * arrays of styled spans, which makes viewport scrolling exact (slice of
 * lines) and snapshot tests trivial (join the span texts).
 */
import stringWidth from 'string-width';

export interface Span {
  text: string;
  color?: string;
  bgColor?: string;
  bold?: boolean;
  italic?: boolean;
  inverse?: boolean;
}

export type Line = Span[];

export function span(text: string, style: Omit<Span, 'text'> = {}): Span {
  return { text, ...style };
}

export function lineText(line: Line): string {
  return line.map((s) => s.text).join('');
}

export function lineWidth(line: Line): number {
  return lineText(line).length;
}

/** Wrap plain text to a width, preserving explicit newlines and breaking on
 * spaces where possible. */
export function wrapText(text: string, width: number): string[] {
  if (width <= 0) return [text];
  const out: string[] = [];
  for (const paragraph of text.split('\n')) {
    if (paragraph.length <= width) {
      out.push(paragraph);
      continue;
    }
    let rest = paragraph;
    while (rest.length > width) {
      let cut = rest.lastIndexOf(' ', width);
      if (cut <= 0) cut = width;
      out.push(rest.slice(0, cut).trimEnd());
      rest = rest.slice(cut).trimStart();
    }
    out.push(rest);
  }
  return out;
}

export function truncate(text: string, width: number): string {
  if (text.length <= width) return text;
  if (width <= 1) return text.slice(0, Math.max(0, width));
  return text.slice(0, width - 1) + '…';
}

/** Terminal display width of `text`: fullwidth/CJK characters count as 2
 * columns, unlike `.length` which counts UTF-16 code units. Every other
 * measurement in this module uses `.length` — reach for this only where
 * content may contain wide characters (see conversation.ts's stance
 * renderer, its one caller). */
export function displayWidth(text: string): number {
  return stringWidth(text);
}

/** Truncate `text` to at most `max` display columns, appending `…` when it
 * doesn't fit. Measures by `displayWidth`, so wide characters never push
 * the result past `max` columns the way `truncate`'s length-based cut
 * would. */
export function truncateDisplay(text: string, max: number): string {
  if (max <= 0) return '';
  if (displayWidth(text) <= max) return text;
  if (max === 1) return '…';
  let result = text;
  while (result.length > 0 && displayWidth(result) > max - 1) {
    result = result.slice(0, -1);
  }
  return result + '…';
}

/** Pad a line with a spacer span so `right` lands on the right edge. */
export function spread(left: Line, right: Line, width: number): Line {
  const gap = width - lineWidth(left) - lineWidth(right);
  if (gap <= 1) return [...left, span(' '), ...right];
  return [...left, span(' '.repeat(gap)), ...right];
}

export function indent(line: Line, spaces: number): Line {
  return [span(' '.repeat(spaces)), ...line];
}

export const BLANK: Line = [span('')];

/** A bordered tile — one of the design system's sanctioned box exceptions
 * (consultation tiles, the composer, the team picker's slot columns). */
export interface TileSpec {
  headerLeft: Line;
  headerRight: Line;
  body: Line[];
  borderColor: string;
}

export function buildTile(spec: TileSpec, width: number): Line[] {
  const inner = width - 2;
  const border = { color: spec.borderColor };
  const hl = spec.headerLeft.reduce((n, s) => n + s.text.length, 0);
  const hr = spec.headerRight.reduce((n, s) => n + s.text.length, 0);
  // With no right header the dashes run flush to the corner.
  const top: Line =
    hr === 0
      ? [
          span('╭ ', border),
          ...spec.headerLeft,
          span(' '),
          span('─'.repeat(Math.max(1, inner - 2 - hl)) + '╮', border),
        ]
      : [
          span('╭ ', border),
          ...spec.headerLeft,
          span(' '),
          span('─'.repeat(Math.max(1, inner - 2 - hl - hr - 2)), border),
          span(' '),
          ...spec.headerRight,
          span(' ╮', border),
        ];
  const rows = spec.body.map((line) => {
    const text = line.reduce((n, s) => n + s.text.length, 0);
    const fill = Math.max(0, inner - 2 - text);
    return [span('│ ', border), ...line, span(' '.repeat(fill)), span(' │', border)];
  });
  const bottom: Line = [span('╰' + '─'.repeat(Math.max(0, width - 2)) + '╯', border)];
  return [top, ...rows, bottom];
}

/** Stitch two tiles side by side with a two-space left margin. */
export function zipTiles(left: Line[], right: Line[], gap: number): Line[] {
  const height = Math.max(left.length, right.length);
  const leftWidth = Math.max(...left.map((l) => l.reduce((n, s) => n + s.text.length, 0)));
  const out: Line[] = [];
  for (let i = 0; i < height; i++) {
    const l = left[i] ?? [span(' '.repeat(leftWidth))];
    const lw = l.reduce((n, s) => n + s.text.length, 0);
    const r = right[i] ?? [];
    out.push([span('  '), ...l, span(' '.repeat(Math.max(0, leftWidth - lw) + gap)), ...r]);
  }
  return out;
}

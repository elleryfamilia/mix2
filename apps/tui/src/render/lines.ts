/**
 * Line-based rendering primitives. Conversation content is produced as
 * arrays of styled spans, which makes viewport scrolling exact (slice of
 * lines) and snapshot tests trivial (join the span texts).
 */

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

/**
 * The /model picker: each agent's available models side by side, with the
 * active choice marked and a cursor for keyboard selection. "provider
 * default" is always the first entry — mix2 never second-guesses the
 * user's CLI configuration unless asked.
 */
import type { SessionInfo } from '../state/store.js';
import {
  MAX_CONTENT_WIDTH,
  TILE_BREAKPOINT,
  agentColor,
  agentGlyph,
  displayName,
  theme,
} from '../theme/theme.js';
import { BLANK, Line, span, spread } from './lines.js';

const INDENT = 2;

export interface ModelCursor {
  /** 0 = lead column, 1 = teammate column. */
  column: 0 | 1;
  index: number;
}

export const PROVIDER_DEFAULT = 'provider default';

/** The selectable entries for an agent: default first, then known models. */
export function modelEntries(models: string[]): string[] {
  return [PROVIDER_DEFAULT, ...models];
}

function columnLines(
  info: SessionInfo['lead'],
  active: boolean,
  cursorIndex: number,
  width: number,
): Line[] {
  const kind = info.kind;
  const lines: Line[] = [];
  lines.push([
    span(agentGlyph(kind), { color: agentColor(kind) }),
    span(` ${displayName(kind)}`, { color: agentColor(kind), bold: true }),
  ]);
  const entries = modelEntries(info.models ?? []);
  entries.forEach((entry, i) => {
    const isCurrent = entry === PROVIDER_DEFAULT ? !info.model : info.model === entry;
    const isCursor = active && i === cursorIndex;
    const marker = isCursor ? '›' : ' ';
    const current = isCurrent ? ' ●' : '';
    const label = entry.length > width - 6 ? entry.slice(0, width - 7) + '…' : entry;
    lines.push([
      span(`${marker} `, { color: theme.agent.team, bold: true }),
      span(label, {
        color: isCursor
          ? theme.text.primary
          : isCurrent
            ? agentColor(kind)
            : theme.text.secondary,
        bold: isCursor,
        inverse: isCursor,
      }),
      span(current, { color: agentColor(kind) }),
    ]);
  });
  return lines;
}

export function renderModelPanel(
  session: SessionInfo,
  cursor: ModelCursor,
  width: number,
): Line[] {
  const w = Math.min(width, MAX_CONTENT_WIDTH);
  const lines: Line[] = [];
  lines.push(
    spread(
      [
        span(' '.repeat(INDENT)),
        span('◐ models', { color: theme.agent.team, bold: true }),
        span(' — pick per agent', { color: theme.text.faint }),
      ],
      [span('esc close ', { color: theme.text.faint })],
      w,
    ),
  );
  lines.push(BLANK);

  const stacked = width < TILE_BREAKPOINT;
  const colWidth = stacked ? w - INDENT : Math.floor((w - INDENT) / 2) - 2;
  const left = columnLines(session.lead, cursor.column === 0, cursor.index, colWidth);
  const right = columnLines(session.teammate, cursor.column === 1, cursor.index, colWidth);

  if (stacked) {
    for (const line of left) lines.push([span(' '.repeat(INDENT)), ...line]);
    lines.push(BLANK);
    for (const line of right) lines.push([span(' '.repeat(INDENT)), ...line]);
  } else {
    const height = Math.max(left.length, right.length);
    for (let i = 0; i < height; i++) {
      const l = left[i] ?? [span('')];
      const lw = l.reduce((n, s) => n + s.text.length, 0);
      lines.push([
        span(' '.repeat(INDENT)),
        ...l,
        span(' '.repeat(Math.max(0, colWidth - lw) + 4)),
        ...(right[i] ?? []),
      ]);
    }
  }
  lines.push(BLANK);
  lines.push([
    span(' '.repeat(INDENT)),
    span('↑↓ choose · ←→ agent · enter apply · esc close', { color: theme.text.faint }),
  ]);
  return lines;
}

/**
 * The /model picker: each slot's available models side by side, with the
 * active choice marked and a cursor for keyboard selection. "provider
 * default" is always the first entry — mix2 never second-guesses the
 * user's CLI configuration unless asked. Long lists (some harnesses expose
 * dozens of models) are handled by type-to-filter plus a scrolling window
 * with above/below counts, so the panel never outgrows the terminal.
 */
import type { SessionInfo } from '../state/store.js';
import { leadInfo, teammateInfo } from '../state/store.js';
import {
  MAX_CONTENT_WIDTH,
  TILE_BREAKPOINT,
  agentColor,
  agentGlyph,
  theme,
} from '../theme/theme.js';
import { BLANK, Line, span, spread } from './lines.js';

const INDENT = 2;

/** Rows of model entries visible per column before windowing kicks in. */
export const MODEL_WINDOW = 8;

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

/** Entries surviving the type-to-filter query (case-insensitive substring).
 * An empty query keeps everything. */
export function filteredModelEntries(models: string[], filter: string): string[] {
  const entries = modelEntries(models);
  const query = filter.trim().toLowerCase();
  if (!query) return entries;
  return entries.filter((entry) => entry.toLowerCase().includes(query));
}

function columnLines(
  info: SessionInfo['one'],
  active: boolean,
  cursorIndex: number,
  width: number,
  filter: string,
): Line[] {
  const slot = info.slot;
  const lines: Line[] = [];
  lines.push([
    span(agentGlyph(slot), { color: agentColor(slot) }),
    span(` ${info.name}`, { color: agentColor(slot), bold: true }),
  ]);
  const entries = filteredModelEntries(info.models ?? [], filter);
  if (entries.length === 0) {
    lines.push([span('  no models match', { color: theme.text.faint })]);
    return lines;
  }

  // Window the list around the cursor (inactive columns start at the top).
  const cursor = active ? Math.min(cursorIndex, entries.length - 1) : 0;
  let start = Math.max(0, cursor - Math.floor(MODEL_WINDOW / 2));
  start = Math.min(start, Math.max(0, entries.length - MODEL_WINDOW));
  const visible = entries.slice(start, start + MODEL_WINDOW);
  const above = start;
  const below = entries.length - start - visible.length;

  if (above > 0) {
    lines.push([span(`  ↑ ${above} more`, { color: theme.text.faint })]);
  }
  visible.forEach((entry, offset) => {
    const i = start + offset;
    const isCurrent = entry === PROVIDER_DEFAULT ? !info.model : info.model === entry;
    const isCursor = active && i === cursor;
    const marker = isCursor ? '›' : ' ';
    const current = isCurrent ? ' ●' : '';
    const label = entry.length > width - 6 ? entry.slice(0, width - 7) + '…' : entry;
    lines.push([
      span(`${marker} `, { color: theme.agent.team, bold: true }),
      span(label, {
        color: isCursor
          ? theme.text.primary
          : isCurrent
            ? agentColor(slot)
            : theme.text.secondary,
        bold: isCursor,
        inverse: isCursor,
      }),
      span(current, { color: agentColor(slot) }),
    ]);
  });
  if (below > 0) {
    lines.push([span(`  ↓ ${below} more`, { color: theme.text.faint })]);
  }
  return lines;
}

export function renderModelPanel(
  session: SessionInfo,
  cursor: ModelCursor,
  width: number,
  filter = '',
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
      [span('esc cancel ', { color: theme.text.faint })],
      w,
    ),
  );
  if (filter) {
    lines.push([
      span(' '.repeat(INDENT)),
      span('filter: ', { color: theme.text.faint }),
      span(filter, { color: theme.text.primary, bold: true }),
    ]);
  }
  lines.push(BLANK);

  const stacked = width < TILE_BREAKPOINT;
  const colWidth = stacked ? w - INDENT : Math.floor((w - INDENT) / 2) - 2;
  const left = columnLines(leadInfo(session), cursor.column === 0, cursor.index, colWidth, filter);
  const right = columnLines(
    teammateInfo(session),
    cursor.column === 1,
    cursor.index,
    colWidth,
    filter,
  );

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
    span('type to filter · ↑↓ choose · ←→ agent · enter apply · esc cancel', {
      color: theme.text.faint,
    }),
  ]);
  return lines;
}

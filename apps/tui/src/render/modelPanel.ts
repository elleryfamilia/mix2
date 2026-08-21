/**
 * The /model picker, shaped like the team picker: one bordered tile per
 * agent listing its models, then a continue button that applies the
 * equipped choices — enter equips, continue commits. "provider default"
 * is always the first entry — mix2 never second-guesses the user's CLI
 * configuration unless asked. Long lists (some harnesses expose dozens of
 * models) are handled by type-to-filter plus a scrolling window with
 * above/below counts, so a tile never outgrows the terminal.
 */
import type { AgentInfo } from '../ipc/protocol.js';
import type { SessionInfo } from '../state/store.js';
import { leadInfo, teammateInfo } from '../state/store.js';
import {
  MAX_CONTENT_WIDTH,
  TILE_BREAKPOINT,
  agentColor,
  agentGlyph,
  theme,
} from '../theme/theme.js';
import { BLANK, Line, buildTile, span, spread, truncate, zipTiles } from './lines.js';

const INDENT = 2;

/** Rows of model entries visible per column before windowing kicks in. */
export const MODEL_WINDOW = 8;

/** Picker cursor: which control has focus and which row is under it. */
export interface ModelCursor {
  /** 0 = lead column, 1 = teammate column, 2 = continue button. */
  column: 0 | 1 | 2;
  index: number;
}

/** Pending model per slot: a model name, or null for the provider default.
 * Nothing is sent to the core until continue commits the pair. */
export interface ModelSelection {
  one: string | null;
  two: string | null;
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

/** The panel's initial selection: each agent's active model. */
export function initialModelSelection(session: SessionInfo): ModelSelection {
  return { one: session.one.model ?? null, two: session.two.model ?? null };
}

/** Where an agent's equipped entry sits in its filtered list (cursor
 * seeding); the top when the filter has hidden it. */
export function modelEntryIndexOf(
  info: AgentInfo,
  selection: ModelSelection,
  filter: string,
): number {
  const chosen = selection[info.slot] ?? PROVIDER_DEFAULT;
  return Math.max(0, filteredModelEntries(info.models ?? [], filter).indexOf(chosen));
}

/** One agent's model list as a bordered tile: the border carries the
 * slot's identity color when focused (quiet otherwise), and the equipped
 * entry reads in the slot color — matching the team picker's slot tiles. */
function modelTile(
  info: AgentInfo,
  selection: ModelSelection,
  active: boolean,
  cursorIndex: number,
  width: number,
  filter: string,
): Line[] {
  const slot = info.slot;
  const chosen = selection[slot] ?? PROVIDER_DEFAULT;
  const bodyWidth = width - 4;
  const body: Line[] = [];
  const entries = filteredModelEntries(info.models ?? [], filter);
  if (entries.length === 0) {
    body.push([span('no models match', { color: theme.text.faint })]);
  } else {
    // Window the list around the cursor (inactive columns anchor on the
    // equipped entry so its mark stays visible).
    const anchor = active
      ? Math.min(cursorIndex, entries.length - 1)
      : Math.max(0, entries.indexOf(chosen));
    let start = Math.max(0, anchor - Math.floor(MODEL_WINDOW / 2));
    start = Math.min(start, Math.max(0, entries.length - MODEL_WINDOW));
    const visible = entries.slice(start, start + MODEL_WINDOW);
    const above = start;
    const below = entries.length - start - visible.length;

    if (above > 0) {
      body.push([span(`  ↑ ${above} more`, { color: theme.text.faint })]);
    }
    visible.forEach((entry, offset) => {
      const i = start + offset;
      const isChosen = entry === chosen;
      const isCursor = active && i === anchor;
      const marker = isCursor ? '›' : ' ';
      const chosenMark = isChosen ? ' ●' : '';
      const label = truncate(entry, Math.max(8, bodyWidth - 4));
      body.push([
        span(`${marker} `, { color: theme.agent.team, bold: true }),
        span(label, {
          color: isChosen
            ? agentColor(slot)
            : isCursor
              ? theme.text.primary
              : theme.text.secondary,
          bold: isCursor || isChosen,
          inverse: isCursor,
        }),
        span(chosenMark, { color: agentColor(slot) }),
      ]);
    });
    if (below > 0) {
      body.push([span(`  ↓ ${below} more`, { color: theme.text.faint })]);
    }
  }
  return buildTile(
    {
      headerLeft: [
        span(agentGlyph(slot), { color: agentColor(slot) }),
        span(` ${info.name}`, { color: agentColor(slot), bold: true }),
      ],
      headerRight: [],
      body,
      borderColor: active ? agentColor(slot) : theme.border.bridge,
    },
    width,
  );
}

export function renderModelPanel(
  session: SessionInfo,
  selection: ModelSelection,
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
  const colWidth = stacked ? w - INDENT : Math.floor((w - INDENT - 3) / 2);
  const left = modelTile(
    leadInfo(session),
    selection,
    cursor.column === 0,
    cursor.index,
    colWidth,
    filter,
  );
  const right = modelTile(
    teammateInfo(session),
    selection,
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
    lines.push(...zipTiles(left, right, 3));
  }

  lines.push(BLANK);
  const continueActive = cursor.column === 2;
  lines.push([
    span(' '.repeat(INDENT)),
    span(continueActive ? '› ' : '  ', { color: theme.agent.team, bold: true }),
    span(' continue ', {
      color: continueActive ? theme.text.primary : theme.text.muted,
      bold: continueActive,
      inverse: continueActive,
    }),
  ]);

  lines.push(BLANK);
  // The hint follows the focused control, matching the team picker: agent
  // columns equip, the continue button is where the choices apply.
  const hint =
    cursor.column === 2
      ? 'enter apply · ←→ back · esc cancel'
      : 'type to filter · ↑↓ choose · enter equip · ←→ switch · esc cancel';
  lines.push([span(' '.repeat(INDENT)), span(hint, { color: theme.text.faint })]);
  return lines;
}

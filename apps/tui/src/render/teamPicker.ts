/**
 * The startup team picker (`selecting-team` phase): one column per slot
 * listing every discovered harness, plus a lead-slot control. The
 * configured proposal arrives preselected; unavailable or ineligible
 * entries stay visible but disabled, each carrying its actionable reason.
 * The same harness on both slots is a supported choice, not an error.
 */
import type { DiscoveredHarness } from '../ipc/protocol.js';
import type { DiscoveryState } from '../state/store.js';
import {
  MAX_CONTENT_WIDTH,
  TILE_BREAKPOINT,
  agentColor,
  agentGlyph,
  glyphs,
  theme,
  type SlotName,
} from '../theme/theme.js';
import { BLANK, Line, span, spread, truncate, wrapText } from './lines.js';

const INDENT = 2;

/** Picker cursor: which control has focus and which row is under it. */
export interface TeamPickerCursor {
  /** 0 = slot one column, 1 = slot two column, 2 = lead control. */
  column: 0 | 1 | 2;
  index: number;
}

export interface TeamPickerSelection {
  one: string;
  two: string;
  leadSlot: SlotName;
}

/** Harness choices for a slot: unique by harness name, preferring an
 * available entry when the same harness was probed at several commands. */
export function pickerEntries(discovery: DiscoveryState): DiscoveredHarness[] {
  const byHarness = new Map<string, DiscoveredHarness>();
  for (const entry of discovery.harnesses) {
    const existing = byHarness.get(entry.harness);
    if (!existing || (!existing.available && entry.available)) {
      byHarness.set(entry.harness, entry);
    }
  }
  return [...byHarness.values()];
}

/** Whether an entry can be chosen for a slot at all. */
export function selectable(entry: DiscoveredHarness, slot: SlotName, leadSlot: SlotName): boolean {
  if (!entry.available || entry.auth === 'unauthenticated') return false;
  return slot === leadSlot ? entry.lead_eligible : entry.teammate_eligible;
}

/** One-line status label for a disabled entry. */
function disabledLabel(entry: DiscoveredHarness, slot: SlotName, leadSlot: SlotName): string {
  if (!entry.available) return entry.reason ?? 'unavailable';
  if (entry.auth === 'unauthenticated') return entry.reason ?? 'not signed in';
  if (slot === leadSlot && !entry.lead_eligible) return 'teammate-only for now';
  return 'not eligible';
}

/** The picker's initial selection: the core's proposal. */
export function initialSelection(discovery: DiscoveryState): TeamPickerSelection {
  return {
    one: discovery.proposal.one,
    two: discovery.proposal.two,
    leadSlot: discovery.proposal.lead_slot,
  };
}

/** Where a harness sits in the picker's entry list (cursor seeding). */
export function entryIndexOf(discovery: DiscoveryState, harness: string): number {
  return Math.max(
    0,
    pickerEntries(discovery).findIndex((e) => e.harness === harness),
  );
}

function columnLines(
  discovery: DiscoveryState,
  slot: SlotName,
  selection: TeamPickerSelection,
  active: boolean,
  cursorIndex: number,
  width: number,
): Line[] {
  const entries = pickerEntries(discovery);
  const chosen = slot === 'one' ? selection.one : selection.two;
  const isLead = selection.leadSlot === slot;
  const lines: Line[] = [];
  lines.push([
    span(agentGlyph(slot), { color: agentColor(slot) }),
    span(` slot ${slot}`, { color: agentColor(slot), bold: true }),
    span(isLead ? '  · coordinates' : '', { color: theme.text.faint }),
  ]);
  entries.forEach((entry, i) => {
    const enabled = selectable(entry, slot, selection.leadSlot);
    const isChosen = entry.harness === chosen;
    const isCursor = active && i === cursorIndex;
    const marker = isCursor ? '›' : ' ';
    const chosenMark = isChosen ? ' ●' : '';
    const version = entry.version ? `  ${entry.version}` : '';
    const label = truncate(`${entry.harness}${version}`, Math.max(8, width - 6));
    lines.push([
      span(`${marker} `, { color: theme.agent.team, bold: true }),
      span(label, {
        color: !enabled ? theme.text.faint : isCursor ? theme.text.primary : theme.text.secondary,
        bold: isCursor,
        inverse: isCursor,
      }),
      span(chosenMark, { color: agentColor(slot) }),
    ]);
    if (!enabled) {
      const reason = truncate(disabledLabel(entry, slot, selection.leadSlot), Math.max(8, width - 4));
      lines.push([span('   '), span(reason, { color: theme.text.faint })]);
    } else if (entry.note && isChosen) {
      // Selection disclosures (e.g. a trust flag) surface right where the
      // choice is made — nothing is passed silently.
      const note = truncate(entry.note, Math.max(8, width - 4));
      lines.push([span('   '), span(note, { color: theme.text.faint })]);
    }
  });
  return lines;
}

export function renderTeamPicker(
  discovery: DiscoveryState,
  selection: TeamPickerSelection,
  cursor: TeamPickerCursor,
  width: number,
  selectionError?: string,
): Line[] {
  const w = Math.min(width, MAX_CONTENT_WIDTH);
  const lines: Line[] = [];
  lines.push(
    spread(
      [
        span(' '.repeat(INDENT)),
        span('◐ pick your team', { color: theme.agent.team, bold: true }),
        span(' — two slots, any agents', { color: theme.text.faint }),
      ],
      [span('esc defaults ', { color: theme.text.faint })],
      w,
    ),
  );
  lines.push(BLANK);

  const stacked = width < TILE_BREAKPOINT;
  const colWidth = stacked ? w - INDENT : Math.floor((w - INDENT) / 2) - 2;
  const left = columnLines(discovery, 'one', selection, cursor.column === 0, cursor.index, colWidth);
  const right = columnLines(discovery, 'two', selection, cursor.column === 1, cursor.index, colWidth);

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
  const leadActive = cursor.column === 2;
  lines.push([
    span(' '.repeat(INDENT)),
    span(`${glyphs.confer} coordinator: `, {
      color: leadActive ? theme.text.primary : theme.text.muted,
      bold: leadActive,
    }),
    span(`slot ${selection.leadSlot}`, {
      color: agentColor(selection.leadSlot),
      bold: true,
      inverse: leadActive,
    }),
    span('  (the UI keeps it secret)', { color: theme.text.faint }),
  ]);

  if (selectionError) {
    lines.push(BLANK);
    for (const line of wrapText(`${glyphs.fail} ${selectionError}`, w - INDENT)) {
      lines.push([span(' '.repeat(INDENT)), span(line, { color: theme.status.error })]);
    }
  }

  lines.push(BLANK);
  // The hint follows the focused control: slot columns equip, the
  // coordinator control is where the team actually starts.
  const hint =
    cursor.column === 2
      ? '↑↓ coordinator · enter start · ←→ back · esc defaults'
      : '↑↓ choose · enter equip · ←→ switch · esc defaults';
  lines.push([span(' '.repeat(INDENT)), span(hint, { color: theme.text.faint })]);
  return lines;
}

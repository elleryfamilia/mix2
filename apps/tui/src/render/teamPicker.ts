/**
 * The startup team picker (`selecting-team` phase): one column per slot
 * listing every discovered harness, then a continue button. The
 * coordinator is a described default (`c` swaps it), not a focus stop —
 * leaving the picker should read as "continue", not as one more setting.
 * The configured proposal arrives preselected; unavailable or ineligible
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
import { BLANK, Line, buildTile, span, spread, truncate, wrapText, zipTiles } from './lines.js';

const INDENT = 2;

/** Picker cursor: which control has focus and which row is under it. */
export interface TeamPickerCursor {
  /** 0 = slot one column, 1 = slot two column, 2 = continue button. */
  column: 0 | 1 | 2;
  index: number;
}

export interface TeamPickerSelection {
  one: string;
  two: string;
  leadSlot: SlotName;
  /** Consultations allowed per question ("turns"), 1..MAX_TURNS_LIMIT. */
  maxTurns: number;
}

/** "2 turns per question" / "1 turn per question". */
export function turnsLabel(maxTurns: number): string {
  return `${maxTurns} turn${maxTurns === 1 ? '' : 's'} per question`;
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
  // The why matters: this slot coordinates, and a teammate-only harness
  // can't — on the other slot the same entry is selectable. When the
  // harness *could* lead under the OS sandbox but it isn't available here,
  // say so — that's an actionable fix, not a permanent limit.
  if (slot === leadSlot && !entry.lead_eligible) {
    return entry.sandboxable_lead
      ? 'needs the OS sandbox to coordinate'
      : "teammate-only: can't coordinate";
  }
  return 'not eligible';
}

/** The picker's initial selection: the core's proposal. */
export function initialSelection(discovery: DiscoveryState): TeamPickerSelection {
  return {
    one: discovery.proposal.one,
    two: discovery.proposal.two,
    leadSlot: discovery.proposal.lead_slot,
    maxTurns: discovery.maxTurns,
  };
}

/** Where a harness sits in the picker's entry list (cursor seeding). */
export function entryIndexOf(discovery: DiscoveryState, harness: string): number {
  return Math.max(
    0,
    pickerEntries(discovery).findIndex((e) => e.harness === harness),
  );
}

/** One slot's harness list as a bordered tile: the border carries the
 * slot's identity color when focused (quiet otherwise), and the equipped
 * entry reads in the slot color — the `●` mark alone was too subtle. */
function slotTile(
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
  const bodyWidth = width - 4;
  const body: Line[] = [];
  entries.forEach((entry, i) => {
    const enabled = selectable(entry, slot, selection.leadSlot);
    const isChosen = entry.harness === chosen;
    const isCursor = active && i === cursorIndex;
    const marker = isCursor ? '›' : ' ';
    const chosenMark = isChosen ? ' ●' : '';
    const version = entry.version ? `  ${entry.version}` : '';
    const label = truncate(`${entry.harness}${version}`, Math.max(8, bodyWidth - 4));
    body.push([
      span(`${marker} `, { color: theme.agent.team, bold: true }),
      span(label, {
        color: !enabled
          ? theme.text.faint
          : isChosen
            ? agentColor(slot)
            : isCursor
              ? theme.text.primary
              : theme.text.secondary,
        bold: isCursor || (enabled && isChosen),
        inverse: isCursor,
      }),
      span(chosenMark, { color: agentColor(slot) }),
    ]);
    if (!enabled) {
      const reason = truncate(disabledLabel(entry, slot, selection.leadSlot), Math.max(8, bodyWidth - 2));
      body.push([span('  '), span(reason, { color: theme.text.faint })]);
    } else if (isChosen) {
      // Selection disclosures surface right where the choice is made —
      // nothing is passed silently. A sandbox-led coordinator states its
      // scope; other notes (e.g. a trust flag) still show.
      if (isLead && entry.sandbox_lead) {
        const note = truncate('leads via OS sandbox — project writes limited to .mix2/', Math.max(8, bodyWidth - 2));
        body.push([span('  '), span(note, { color: theme.text.faint })]);
      }
      if (entry.note) {
        const note = truncate(entry.note, Math.max(8, bodyWidth - 2));
        body.push([span('  '), span(note, { color: theme.text.faint })]);
      }
    }
  });
  return buildTile(
    {
      headerLeft: [
        span(agentGlyph(slot), { color: agentColor(slot) }),
        span(` slot ${slot}`, { color: agentColor(slot), bold: true }),
        ...(isLead ? [span(` ${glyphs.dot} coordinates`, { color: theme.text.faint })] : []),
      ],
      headerRight: [],
      body,
      borderColor: active ? agentColor(slot) : theme.border.bridge,
    },
    width,
  );
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
  const colWidth = stacked ? w - INDENT : Math.floor((w - INDENT - 3) / 2);
  const left = slotTile(discovery, 'one', selection, cursor.column === 0, cursor.index, colWidth);
  const right = slotTile(discovery, 'two', selection, cursor.column === 1, cursor.index, colWidth);

  if (stacked) {
    for (const line of left) lines.push([span(' '.repeat(INDENT)), ...line]);
    lines.push(BLANK);
    for (const line of right) lines.push([span(' '.repeat(INDENT)), ...line]);
  } else {
    lines.push(...zipTiles(left, right, 3));
  }

  lines.push(BLANK);
  // The coordinator is a description, not a control: the default is fine
  // almost always, so it never costs a focus stop — `c` swaps it.
  lines.push([
    span(' '.repeat(INDENT)),
    span(`${glyphs.confer} `, { color: theme.text.muted }),
    span(`slot ${selection.leadSlot}`, { color: agentColor(selection.leadSlot), bold: true }),
    span(' coordinates', { color: theme.text.muted }),
    span('  (press c to swap)', { color: theme.text.faint }),
  ]);
  // Same idiom for the budget: a described default with adjust keys, not
  // a focus stop. It persists with the team, and `/turns` edits it later.
  lines.push([
    span(' '.repeat(INDENT)),
    span(`${glyphs.consult} `, { color: theme.text.muted }),
    span(turnsLabel(selection.maxTurns), { color: theme.agent.team, bold: true }),
    span('  (press + / - to change)', { color: theme.text.faint }),
  ]);
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

  if (selectionError) {
    lines.push(BLANK);
    for (const line of wrapText(`${glyphs.fail} ${selectionError}`, w - INDENT)) {
      lines.push([span(' '.repeat(INDENT)), span(line, { color: theme.status.error })]);
    }
  }

  lines.push(BLANK);
  // The hint follows the focused control: slot columns equip, the
  // continue button is where the team actually starts.
  const hint =
    cursor.column === 2
      ? 'enter start · ←→ back · esc defaults'
      : '↑↓ choose · enter equip · ←→ switch · esc defaults';
  lines.push([span(' '.repeat(INDENT)), span(hint, { color: theme.text.faint })]);
  return lines;
}

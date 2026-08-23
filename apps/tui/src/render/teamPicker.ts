/**
 * The startup team picker (`selecting-team` phase): one column per slot
 * listing the detected CLIs, then a continue button. The coordinator is a
 * described default (`c` swaps it), not a focus stop — leaving the picker
 * should read as "continue", not as one more setting. The configured
 * proposal arrives preselected; undetected CLIs are hidden entirely (a
 * footer hint covers installing or configuring one), while detected-but-
 * blocked entries stay visible with their actionable reason. A team is
 * two different CLIs, enforced directionally: slot one chooses among
 * everything detected, and slot two's list simply omits slot one's pick —
 * taking slot two's CLI onto slot one swaps them rather than blocking.
 * The exception is a single-CLI machine, where the duplicate is the only
 * way to start and stays offered.
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
}

/** Harness choices for a slot: detected CLIs only, unique by harness name
 * (preferring an available entry when the same harness was probed at
 * several commands). Undetected candidates are hidden — the picker lists
 * what's actually on this machine, not the whole registry. */
export function pickerEntries(discovery: DiscoveryState): DiscoveredHarness[] {
  const byHarness = new Map<string, DiscoveredHarness>();
  for (const entry of discovery.harnesses) {
    const existing = byHarness.get(entry.harness);
    if (!existing || (!existing.available && entry.available)) {
      byHarness.set(entry.harness, entry);
    }
  }
  return [...byHarness.values()].filter((entry) => entry.available);
}

/** Whether an entry could ever back this slot: signed in and role-eligible.
 * (Availability is a given — undetected entries never reach the picker.) */
function eligible(entry: DiscoveredHarness, slot: SlotName, leadSlot: SlotName): boolean {
  if (!entry.available || entry.auth === 'unauthenticated') return false;
  return slot === leadSlot ? entry.lead_eligible : entry.teammate_eligible;
}

/** Whether an entry can be chosen for a slot: signed in and role-eligible.
 * (The two-different-CLIs rule lives in `slotEntries`, not here — slot
 * two's list already omits slot one's pick.) */
export function selectable(entry: DiscoveredHarness, slot: SlotName, leadSlot: SlotName): boolean {
  return eligible(entry, slot, leadSlot);
}

/** The list a slot offers. Slot one chooses among every detected CLI;
 * slot two omits slot one's pick — a team is two different CLIs — unless
 * nothing eligible would remain (single-CLI machines still start). */
export function slotEntries(
  discovery: DiscoveryState,
  slot: SlotName,
  selection: TeamPickerSelection,
): DiscoveredHarness[] {
  const entries = pickerEntries(discovery);
  if (slot === 'one') return entries;
  const rest = entries.filter((e) => e.harness !== selection.one);
  return rest.some((e) => eligible(e, 'two', selection.leadSlot)) ? rest : entries;
}

/** The next selection after equipping `harness` onto `slot`. Equipping
 * slot one with slot two's current pick swaps rather than duplicates:
 * slot two takes the outgoing slot-one CLI when it's eligible there,
 * else the first eligible remaining entry. Only a single-CLI machine
 * ends up with the same CLI on both slots. */
export function equipSelection(
  discovery: DiscoveryState,
  selection: TeamPickerSelection,
  slot: SlotName,
  harness: string,
): TeamPickerSelection {
  const next: TeamPickerSelection = { ...selection, [slot]: harness };
  if (slot !== 'one' || harness !== selection.two) return next;
  const entries = pickerEntries(discovery);
  const outgoing = entries.find(
    (e) => e.harness === selection.one && e.harness !== harness,
  );
  const fallback =
    outgoing && eligible(outgoing, 'two', next.leadSlot)
      ? outgoing
      : entries.find((e) => e.harness !== harness && eligible(e, 'two', next.leadSlot));
  if (fallback) next.two = fallback.harness;
  return next;
}

/** One-line status label for a disabled entry. */
function disabledLabel(entry: DiscoveredHarness, slot: SlotName, leadSlot: SlotName): string {
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

/** The picker's initial selection: the core's proposal, remapped onto a
 * detected CLI wherever the proposal names one that isn't offered by that
 * slot's list — a preselection hidden from the list would be
 * unexplainable. (This also moves slot two off a duplicate proposal.) */
export function initialSelection(discovery: DiscoveryState): TeamPickerSelection {
  const selection: TeamPickerSelection = {
    one: discovery.proposal.one,
    two: discovery.proposal.two,
    leadSlot: discovery.proposal.lead_slot,
  };
  for (const slot of ['one', 'two'] as const) {
    const entries = slotEntries(discovery, slot, selection);
    if (entries.some((e) => e.harness === selection[slot])) continue;
    const fallback = entries.find((e) => selectable(e, slot, selection.leadSlot));
    if (fallback) selection[slot] = fallback.harness;
  }
  return selection;
}

/** Where a harness sits in a slot's entry list (cursor seeding). */
export function entryIndexOf(entries: DiscoveredHarness[], harness: string): number {
  return Math.max(
    0,
    entries.findIndex((e) => e.harness === harness),
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
  const entries = slotEntries(discovery, slot, selection);
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
        span(' — detected CLIs', { color: theme.text.faint }),
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
  // Only detected CLIs are listed, so say how to grow the list — install
  // the CLI, or point mix2 at a custom command when it's off the PATH.
  for (const line of wrapText(
    'missing a CLI? install it and relaunch — or set its command in ~/.config/mix2/config.toml',
    w - INDENT,
  )) {
    lines.push([span(' '.repeat(INDENT)), span(line, { color: theme.text.faint })]);
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

import { describe, expect, it } from 'vitest';
import type { DiscoveredHarness } from '../ipc/protocol.js';
import type { DiscoveryState } from '../state/store.js';
import { displayWidth, lineText } from './lines.js';
import {
  equipSelection,
  initialSelection,
  pickerEntries,
  renderTeamPicker,
  selectable,
  slotEntries,
} from './teamPicker.js';

const caps = {
  teammate_read_only: 'enforced',
  lead_permission_scoping: 'enforced',
  instruction_injection: 'enforced',
} as const;

function harness(overrides: Partial<DiscoveredHarness> & { harness: 'claude' | 'codex' }): DiscoveredHarness {
  return {
    command: overrides.harness,
    version: '1.0.0',
    auth: 'authenticated',
    available: true,
    lead_eligible: true,
    teammate_eligible: true,
    sandboxable_lead: false,
    sandbox_lead: false,
    capabilities: caps,
    ...overrides,
  };
}

function discovery(overrides: Partial<DiscoveryState> = {}): DiscoveryState {
  return {
    harnesses: [harness({ harness: 'claude' }), harness({ harness: 'codex' })],
    proposal: { one: 'claude', two: 'codex', lead_slot: 'one' },
    auto: false,
    ...overrides,
  };
}

function frame(d: DiscoveryState, width = 100, error?: string): string[] {
  return renderTeamPicker(d, initialSelection(d), { column: 0, index: 0 }, width, error).map(
    lineText,
  );
}

describe('team picker', () => {
  it('shows both slot columns with the proposal preselected', () => {
    const lines = frame(discovery());
    const joined = lines.join('\n');
    expect(joined).toContain('pick your team');
    // The list is framed as what's on this machine, with a pointer to
    // growing it — install, or configure a custom command.
    expect(joined).toContain('detected CLIs');
    expect(joined).toContain('~/.config/mix2/config.toml');
    expect(joined).toContain('● slot one');
    expect(joined).toContain('○ slot two');
    // The slot lists render as bordered tiles.
    expect(joined).toContain('╭ ● slot one');
    expect(joined).toContain('╰');
    expect(joined).toContain('coordinates');
    // The coordinator is a description with a swap key, and leaving the
    // picker is a plain continue button — not a coordinator focus stop.
    expect(joined).toContain('slot one coordinates');
    expect(joined).toContain('(press c to swap)');
    expect(joined).toContain('continue');
    // The proposal's choices carry the chosen mark.
    expect(lines.some((l) => l.includes('claude') && l.includes('●'))).toBe(true);
  });

  it('dedupes multiple probes of one harness, preferring the available one', () => {
    const d = discovery({
      harnesses: [
        harness({ harness: 'claude', command: '/broken/claude', available: false, reason: 'not installed: x' }),
        harness({ harness: 'claude', command: 'claude' }),
        harness({ harness: 'codex' }),
      ],
    });
    const entries = pickerEntries(d);
    expect(entries).toHaveLength(2);
    expect(entries[0]!.available).toBe(true);
  });

  it('hides undetected CLIs entirely, remapping a proposal that named one', () => {
    const d = discovery({
      harnesses: [
        harness({ harness: 'claude' }),
        harness({
          harness: 'codex',
          available: false,
          reason: 'not installed: npm i -g @openai/codex',
        }),
      ],
    });
    // The undetected codex is neither an entry nor a rendered row.
    expect(pickerEntries(d).map((e) => e.harness)).toEqual(['claude']);
    const joined = frame(d).join('\n');
    expect(joined).not.toContain('codex');
    expect(joined).not.toContain('not installed');
    // The proposal named codex for slot two; the preselection lands on a
    // CLI that's actually listed instead of an invisible one.
    expect(initialSelection(d)).toEqual({ one: 'claude', two: 'claude', leadSlot: 'one' });
  });

  it('disables signed-out entries with their reasons', () => {
    const signedOut = discovery({
      harnesses: [
        harness({ harness: 'claude' }),
        harness({ harness: 'codex', auth: 'unauthenticated', reason: 'not signed in: run `codex login`' }),
      ],
    });
    expect(selectable(signedOut.harnesses[1]!, 'two', 'one')).toBe(false);
    expect(frame(signedOut).join('\n')).toContain('not signed in: run `codex login`');
  });

  it("slot one offers everything; slot two's list omits slot one's pick", () => {
    const d = discovery();
    const selection = initialSelection(d); // claude / codex
    // Slot one lists both CLIs, both selectable — including slot two's pick.
    expect(slotEntries(d, 'one', selection).map((e) => e.harness)).toEqual(['claude', 'codex']);
    expect(selectable(slotEntries(d, 'one', selection)[1]!, 'one', 'one')).toBe(true);
    // Slot two adapts: whatever slot one holds simply isn't offered.
    expect(slotEntries(d, 'two', selection).map((e) => e.harness)).toEqual(['codex']);
    const joined = renderTeamPicker(d, selection, { column: 1, index: 0 }, 100)
      .map(lineText)
      .join('\n');
    // claude renders once — in slot one's tile only.
    expect(joined.match(/claude/g)).toHaveLength(1);
  });

  it("equipping slot two's pick onto slot one swaps the slots", () => {
    const d = discovery();
    const selection = initialSelection(d); // claude / codex
    expect(equipSelection(d, selection, 'one', 'codex')).toEqual({
      one: 'codex',
      two: 'claude',
      leadSlot: 'one',
    });
    // Equipping an unclaimed CLI touches nothing else.
    expect(equipSelection(d, selection, 'one', 'claude')).toEqual(selection);
    expect(equipSelection(d, selection, 'two', 'codex')).toEqual(selection);
  });

  it('a single detected CLI stays offered to both slots', () => {
    const d = discovery({
      harnesses: [harness({ harness: 'claude' })],
      proposal: { one: 'claude', two: 'codex', lead_slot: 'one' },
    });
    const selection = initialSelection(d); // remaps slot two onto claude
    expect(selection.two).toBe('claude');
    // Filtering slot one's pick out would leave slot two empty — the
    // duplicate is the only way to start a team here, so it stays.
    expect(slotEntries(d, 'two', selection).map((e) => e.harness)).toEqual(['claude']);
    expect(selectable(slotEntries(d, 'two', selection)[0]!, 'two', 'one')).toBe(true);
  });

  it('teammate-only harnesses are ineligible for the lead slot only', () => {
    const teammateOnly = harness({ harness: 'codex', lead_eligible: false });
    expect(selectable(teammateOnly, 'one', 'one')).toBe(false);
    expect(selectable(teammateOnly, 'two', 'one')).toBe(true);
    const d = discovery({
      harnesses: [harness({ harness: 'claude' }), teammateOnly],
      proposal: { one: 'claude', two: 'codex', lead_slot: 'two' },
    });
    // With slot two leading, codex is now blocked there and says why.
    expect(frame(d).join('\n')).toContain("teammate-only: can't coordinate");
  });

  it('a sandbox-only harness shows the actionable reason, not a permanent block', () => {
    // sandboxable but not lead-eligible here (no engine) → the label points
    // at the fix rather than reading as a permanent limitation.
    const needsSandbox = harness({
      harness: 'codex',
      lead_eligible: false,
      sandboxable_lead: true,
    });
    const d = discovery({
      harnesses: [harness({ harness: 'claude' }), needsSandbox],
      proposal: { one: 'claude', two: 'codex', lead_slot: 'two' },
    });
    const joined = frame(d).join('\n');
    expect(joined).toContain('needs the OS sandbox to coordinate');
    expect(joined).not.toContain("teammate-only: can't coordinate");
  });

  it('a sandbox-led coordinator discloses its confined write scope', () => {
    const sandboxLed = harness({
      harness: 'codex',
      sandbox_lead: true,
      sandboxable_lead: true,
    });
    const d = discovery({
      harnesses: [harness({ harness: 'claude' }), sandboxLed],
      proposal: { one: 'claude', two: 'codex', lead_slot: 'two' },
    });
    // codex leads (slot two) via the sandbox → the disclosure shows.
    expect(frame(d).join('\n')).toContain('leads via OS sandbox');
  });

  it('shows a selection note on the chosen entry — disclosures are visible', () => {
    const d = discovery({
      harnesses: [
        harness({ harness: 'claude' }),
        harness({
          harness: 'codex',
          note: 'runs with --trust: picking it marks this workspace trusted',
        }),
      ],
    });
    // The note shows where the harness is chosen (slot two in the proposal).
    expect(frame(d).join('\n')).toContain('runs with --trust');
  });

  it('shows the core refusal and stays within the frame on narrow terminals', () => {
    const lines = frame(
      discovery(),
      60,
      'Codex — not signed in: run `codex login`',
    );
    const joined = lines.join('\n');
    expect(joined).toContain('not signed in');
    for (const line of lines) {
      expect(displayWidth(line)).toBeLessThanOrEqual(60);
    }
    // Stacked below the tile breakpoint: slot two starts after slot one ends.
    const oneIdx = lines.findIndex((l) => l.includes('slot one'));
    const twoIdx = lines.findIndex((l) => l.includes('slot two'));
    expect(oneIdx).toBeGreaterThanOrEqual(0);
    expect(twoIdx).toBeGreaterThan(oneIdx);
  });
});

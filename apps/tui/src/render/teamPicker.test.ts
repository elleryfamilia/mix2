import { describe, expect, it } from 'vitest';
import type { DiscoveredHarness } from '../ipc/protocol.js';
import type { DiscoveryState } from '../state/store.js';
import { displayWidth, lineText } from './lines.js';
import {
  initialSelection,
  pickerEntries,
  renderTeamPicker,
  selectable,
  takenFor,
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
    maxTurns: 2,
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
    // The budget reads the same way: a described default with adjust keys.
    expect(joined).toContain('↔ 2 turns per question');
    expect(joined).toContain('(press + / - to change)');
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

  it('disables unavailable and signed-out entries with their reasons', () => {
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
    expect(selectable(d.harnesses[1]!, 'two', 'one')).toBe(false);
    const joined = frame(d).join('\n');
    expect(joined).toContain('not installed: npm i -g @openai/codex');

    const signedOut = discovery({
      harnesses: [
        harness({ harness: 'claude' }),
        harness({ harness: 'codex', auth: 'unauthenticated', reason: 'not signed in: run `codex login`' }),
      ],
    });
    expect(selectable(signedOut.harnesses[1]!, 'two', 'one')).toBe(false);
    expect(frame(signedOut).join('\n')).toContain('not signed in: run `codex login`');
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

describe('turns label', () => {
  it('reads naturally for one and many', () => {
    const d = discovery({ maxTurns: 1 });
    const one = renderTeamPicker(d, initialSelection(d), { column: 0, index: 0 }, 100).map(lineText).join('\n');
    expect(one).toContain('1 turn per question');
    const many = renderTeamPicker(d, { ...initialSelection(d), maxTurns: 5 }, { column: 0, index: 0 }, 100)
      .map(lineText)
      .join('\n');
    expect(many).toContain('5 turns per question');
  });
});

describe('one harness per slot', () => {
  it('the other slot\'s pick is disabled with its reason', () => {
    const d = discovery();
    const selection = { ...initialSelection(d), one: 'codex', two: 'claude' };
    expect(takenFor(d, 'two', selection)).toBe('codex');
    // Slot one is picked first and picks freely.
    expect(takenFor(d, 'one', selection)).toBeUndefined();
    const lines = renderTeamPicker(d, selection, { column: 1, index: 0 }, 100).map(lineText);
    expect(lines.join('\n')).toContain('picked for slot one');
    expect(lines.join('\n')).not.toContain('picked for slot two');
  });

  it('stays on the menu when it is the only harness the slot could run', () => {
    const d = discovery({
      harnesses: [
        harness({ harness: 'claude' }),
        harness({ harness: 'codex', available: false, reason: 'not installed: x' }),
      ],
    });
    const selection = { ...initialSelection(d), one: 'claude', two: 'claude' };
    expect(takenFor(d, 'two', selection)).toBeUndefined();
    const joined = renderTeamPicker(d, selection, { column: 1, index: 0 }, 100).map(lineText).join('\n');
    expect(joined).not.toContain('picked for slot');
  });

  it('the turns row is a focus stop with its own hint', () => {
    const d = discovery();
    const joined = renderTeamPicker(d, initialSelection(d), { column: 2, index: 0 }, 100).map(lineText).join('\n');
    expect(joined).toContain('› ↔ 2 turns per question');
    expect(joined).toContain('↑↓ or + / - to change (1–20)');
    expect(joined).toContain('↑↓ change · enter continue');
  });
});

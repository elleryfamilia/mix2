import { describe, expect, it } from 'vitest';
import type { DiscoveredHarness } from '../ipc/protocol.js';
import type { DiscoveryState } from '../state/store.js';
import { displayWidth, lineText } from './lines.js';
import {
  initialSelection,
  pickerEntries,
  renderTeamPicker,
  selectable,
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
    expect(joined).toContain('● slot one');
    expect(joined).toContain('○ slot two');
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
    expect(frame(d).join('\n')).toContain('teammate-only for now');
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

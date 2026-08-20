import { describe, expect, it } from 'vitest';
import type { CoreEvent } from '../ipc/protocol.js';
import { initialState, reduce, type AppState } from '../state/store.js';
import { lineText } from './lines.js';
import { renderTeamPanel } from './teamPanel.js';

const T = 1_000_000;

function apply(state: AppState, event: CoreEvent, now = T): AppState {
  return reduce(state, { type: 'core-event', event, now });
}

const ready: CoreEvent = {
  type: 'ready',
  protocol: 2,
  session_id: 's1',
  one: { slot: 'one', harness: 'claude', name: 'Claude', available: true },
  two: { slot: 'two', harness: 'codex', name: 'Codex', available: true },
  lead_slot: 'one',
  cwd: '/repo',
  project: true,
};

function text(state: AppState, now = T): string[] {
  return renderTeamPanel(state, 100, now).map(lineText);
}

describe('team panel disagreement ledger', () => {
  it('renders the ledger while the turn is still live', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'q' }, T);
    s = apply(s, {
      type: 'consult.started',
      turn_id: 't1',
      slot: 'two',
      index: 1,
      max: 2,
      prompt: 'Should we use a GSI?',
    });
    s = apply(s, {
      type: 'disagreement.recorded',
      turn_id: 't1',
      stances: [
        { slot: 'one', position: 'Add a second GSI on user_id.', outcome: 'chosen' },
        { slot: 'two', position: 'Denormalize instead of indexing.', outcome: 'dropped' },
      ],
      resolution: 'Ship the GSI now, revisit denormalization later.',
      revision: 1,
    });
    const lines = text(s);
    const joined = lines.join('\n');
    expect(joined).toContain('△ disagreement');
    expect(joined).toContain('Add a second GSI on user_id.');
    expect(joined).toContain('← shipped');
    expect(joined).toContain('Denormalize instead of indexing.');
    expect(joined).toContain('→ set aside');
    expect(joined).toContain('◐ team');
    expect(joined).toContain('Ship the GSI now, revisit denormalization later.');
  });

  it('renders the ledger for a settled turn', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'q' }, T);
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'team',
      lead_slot: 'one',
      text: 'Done.',
      consultations: 1,
      duration_ms: 500,
      disagreement: {
        stances: [{ slot: 'one', position: 'Use retries.', outcome: 'chosen' }],
        resolution: 'Retry with backoff.',
      },
    });
    s = apply(s, { type: 'turn.completed', turn_id: 't1', duration_ms: 500, consultations: 1 });
    const lines = text(s);
    const joined = lines.join('\n');
    expect(joined).toContain('△ disagreement');
    expect(joined).toContain('Use retries.');
    expect(joined).toContain('← shipped');
    expect(joined).toContain('Retry with backoff.');
  });

  it('wraps a long position across lines instead of truncating it', () => {
    const longPosition =
      'this stance position rambles on at great length about tradeoffs benchmarks and edge cases far past what any single row could hold without truncation';
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'q' }, T);
    s = apply(s, {
      type: 'disagreement.recorded',
      turn_id: 't1',
      stances: [{ slot: 'one', position: longPosition, outcome: 'chosen' }],
      resolution: 'ok',
      revision: 1,
    });
    const lines = text(s);
    // The full position is present, unabridged, with no ellipsis truncation.
    expect(lines.some((l) => l.includes('…'))).toBe(false);
    const words = longPosition.split(' ');
    expect(lines.some((l) => l.includes(words.slice(0, 4).join(' ')))).toBe(true);
    expect(lines.some((l) => l.includes(words.slice(-4).join(' ')))).toBe(true);
    // It actually wraps: more than one line carries a fragment of the position.
    const positionLines = lines.filter((l) => words.some((w) => l.includes(w)));
    expect(positionLines.length).toBeGreaterThan(1);
  });

  it('renders the ledger even when the consult list is empty', () => {
    const s: AppState = {
      ...initialState,
      phase: 'ready',
      session: {
        sessionId: 's1',
        one: { slot: 'one', harness: 'claude', name: 'Claude', available: true },
        two: { slot: 'two', harness: 'codex', name: 'Codex', available: true },
        leadSlot: 'one',
        cwd: '/repo',
        project: true,
      },
      lastTurn: {
        id: 't1',
        durationMs: 100,
        consults: [],
        toolsCompleted: 0,
        outcome: 'completed',
        disagreement: {
          stances: [{ slot: 'one', position: 'Solo call, still logged.', outcome: 'chosen' }],
          resolution: 'Went with it.',
        },
      },
    };
    const lines = text(s);
    const joined = lines.join('\n');
    expect(joined).toContain('no consultations this run');
    expect(joined).toContain('△ disagreement');
    expect(joined).toContain('Solo call, still logged.');
  });

  it('renders no ledger when there is no disagreement record', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'q' }, T);
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'one',
      lead_slot: 'one',
      text: 'Sure.',
      consultations: 0,
      duration_ms: 100,
    });
    s = apply(s, { type: 'turn.completed', turn_id: 't1', duration_ms: 100, consultations: 0 });
    const lines = text(s);
    expect(lines.some((l) => l.includes('△ disagreement'))).toBe(false);
  });
});

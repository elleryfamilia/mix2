import { describe, expect, it } from 'vitest';
import type { CoreEvent, Disagreement } from '../ipc/protocol.js';
import {
  formatDuration,
  formatElapsed,
  initialState,
  reduce,
  type AppState,
} from './store.js';

const T = 1_000_000;

function apply(state: AppState, event: CoreEvent, now = T): AppState {
  return reduce(state, { type: 'core-event', event, now });
}

const ready: CoreEvent = {
  type: 'ready',
  protocol: 1,
  session_id: 's1',
  lead: { kind: 'claude', name: 'Claude', version: '2.1', available: true },
  teammate: { kind: 'codex', name: 'Codex', version: '0.146', available: true },
  cwd: '/repo',
};

function startedTurn(state = initialState): AppState {
  let s = apply(state, ready);
  s = apply(s, { type: 'message.user', turn_id: 't1', text: 'hi there' });
  return s;
}

describe('startup', () => {
  it('reaches ready with session info', () => {
    const s = apply(initialState, ready);
    expect(s.phase).toBe('ready');
    expect(s.session?.lead.kind).toBe('claude');
    expect(s.session?.teammate.available).toBe(true);
  });

  it('fatal event switches to fatal phase', () => {
    const s = apply(initialState, { type: 'fatal', message: 'lead unavailable' });
    expect(s.phase).toBe('fatal');
    expect(s.fatalMessage).toContain('lead unavailable');
  });

  it('core exit becomes fatal', () => {
    const s = reduce(apply(initialState, ready), { type: 'core-exited', code: 1 });
    expect(s.phase).toBe('fatal');
    expect(s.fatalMessage).toContain('exited unexpectedly');
  });

  it('core exit surfaces the stderr tail', () => {
    const s = reduce(apply(initialState, ready), {
      type: 'core-exited',
      code: 1,
      stderr: "/lib/x86_64-linux-gnu/libc.so.6: version `GLIBC_2.39' not found",
    });
    expect(s.fatalMessage).toContain('GLIBC_2.39');
  });
});

describe('single-agent turn', () => {
  it('user message opens a turn', () => {
    const s = startedTurn();
    expect(s.items).toEqual([{ kind: 'user', text: 'hi there' }]);
    expect(s.turn?.id).toBe('t1');
    expect(s.turn?.phase).toBe('working');
  });

  it('streams lead deltas', () => {
    let s = startedTurn();
    s = apply(s, { type: 'agent.text_delta', turn_id: 't1', agent: 'claude', role: 'lead', text: 'Hel' });
    s = apply(s, { type: 'agent.text_delta', turn_id: 't1', agent: 'claude', role: 'lead', text: 'lo' });
    expect(s.turn?.streamText).toBe('Hello');
  });

  it('tool start settles the open stream segment as interim text', () => {
    let s = startedTurn();
    s = apply(s, { type: 'agent.text_delta', turn_id: 't1', agent: 'claude', role: 'lead', text: 'Looking…' });
    s = apply(s, {
      type: 'agent.tool.started',
      turn_id: 't1',
      agent: 'claude',
      role: 'lead',
      name: 'Read',
      detail: 'src/db.ts',
    });
    expect(s.items.at(-1)).toEqual({ kind: 'interim', agent: 'claude', text: 'Looking…' });
    expect(s.turn?.streamText).toBe('');
    expect(s.turn?.tools).toHaveLength(1);
  });

  it('completes with a solo final message and summary', () => {
    let s = startedTurn();
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'claude',
      lead: 'claude',
      text: 'Hey. What are we working on?',
      consultations: 0,
      duration_ms: 900,
    });
    s = apply(s, { type: 'turn.completed', turn_id: 't1', duration_ms: 900, consultations: 0 });
    expect(s.turn).toBeUndefined();
    const final = s.items.at(-1);
    expect(final).toMatchObject({ kind: 'final', speaker: 'claude', consultations: 0 });
    expect(s.lastSummary).toEqual({ durationMs: 900, consultations: 0, disagreements: 0 });
  });
});

describe('consultation flow', () => {
  function consultingTurn(): AppState {
    let s = startedTurn();
    s = apply(s, {
      type: 'consult.started',
      turn_id: 't1',
      agent: 'codex',
      index: 1,
      max: 2,
      prompt: 'Independently evaluate DynamoDB.',
    });
    return s;
  }

  it('tracks consult lifecycle and team attribution', () => {
    let s = consultingTurn();
    expect(s.turn?.phase).toBe('consulting');
    expect(s.turn?.consults[0]).toMatchObject({ status: 'running', agent: 'codex', index: 1 });

    s = apply(s, {
      type: 'consult.completed',
      turn_id: 't1',
      agent: 'codex',
      index: 1,
      duration_ms: 8120,
      text: 'DynamoDB fits writes, not your joins.',
    });
    expect(s.turn?.consults[0]).toMatchObject({ status: 'done', durationMs: 8120 });

    s = apply(s, { type: 'lead.synthesizing', turn_id: 't1', agent: 'claude' });
    expect(s.turn?.phase).toBe('synthesizing');

    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'team',
      lead: 'claude',
      text: "I wouldn't replace Postgres wholesale.",
      consultations: 1,
      duration_ms: 20_000,
    });
    const final = s.items.at(-1);
    expect(final).toMatchObject({ kind: 'final', speaker: 'team' });
    // A trace pill precedes the final answer when consultation happened.
    expect(s.items.some((i) => i.kind === 'trace')).toBe(true);
  });

  it('teammate deltas and tools feed the live consultation tile', () => {
    let s = consultingTurn();
    s = apply(s, {
      type: 'agent.tool.started',
      turn_id: 't1',
      agent: 'codex',
      role: 'teammate',
      name: 'shell',
      detail: 'rg SessionManager',
    });
    s = apply(s, { type: 'agent.text_delta', turn_id: 't1', agent: 'codex', role: 'teammate', text: 'checking' });
    expect(s.turn?.consults[0]?.tools).toHaveLength(1);
    expect(s.turn?.consults[0]?.streamText).toBe('checking');
    // The lead's stream stays untouched.
    expect(s.turn?.streamText).toBe('');
  });

  it('failed consultation keeps the turn alive', () => {
    let s = consultingTurn();
    s = apply(s, {
      type: 'consult.failed',
      turn_id: 't1',
      agent: 'codex',
      index: 1,
      message: 'Codex is unavailable',
    });
    expect(s.turn).toBeDefined();
    expect(s.turn?.consults[0]).toMatchObject({ status: 'failed' });
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'claude',
      lead: 'claude',
      text: 'Continuing alone.',
      consultations: 0,
      duration_ms: 5000,
    });
    expect(s.items.at(-1)).toMatchObject({ kind: 'final', speaker: 'claude' });
  });
});

describe('failure and cancellation', () => {
  it('turn failure yields an error item and frees the composer', () => {
    let s = startedTurn();
    s = apply(s, { type: 'turn.failed', turn_id: 't1', message: 'usage limit reached' });
    expect(s.turn).toBeUndefined();
    expect(s.items.at(-1)).toMatchObject({ kind: 'error', text: 'usage limit reached' });
  });

  it('cancellation settles activity and marks cancelled', () => {
    let s = startedTurn();
    s = apply(s, {
      type: 'agent.tool.started',
      turn_id: 't1',
      agent: 'claude',
      role: 'lead',
      name: 'Read',
      detail: 'a.ts',
    });
    s = apply(s, { type: 'agent.tool.finished', turn_id: 't1', agent: 'claude', role: 'lead', name: 'Read' });
    s = apply(s, { type: 'turn.cancelled', turn_id: 't1' });
    expect(s.turn).toBeUndefined();
    expect(s.items.at(-1)).toEqual({ kind: 'cancelled' });
    expect(s.items.some((i) => i.kind === 'activity')).toBe(true);
    expect(s.lastTurn?.outcome).toBe('cancelled');
  });

  it('events for stale turns are ignored', () => {
    let s = startedTurn();
    s = apply(s, { type: 'agent.text_delta', turn_id: 'OLD', agent: 'claude', role: 'lead', text: 'x' });
    expect(s.turn?.streamText).toBe('');
  });
});

describe('disagreement', () => {
  const liveDisagreement = {
    stances: [{ agent: 'claude' as const, position: 'Use Postgres (live)', outcome: 'chosen' as const }],
    resolution: 'Leaning Postgres.',
  };

  const finalDisagreement: Disagreement = {
    stances: [
      { agent: 'claude', position: 'Use Postgres', outcome: 'chosen' },
      { agent: 'codex', position: 'Use DynamoDB', outcome: 'dropped' },
    ],
    resolution: 'Went with Postgres for join support.',
  };

  it('disagreement.recorded attaches to the live turn', () => {
    let s = startedTurn();
    s = apply(s, {
      type: 'disagreement.recorded',
      turn_id: 't1',
      stances: liveDisagreement.stances,
      resolution: liveDisagreement.resolution,
      revision: 1,
    });
    expect(s.turn?.disagreement).toEqual({ ...liveDisagreement, revision: 1 });
  });

  it('stale revision is ignored', () => {
    let s = startedTurn();
    s = apply(s, {
      type: 'disagreement.recorded',
      turn_id: 't1',
      stances: liveDisagreement.stances,
      resolution: 'rev 2 resolution',
      revision: 2,
    });
    s = apply(s, {
      type: 'disagreement.recorded',
      turn_id: 't1',
      stances: liveDisagreement.stances,
      resolution: 'rev 1 resolution (stale)',
      revision: 1,
    });
    expect(s.turn?.disagreement?.revision).toBe(2);
    expect(s.turn?.disagreement?.resolution).toBe('rev 2 resolution');
  });

  it('message.final payload lands on the final item and overwrites live state', () => {
    let s = startedTurn();
    s = apply(s, {
      type: 'disagreement.recorded',
      turn_id: 't1',
      stances: liveDisagreement.stances,
      resolution: liveDisagreement.resolution,
      revision: 1,
    });
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'team',
      lead: 'claude',
      text: 'Final answer.',
      consultations: 1,
      duration_ms: 5000,
      disagreement: finalDisagreement,
    });
    const final = s.items.at(-1);
    expect(final).toMatchObject({ kind: 'final', disagreement: finalDisagreement });
    expect(s.turn?.disagreement).toEqual({ ...finalDisagreement, revision: 1 });
  });

  it('turn.completed carries it into lastTurn and lastSummary.disagreements === 1', () => {
    let s = startedTurn();
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'claude',
      lead: 'claude',
      text: 'Final.',
      consultations: 0,
      duration_ms: 1000,
      disagreement: finalDisagreement,
    });
    s = apply(s, { type: 'turn.completed', turn_id: 't1', duration_ms: 1000, consultations: 0 });
    expect(s.lastTurn?.disagreement).toEqual(finalDisagreement);
    expect(s.lastSummary?.disagreements).toBe(1);
  });

  it('absent payload yields disagreements === 0', () => {
    let s = startedTurn();
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'claude',
      lead: 'claude',
      text: 'No disagreement here.',
      consultations: 0,
      duration_ms: 900,
    });
    s = apply(s, { type: 'turn.completed', turn_id: 't1', duration_ms: 900, consultations: 0 });
    expect(s.lastTurn?.disagreement).toBeUndefined();
    expect(s.lastSummary?.disagreements).toBe(0);
  });

  it('turn.cancelled clears it and lastTurn carries none', () => {
    let s = startedTurn();
    s = apply(s, {
      type: 'disagreement.recorded',
      turn_id: 't1',
      stances: liveDisagreement.stances,
      resolution: liveDisagreement.resolution,
      revision: 1,
    });
    s = apply(s, { type: 'turn.cancelled', turn_id: 't1' });
    expect(s.lastTurn?.disagreement).toBeUndefined();
  });

  it('turn.failed clears it and lastTurn carries none', () => {
    let s = startedTurn();
    s = apply(s, {
      type: 'disagreement.recorded',
      turn_id: 't1',
      stances: liveDisagreement.stances,
      resolution: liveDisagreement.resolution,
      revision: 1,
    });
    s = apply(s, { type: 'turn.failed', turn_id: 't1', message: 'usage limit reached' });
    expect(s.lastTurn?.disagreement).toBeUndefined();
  });
});

describe('formatting', () => {
  it('formats elapsed and durations', () => {
    expect(formatElapsed(48_000)).toBe('0:48');
    expect(formatElapsed(153_000)).toBe('2:33');
    expect(formatDuration(46_000)).toBe('46s');
    expect(formatDuration(153_000)).toBe('2:33');
  });
});

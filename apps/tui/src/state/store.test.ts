import { describe, expect, it } from 'vitest';
import type { AgentInfo, CoreEvent, Disagreement } from '../ipc/protocol.js';
import {
  formatDuration,
  formatElapsed,
  initialState,
  leadInfo,
  reduce,
  speakerLabel,
  teammateInfo,
  type AppState,
} from './store.js';

const T = 1_000_000;

function apply(state: AppState, event: CoreEvent, now = T): AppState {
  return reduce(state, { type: 'core-event', event, now });
}

const infoOne: AgentInfo = {
  slot: 'one',
  harness: 'claude',
  name: 'Claude',
  version: '2.1',
  auth: 'authenticated', available: true,
};
const infoTwo: AgentInfo = {
  slot: 'two',
  harness: 'codex',
  name: 'Codex',
  version: '0.146',
  auth: 'authenticated', available: true,
};

const ready: Extract<CoreEvent, { type: 'ready' }> = {
  type: 'ready',
  protocol: 2,
  session_id: 's1',
  one: infoOne,
  two: infoTwo,
  lead_slot: 'one',
  cwd: '/repo',
};

function startedTurn(state = initialState): AppState {
  let s = apply(state, ready);
  s = apply(s, { type: 'message.user', turn_id: 't1', text: 'hi there' });
  return s;
}

describe('startup', () => {
  it('reaches ready with slot-keyed session info', () => {
    const s = apply(initialState, ready);
    expect(s.phase).toBe('ready');
    expect(s.session?.leadSlot).toBe('one');
    expect(s.session?.one.harness).toBe('claude');
    expect(s.session?.two.available).toBe(true);
    expect(leadInfo(s.session!).name).toBe('Claude');
    expect(teammateInfo(s.session!).name).toBe('Codex');
  });

  it('reversed lead keeps the harness-to-slot mapping stable', () => {
    const s = apply(initialState, { ...ready, lead_slot: 'two' } as CoreEvent);
    expect(s.session?.leadSlot).toBe('two');
    expect(s.session?.one.harness).toBe('claude');
    expect(leadInfo(s.session!).name).toBe('Codex');
    expect(teammateInfo(s.session!).name).toBe('Claude');
    const turn = apply(s, { type: 'message.user', turn_id: 't1', text: 'hi' });
    expect(turn.turn?.leadSlot).toBe('two');
  });

  it('same-harness teams keep distinct slot identities', () => {
    const s = apply(initialState, {
      ...ready,
      one: { ...infoOne, harness: 'codex', name: 'Codex (one)' },
      two: { ...infoTwo, name: 'Codex (two)' },
      lead_slot: 'two',
    } as CoreEvent);
    expect(speakerLabel(s.session, 'one')).toBe('codex (one)');
    expect(leadInfo(s.session!).name).toBe('Codex (two)');
    expect(teammateInfo(s.session!).slot).toBe('one');
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
    s = apply(s, { type: 'agent.text_delta', turn_id: 't1', slot: 'one', role: 'lead', text: 'Hel' });
    s = apply(s, { type: 'agent.text_delta', turn_id: 't1', slot: 'one', role: 'lead', text: 'lo' });
    expect(s.turn?.streamText).toBe('Hello');
  });

  it('tool start settles the open stream segment as interim text', () => {
    let s = startedTurn();
    s = apply(s, { type: 'agent.text_delta', turn_id: 't1', slot: 'one', role: 'lead', text: 'Looking…' });
    s = apply(s, {
      type: 'agent.tool.started',
      turn_id: 't1',
      slot: 'one',
      role: 'lead',
      name: 'Read',
      detail: 'src/db.ts',
    });
    expect(s.items.at(-1)).toEqual({ kind: 'interim', slot: 'one', text: 'Looking…' });
    expect(s.turn?.streamText).toBe('');
    expect(s.turn?.tools).toHaveLength(1);
  });

  it('completes with a solo final message and summary', () => {
    let s = startedTurn();
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'one',
      lead_slot: 'one',
      text: 'Hey. What are we working on?',
      consultations: 0,
      duration_ms: 900,
    });
    s = apply(s, { type: 'turn.completed', turn_id: 't1', duration_ms: 900, consultations: 0 });
    expect(s.turn).toBeUndefined();
    const final = s.items.at(-1);
    expect(final).toMatchObject({ kind: 'final', speaker: 'one', consultations: 0 });
    expect(s.lastSummary).toEqual({ durationMs: 900, consultations: 0, disagreements: 0 });
  });
});

describe('consultation flow', () => {
  function consultingTurn(): AppState {
    let s = startedTurn();
    s = apply(s, {
      type: 'consult.started',
      turn_id: 't1',
      slot: 'two',
      index: 1,
      max: 2,
      prompt: 'Independently evaluate DynamoDB.',
    });
    return s;
  }

  it('tracks consult lifecycle and team attribution', () => {
    let s = consultingTurn();
    expect(s.turn?.phase).toBe('consulting');
    expect(s.turn?.consults[0]).toMatchObject({ status: 'running', slot: 'two', index: 1 });

    s = apply(s, {
      type: 'consult.completed',
      turn_id: 't1',
      slot: 'two',
      index: 1,
      duration_ms: 8120,
      text: 'DynamoDB fits writes, not your joins.',
    });
    expect(s.turn?.consults[0]).toMatchObject({ status: 'done', durationMs: 8120 });

    s = apply(s, { type: 'lead.synthesizing', turn_id: 't1', slot: 'one' });
    expect(s.turn?.phase).toBe('synthesizing');

    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'team',
      lead_slot: 'one',
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
      slot: 'two',
      role: 'teammate',
      name: 'shell',
      detail: 'rg SessionManager',
    });
    s = apply(s, { type: 'agent.text_delta', turn_id: 't1', slot: 'two', role: 'teammate', text: 'checking' });
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
      slot: 'two',
      index: 1,
      message: 'Codex is unavailable',
    });
    expect(s.turn).toBeDefined();
    expect(s.turn?.consults[0]).toMatchObject({ status: 'failed' });
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'one',
      lead_slot: 'one',
      text: 'Continuing alone.',
      consultations: 0,
      duration_ms: 5000,
    });
    expect(s.items.at(-1)).toMatchObject({ kind: 'final', speaker: 'one' });
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
      slot: 'one',
      role: 'lead',
      name: 'Read',
      detail: 'a.ts',
    });
    s = apply(s, { type: 'agent.tool.finished', turn_id: 't1', slot: 'one', role: 'lead', name: 'Read' });
    s = apply(s, { type: 'turn.cancelled', turn_id: 't1' });
    expect(s.turn).toBeUndefined();
    expect(s.items.at(-1)).toEqual({ kind: 'cancelled' });
    expect(s.items.some((i) => i.kind === 'activity')).toBe(true);
    expect(s.lastTurn?.outcome).toBe('cancelled');
  });

  it('events for stale turns are ignored', () => {
    let s = startedTurn();
    s = apply(s, { type: 'agent.text_delta', turn_id: 'OLD', slot: 'one', role: 'lead', text: 'x' });
    expect(s.turn?.streamText).toBe('');
  });
});

describe('disagreement', () => {
  const liveDisagreement = {
    stances: [{ slot: 'one' as const, position: 'Use Postgres (live)', outcome: 'chosen' as const }],
    resolution: 'Leaning Postgres.',
  };

  const finalDisagreement: Disagreement = {
    stances: [
      { slot: 'one', position: 'Use Postgres', outcome: 'chosen' },
      { slot: 'two', position: 'Use DynamoDB', outcome: 'dropped' },
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

    // Boundary: an equal revision with different content is also stale.
    s = apply(s, {
      type: 'disagreement.recorded',
      turn_id: 't1',
      stances: liveDisagreement.stances,
      resolution: 'rev 2 resolution (duplicate, different content)',
      revision: 2,
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
      lead_slot: 'one',
      text: 'Final answer.',
      consultations: 1,
      duration_ms: 5000,
      disagreement: finalDisagreement,
    });
    const final = s.items.at(-1);
    expect(final).toMatchObject({ kind: 'final', disagreement: finalDisagreement });
    expect(s.turn?.disagreement).toEqual({ ...finalDisagreement, revision: 1 });
  });

  it('message.final without a payload clears live disagreement state', () => {
    let s = startedTurn();
    s = apply(s, {
      type: 'disagreement.recorded',
      turn_id: 't1',
      stances: liveDisagreement.stances,
      resolution: liveDisagreement.resolution,
      revision: 1,
    });
    expect(s.turn?.disagreement).toBeDefined();
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'one',
      lead_slot: 'one',
      text: 'Never mind, no disagreement after all.',
      consultations: 0,
      duration_ms: 1000,
    });
    expect(s.turn?.disagreement).toBeUndefined();
    const final = s.items.at(-1);
    expect(final).toMatchObject({ kind: 'final' });
    expect((final as { disagreement?: Disagreement }).disagreement).toBeUndefined();
    s = apply(s, { type: 'turn.completed', turn_id: 't1', duration_ms: 1000, consultations: 0 });
    expect(s.lastSummary?.disagreements).toBe(0);
  });

  it('turn.completed carries it into lastTurn and lastSummary.disagreements === 1', () => {
    let s = startedTurn();
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'one',
      lead_slot: 'one',
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
      speaker: 'one',
      lead_slot: 'one',
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

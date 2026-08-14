import { describe, expect, it } from 'vitest';
import type { CoreEvent } from '../ipc/protocol.js';
import { initialState, reduce, type AppState } from '../state/store.js';
import { renderConversation, type RenderContext } from './conversation.js';
import { lineText } from './lines.js';
import { renderTeamPanel } from './teamPanel.js';

const T = 1_000_000;

function apply(state: AppState, event: CoreEvent, now = T): AppState {
  return reduce(state, { type: 'core-event', event, now });
}

const ready: CoreEvent = {
  type: 'ready',
  protocol: 1,
  session_id: 's1',
  lead: { kind: 'claude', name: 'Claude', available: true },
  teammate: { kind: 'codex', name: 'Codex', available: true },
  cwd: '/repo',
};

const ctx: RenderContext = { width: 100, spinner: '⠸', now: T + 48_000 };

function text(state: AppState, context: RenderContext = ctx): string[] {
  return renderConversation(state, context).map(lineText);
}

describe('startup state', () => {
  it('shows the greeting before any conversation', () => {
    const lines = text(apply(initialState, ready));
    expect(lines).toEqual(['  How can we help?']);
  });
});

describe('single-agent response', () => {
  it('renders user message with prompt glyph and solo attribution chip', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'hi' });
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'claude',
      lead: 'claude',
      text: 'Hey. What are we working on?',
      consultations: 0,
      duration_ms: 800,
    });
    s = apply(s, { type: 'turn.completed', turn_id: 't1', duration_ms: 800, consultations: 0 });
    const lines = text(s);
    expect(lines[0]).toBe('  ❯ hi');
    expect(lines).toContain('   Claude ');
    expect(lines).toContain('  Hey. What are we working on?');
  });
});

describe('live activity', () => {
  it('shows working line with spinner, elapsed, and tool tree', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'look around' }, T);
    s = apply(s, {
      type: 'agent.tool.started',
      turn_id: 't1',
      agent: 'claude',
      role: 'lead',
      name: 'Read',
      detail: 'src/db/session.ts',
    });
    const lines = text(s);
    const working = lines.find((l) => l.includes('● Claude'));
    expect(working).toBeDefined();
    expect(working).toContain('investigating');
    expect(working).toContain('⠸ 0:48');
    expect(lines.some((l) => l.includes('└ read src/db/session.ts'))).toBe(true);
  });
});

describe('consultation activity', () => {
  function consulting(): AppState {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'postgres vs dynamo?' }, T);
    s = apply(s, {
      type: 'consult.started',
      turn_id: 't1',
      agent: 'codex',
      index: 1,
      max: 2,
      prompt: 'Independently evaluate DynamoDB for this repo.',
    });
    return s;
  }

  it('announces the consultation and renders parallel tiles', () => {
    const lines = text(consulting());
    expect(lines.some((l) => l.includes('↔ bringing in codex'))).toBe(true);
    expect(lines.some((l) => l.includes('○ codex — reviewing'))).toBe(true);
    expect(lines.some((l) => l.includes('╭'))).toBe(true);
  });

  it('stacks tiles on narrow terminals', () => {
    const narrow = { ...ctx, width: 80 };
    const lines = text(consulting(), narrow);
    // Stacked: lead tile ends before teammate tile starts.
    const claudeIdx = lines.findIndex((l) => l.includes('● claude'));
    const codexIdx = lines.findIndex((l) => l.includes('○ codex'));
    expect(claudeIdx).toBeGreaterThanOrEqual(0);
    expect(codexIdx).toBeGreaterThan(claudeIdx);
    for (const line of lines) {
      expect(line.length).toBeLessThanOrEqual(80);
    }
  });

  it('merges into a mauve exchange tile after completion', () => {
    let s = consulting();
    s = apply(s, {
      type: 'consult.completed',
      turn_id: 't1',
      agent: 'codex',
      index: 1,
      duration_ms: 31_000,
      text: 'GSI on user_id will not cover the analytics path.',
    });
    const lines = text(s);
    expect(lines.some((l) => l.includes('⇄ conferred'))).toBe(true);
    expect(lines.some((l) => l.includes(' Claude ') && l.includes(' Codex '))).toBe(true);
    expect(lines.some((l) => l.includes('Independently evaluate DynamoDB'))).toBe(true);
    expect(lines.some((l) => l.includes('GSI on user_id'))).toBe(true);
  });
});

describe('team response', () => {
  it('renders the Team chip with participants and a trace pill', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'decide!' }, T);
    s = apply(s, {
      type: 'consult.started',
      turn_id: 't1',
      agent: 'codex',
      index: 1,
      max: 2,
      prompt: 'evaluate',
    });
    s = apply(s, {
      type: 'consult.completed',
      turn_id: 't1',
      agent: 'codex',
      index: 1,
      duration_ms: 41_000,
      text: 'ship it',
    });
    s = apply(
      s,
      {
        type: 'message.final',
        turn_id: 't1',
        speaker: 'team',
        lead: 'claude',
        text: 'We would keep Postgres.\n\n1  Keep Postgres as source of truth',
        consultations: 1,
        duration_ms: 120_000,
      },
      T + 112_000,
    );
    const lines = text(s);
    expect(lines.some((l) => l.includes(' Team ') && l.includes('claude + codex'))).toBe(true);
    expect(lines.some((l) => l.includes('└ trace') && l.includes('⇄ 1 consultation'))).toBe(true);
    expect(lines).toContain('  We would keep Postgres.');
  });
});

describe('error state', () => {
  it('renders turn failure as an error line', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'x' });
    s = apply(s, { type: 'turn.failed', turn_id: 't1', message: 'usage limit reached' });
    const lines = text(s);
    expect(lines.some((l) => l.includes('× usage limit reached'))).toBe(true);
  });
});

describe('long responses and narrow terminals', () => {
  it('wraps long responses within the max reading width', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'explain' });
    const longText = 'word '.repeat(60).trim();
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'claude',
      lead: 'claude',
      text: longText,
      consultations: 0,
      duration_ms: 100,
    });
    const wide = { ...ctx, width: 160 };
    for (const line of text(s, wide)) {
      expect(line.length).toBeLessThanOrEqual(94);
    }
  });

  it('stays within 80 columns on narrow terminals', () => {
    let s = apply(initialState, ready);
    s = apply(s, {
      type: 'message.user',
      turn_id: 't1',
      text: 'a rather long question that will definitely need wrapping on a narrow terminal for sure',
    });
    const narrow = { ...ctx, width: 80 };
    for (const line of text(s, narrow)) {
      expect(line.length).toBeLessThanOrEqual(80);
    }
  });
});

describe('team panel', () => {
  it('lists participants, exchange, and the teammate response', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'q' }, T);
    s = apply(s, {
      type: 'consult.started',
      turn_id: 't1',
      agent: 'codex',
      index: 1,
      max: 2,
      prompt: 'Does a GSI on user_id cover the analytics path?',
    });
    s = apply(s, {
      type: 'consult.completed',
      turn_id: 't1',
      agent: 'codex',
      index: 1,
      duration_ms: 41_000,
      text: 'No — that path scans; you need a second GSI.',
    });
    const lines = renderTeamPanel(s, 100, T + 112_000).map(lineText);
    expect(lines.some((l) => l.includes('◐ team'))).toBe(true);
    expect(lines.some((l) => l.includes('● claude') && l.includes('lead'))).toBe(true);
    expect(lines.some((l) => l.includes('○ codex') && l.includes('teammate'))).toBe(true);
    expect(lines.some((l) => l.includes('Does a GSI on user_id'))).toBe(true);
    expect(lines.some((l) => l.includes('consultation 1 response'))).toBe(true);
  });

  it('shows the teammate as unavailable when it is', () => {
    const notReady: CoreEvent = {
      ...ready,
      teammate: { kind: 'codex', name: 'Codex', available: false, reason: 'not installed' },
    };
    const s = apply(initialState, notReady);
    const lines = renderTeamPanel(s, 100, T).map(lineText);
    expect(lines.some((l) => l.includes('unavailable — not installed'))).toBe(true);
  });
});

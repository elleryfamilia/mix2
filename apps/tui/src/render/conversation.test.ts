import { describe, expect, it } from 'vitest';
import type { CoreEvent, Stance } from '../ipc/protocol.js';
import { initialState, reduce, type AppState } from '../state/store.js';
import { renderConversation, type RenderContext } from './conversation.js';
import { displayWidth, lineText } from './lines.js';
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

const ctx: RenderContext = {
  width: 100,
  spinner: '⠸',
  now: T + 48_000,
  names: { one: 'Claude', two: 'Codex' },
};

function text(state: AppState, context: RenderContext = ctx): string[] {
  return renderConversation(state, context).map(lineText);
}

describe('startup state', () => {
  it('shows the project welcome with team framing', () => {
    const lines = text(apply(initialState, ready));
    expect(lines[0]).toBe('  How can we help?');
    const joined = lines.join('\n');
    expect(joined).toContain('one team');
    expect(joined).toContain('.mix2/');
    expect(joined).toContain('/help commands');
    expect(joined).not.toContain('No project detected');
  });

  it('adapts the welcome outside a software project', () => {
    const general = apply(initialState, { ...ready, project: false });
    const joined = text(general).join('\n');
    expect(joined).toContain('No project detected');
    expect(joined).toContain('business');
  });
});

describe('single-agent response', () => {
  it('renders user message with prompt glyph and solo attribution chip', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'hi' });
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'one',
      lead_slot: 'one',
      text: 'Hey. What are we working on?',
      consultations: 0,
      duration_ms: 800,
    });
    s = apply(s, { type: 'turn.completed', turn_id: 't1', duration_ms: 800, consultations: 0 });
    const lines = text(s);
    expect(lines[0]).toBe('  ❯ hi');
    expect(lines).toContain('   Team ');
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
      slot: 'one',
      role: 'lead',
      name: 'Read',
      detail: 'src/db/session.ts',
    });
    const lines = text(s);
    const working = lines.find((l) => l.includes('◐ Team'));
    expect(working).toBeDefined();
    expect(working).toContain('investigating');
    // Solo work is the team's, never a named agent's.
    expect(lines.some((l) => l.includes('● Claude'))).toBe(false);
    expect(working).toContain('0:48');
    expect(lines.some((l) => l.includes('└ read src/db/session.ts'))).toBe(true);
  });

  it('animates the team mark while busy', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'go' }, T);
    const rotating = { ...ctx, teamGlyph: '◓' };
    const lines = text(s, rotating);
    expect(lines.some((l) => l.includes('◓ Team'))).toBe(true);
    // Without a frame supplied, the mark stays the static ◐.
    expect(text(s).some((l) => l.includes('◐ Team'))).toBe(true);
  });
});

describe('narration', () => {
  const narration =
    'Codex is reading the doc independently; Claude is extracting the text so both work from the same source and nothing is lost.';

  it('shows the live stream under the mix2 chip in a hanging block', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'viable?' }, T);
    s = apply(s, { type: 'agent.text_delta', turn_id: 't1', slot: 'one', role: 'lead', text: narration });
    const lines = text(s, { ...ctx, width: 60 });
    const first = lines.findIndex((l) => l.includes(' mix2 '));
    expect(first).toBeGreaterThan(-1);
    expect(lines[first]).toMatch(/^ {2} mix2 {3}Codex is reading/);
    // Wrapped continuation hangs under the text, not under the chip.
    expect(lines[first + 1]).toMatch(/^ {10}\S/);
    // The narrator is the harness, never the team or a named agent.
    expect(lines.some((l) => l.includes(' Team ') && l.includes('Codex is reading'))).toBe(false);
  });

  it('settles narration as a mix2 block when a tool call interrupts it', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'viable?' }, T);
    s = apply(s, { type: 'agent.text_delta', turn_id: 't1', slot: 'one', role: 'lead', text: 'Codex is reading the doc.' });
    s = apply(s, {
      type: 'agent.tool.started',
      turn_id: 't1',
      slot: 'one',
      role: 'lead',
      name: 'Bash',
      detail: 'mix2-consult start',
    });
    const lines = text(s);
    const settled = lines.findIndex((l) => l.includes(' mix2 ') && l.includes('Codex is reading the doc.'));
    const working = lines.findIndex((l) => l.includes('◐ Team'));
    expect(settled).toBeGreaterThan(-1);
    // Settled narration sits above the live working block.
    expect(settled).toBeLessThan(working);
  });

  it('keeps the answer under the Team chip, not the narrator', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'viable?' }, T);
    s = apply(s, { type: 'agent.text_delta', turn_id: 't1', slot: 'one', role: 'lead', text: 'Codex is reading the doc.' });
    s = apply(s, {
      type: 'agent.tool.started',
      turn_id: 't1',
      slot: 'one',
      role: 'lead',
      name: 'Bash',
      detail: 'mix2-consult start',
    });
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'team',
      lead_slot: 'one',
      text: 'Our call: viable as a narrow wedge.',
      consultations: 1,
      duration_ms: 9000,
    });
    const lines = text(s);
    const narrator = lines.findIndex((l) => l.includes(' mix2 '));
    const team = lines.findIndex((l) => l.includes(' Team '));
    const answer = lines.findIndex((l) => l.includes('Our call'));
    expect(narrator).toBeGreaterThan(-1);
    expect(team).toBeGreaterThan(narrator);
    expect(answer).toBeGreaterThan(team);
    expect(lines[answer]).not.toContain(' mix2 ');
  });
});

describe('consultation activity', () => {
  function consulting(): AppState {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'postgres vs dynamo?' }, T);
    s = apply(s, {
      type: 'consult.started',
      turn_id: 't1',
      slot: 'two',
      index: 1,
      max: 2,
      prompt: 'Independently evaluate DynamoDB for this repo.',
    });
    return s;
  }

  it('announces the consultation and renders parallel tiles', () => {
    const lines = text(consulting());
    expect(lines.some((l) => l.includes('↔ second opinion'))).toBe(true);
    expect(lines.some((l) => l.includes('○ codex — reviewing'))).toBe(true);
    expect(lines.some((l) => l.includes('╭'))).toBe(true);
  });

  it('both live tiles carry the spinner in their headers', () => {
    const lines = text(consulting());
    const leadHeader = lines.find((l) => l.includes('● claude —'));
    const teammateHeader = lines.find((l) => l.includes('○ codex —'));
    expect(leadHeader).toContain(ctx.spinner);
    expect(teammateHeader).toContain(ctx.spinner);
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

  it('shows the lead researching in its tile during a concurrent consult', () => {
    let s = consulting();
    s = apply(s, {
      type: 'agent.tool.started',
      turn_id: 't1',
      slot: 'one',
      role: 'lead',
      name: 'Grep',
      detail: 'SessionManager',
    });
    const lines = text(s);
    expect(lines.some((l) => l.includes('● claude — researching'))).toBe(true);
    expect(lines.some((l) => l.includes('└ grep SessionManager'))).toBe(true);
  });

  it('merges into a mauve exchange tile after completion', () => {
    let s = consulting();
    s = apply(s, {
      type: 'consult.completed',
      turn_id: 't1',
      slot: 'two',
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

describe('second consultation', () => {
  it('renders the follow-up ask and both consult blocks in order', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'decide' }, T);
    s = apply(s, { type: 'consult.started', turn_id: 't1', slot: 'two', index: 1, max: 2, prompt: 'first ask' });
    s = apply(s, { type: 'consult.completed', turn_id: 't1', slot: 'two', index: 1, duration_ms: 20_000, text: 'first answer' });
    s = apply(s, { type: 'consult.started', turn_id: 't1', slot: 'two', index: 2, max: 2, prompt: 'challenge: are you sure?' });
    const lines = text(s);
    const first = lines.findIndex((l) => l.includes('↔ second opinion') && l.includes('1 of 2'));
    const conferred = lines.findIndex((l) => l.includes('⇄ conferred'));
    const second = lines.findIndex((l) => l.includes('↔ one more round') && l.includes('2 of 2'));
    const live = lines.findIndex((l) => l.includes('◐ Team — consulting'));
    expect(first).toBeGreaterThan(-1);
    expect(conferred).toBeGreaterThan(first);
    expect(second).toBeGreaterThan(conferred);
    expect(live).toBeGreaterThan(second);
  });
});

describe('synthesis stays visibly alive', () => {
  it('keeps the live team line below the conferred tile while reconciling', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'decide' }, T);
    s = apply(s, {
      type: 'consult.started',
      turn_id: 't1',
      slot: 'two',
      index: 1,
      max: 2,
      prompt: 'evaluate',
    });
    s = apply(s, {
      type: 'consult.completed',
      turn_id: 't1',
      slot: 'two',
      index: 1,
      duration_ms: 30_000,
      text: 'my assessment',
    });
    s = apply(s, { type: 'lead.synthesizing', turn_id: 't1', slot: 'one' });
    const lines = text(s);
    const tileBottom = lines.findIndex((l) => l.includes('╰'));
    const liveLine = lines.findIndex((l) => l.includes('◐ Team — reconciling'));
    expect(tileBottom).toBeGreaterThan(-1);
    expect(liveLine).toBeGreaterThan(tileBottom);
  });

  it('the live line trails the tiles during an active consult too', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'go' }, T);
    s = apply(s, {
      type: 'consult.started',
      turn_id: 't1',
      slot: 'two',
      index: 1,
      max: 2,
      prompt: 'evaluate',
    });
    const lines = text(s);
    const tileTop = lines.findIndex((l) => l.includes('╭'));
    const liveLine = lines.findIndex((l) => l.includes('◐ Team — consulting'));
    expect(tileTop).toBeGreaterThan(-1);
    expect(liveLine).toBeGreaterThan(tileTop);
  });
});

describe('team response', () => {
  it('renders the Team chip with participants and a trace pill', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'decide!' }, T);
    s = apply(s, {
      type: 'consult.started',
      turn_id: 't1',
      slot: 'two',
      index: 1,
      max: 2,
      prompt: 'evaluate',
    });
    s = apply(s, {
      type: 'consult.completed',
      turn_id: 't1',
      slot: 'two',
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
        lead_slot: 'one',
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

describe('scratchpad notice', () => {
  it('points at .mix2 files the team wrote', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'plan the refactor' }, T);
    s = apply(s, {
      type: 'agent.tool.started',
      turn_id: 't1',
      slot: 'one',
      role: 'lead',
      name: 'Write',
      detail: '.mix2/auth-refactor-plan.md',
    });
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'team',
      lead_slot: 'one',
      text: 'Plan written.',
      consultations: 1,
      duration_ms: 60_000,
    });
    const lines = text(s);
    expect(lines.some((l) => l.includes('▸ .mix2/auth-refactor-plan.md updated'))).toBe(true);
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
      speaker: 'one',
      lead_slot: 'one',
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

describe('stance block', () => {
  const resolution = "lead's call — ship now, file the rework";
  const stances: Stance[] = [
    { slot: 'one', position: 'cache compiled schema in-process', outcome: 'chosen' },
    { slot: 'two', position: 'move validation off the hot path', outcome: 'deferred' },
  ];

  function withDisagreement(width: number, overrideStances: Stance[]): string[] {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'decide!' }, T);
    s = apply(
      s,
      {
        type: 'message.final',
        turn_id: 't1',
        speaker: 'team',
        lead_slot: 'one',
        text: 'Shipping the cache now.',
        consultations: 1,
        duration_ms: 60_000,
        disagreement: { stances: overrideStances, resolution },
      },
      T + 60_000,
    );
    return text(s, { ...ctx, width });
  }

  it('renders a row per stance plus the team row after the answer body', () => {
    const lines = withDisagreement(100, stances);
    expect(lines.some((l) => l.includes('△ where we split'))).toBe(true);
    expect(
      lines.some(
        (l) => l.includes('● claude') && l.includes('cache compiled schema in-process') && l.includes('← shipped'),
      ),
    ).toBe(true);
    expect(
      lines.some(
        (l) => l.includes('○ codex') && l.includes('move validation off the hot path') && l.includes('→ follow-up'),
      ),
    ).toBe(true);
    expect(lines.some((l) => l.includes('◐ team') && l.includes(resolution))).toBe(true);
  });

  it('right-aligns the outcome arrow at the content edge', () => {
    const lines = withDisagreement(100, stances);
    const row = lines.find((l) => l.includes('← shipped'));
    expect(row).toBeDefined();
    // width 100 caps to MAX_CONTENT_WIDTH (92).
    expect(row).toHaveLength(92);
    expect(row!.endsWith('shipped')).toBe(true);
  });

  it('renders the dropped outcome as a right-aligned "set aside" arrow', () => {
    const lines = withDisagreement(100, [
      { slot: 'two', position: 'rewrite the retry logic from scratch', outcome: 'dropped' },
    ]);
    const row = lines.find((l) => l.includes('→ set aside'));
    expect(row).toBeDefined();
    expect(row).toContain('○ codex');
    expect(row).toContain('rewrite the retry logic from scratch');
    // width 100 caps to MAX_CONTENT_WIDTH (92).
    expect(row).toHaveLength(92);
    expect(row!.endsWith('set aside')).toBe(true);
  });

  it('truncates a long position with an ellipsis and never exceeds the frame', () => {
    const longPosition =
      'this stance position rambles on at great length about tradeoffs, benchmarks, and edge cases far past what any row could hold '.repeat(
        2,
      ).trim();
    for (const width of [80, 50]) {
      const lines = withDisagreement(width, [{ slot: 'one', position: longPosition, outcome: 'chosen' }]);
      const row = lines.find((l) => l.includes('claude') && l.includes('…'));
      expect(row).toBeDefined();
      expect(row!.length).toBeLessThanOrEqual(width);
    }
  });

  it('fits a CJK position within the frame at 50 columns using display width, not .length', () => {
    const lines = withDisagreement(50, [
      { slot: 'one', position: 'キャッシュを使う', outcome: 'chosen' },
    ]);
    const row = lines.find((l) => l.includes('● claude'));
    expect(row).toBeDefined();
    expect(displayWidth(row!)).toBeLessThanOrEqual(50);
  });

  it('renders no stance block when the final item has no disagreement', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'ok' }, T);
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'one',
      lead_slot: 'one',
      text: 'Sure thing.',
      consultations: 0,
      duration_ms: 100,
    });
    const lines = text(s);
    expect(lines.some((l) => l.includes('where we split'))).toBe(false);
  });
});

describe('reversed lead', () => {
  const reversed: CoreEvent = { ...ready, lead_slot: 'two' };

  it('keeps slot colors/glyphs stable while roles swap', () => {
    let s = apply(initialState, reversed);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'q' }, T);
    s = apply(s, {
      type: 'consult.started',
      turn_id: 't1',
      slot: 'one',
      index: 1,
      max: 2,
      prompt: 'evaluate',
    });
    const lines = text(s);
    // Codex (slot two) leads; Claude (slot one) reviews — glyphs follow the
    // slot, not the role.
    expect(lines.some((l) => l.includes('○ codex —'))).toBe(true);
    expect(lines.some((l) => l.includes('● claude — reviewing'))).toBe(true);
  });

  it('names the lead first in the team roster suffix', () => {
    let s = apply(initialState, reversed);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'q' }, T);
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'team',
      lead_slot: 'two',
      text: 'Answer.',
      consultations: 1,
      duration_ms: 1000,
    });
    const lines = text(s);
    expect(lines.some((l) => l.includes(' Team ') && l.includes('codex + claude'))).toBe(true);
  });
});

describe('same-harness team', () => {
  const twins: CoreEvent = {
    ...ready,
    one: { slot: 'one', harness: 'codex', name: 'Codex (one)', available: true },
    two: { slot: 'two', harness: 'codex', name: 'Codex (two)', available: true },
    lead_slot: 'one',
  };
  const twinCtx: RenderContext = {
    ...ctx,
    names: { one: 'Codex (one)', two: 'Codex (two)' },
  };

  it('renders both participants with distinct slot identities', () => {
    let s = apply(initialState, twins);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'q' }, T);
    s = apply(s, {
      type: 'consult.started',
      turn_id: 't1',
      slot: 'two',
      index: 1,
      max: 2,
      prompt: 'evaluate',
    });
    const lines = text(s, twinCtx);
    expect(lines.some((l) => l.includes('● codex (one) —'))).toBe(true);
    expect(lines.some((l) => l.includes('○ codex (two) — reviewing'))).toBe(true);
  });

  it('keys stances by slot so identical harnesses stay distinguishable', () => {
    let s = apply(initialState, twins);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'q' }, T);
    s = apply(s, {
      type: 'message.final',
      turn_id: 't1',
      speaker: 'team',
      lead_slot: 'one',
      text: 'Answer.',
      consultations: 1,
      duration_ms: 1000,
      disagreement: {
        stances: [
          { slot: 'one', position: 'plan A', outcome: 'chosen' },
          { slot: 'two', position: 'plan B', outcome: 'dropped' },
        ],
        resolution: 'went with A',
      },
    });
    const lines = text(s, twinCtx);
    expect(lines.some((l) => l.includes('● codex (one)') && l.includes('plan A'))).toBe(true);
    expect(lines.some((l) => l.includes('○ codex (two)') && l.includes('plan B'))).toBe(true);
  });
});

describe('team panel', () => {
  it('lists participants, exchange, and the teammate response', () => {
    let s = apply(initialState, ready);
    s = apply(s, { type: 'message.user', turn_id: 't1', text: 'q' }, T);
    s = apply(s, {
      type: 'consult.started',
      turn_id: 't1',
      slot: 'two',
      index: 1,
      max: 2,
      prompt: 'Does a GSI on user_id cover the analytics path?',
    });
    s = apply(s, {
      type: 'consult.completed',
      turn_id: 't1',
      slot: 'two',
      index: 1,
      duration_ms: 41_000,
      text: 'No — that path scans; you need a second GSI.',
    });
    const lines = renderTeamPanel(s, 100, T + 112_000).map(lineText);
    expect(lines.some((l) => l.includes('◐ team'))).toBe(true);
    expect(lines.some((l) => l.includes('● claude'))).toBe(true);
    expect(lines.some((l) => l.includes('○ codex'))).toBe(true);
    expect(lines.some((l) => l.includes('Does a GSI on user_id'))).toBe(true);
    expect(lines.some((l) => l.includes('consultation 1 response'))).toBe(true);
  });

  it('shows the teammate as unavailable when it is', () => {
    const notReady: CoreEvent = {
      ...ready,
      two: { slot: 'two', harness: 'codex', name: 'Codex', available: false, reason: 'not installed' },
    };
    const s = apply(initialState, notReady);
    const lines = renderTeamPanel(s, 100, T).map(lineText);
    expect(lines.some((l) => l.includes('offline — not installed'))).toBe(true);
  });
});

import { render } from 'ink-testing-library';
import React from 'react';
import { describe, expect, it, vi } from 'vitest';
import type { CoreClient } from '../ipc/client.js';
import type { CoreEvent } from '../ipc/protocol.js';
import { App } from './App.js';

interface Harness {
  lastFrame: () => string | undefined;
  stdin: { write: (data: string) => void };
  emit: (event: CoreEvent) => void;
  client: {
    submit: ReturnType<typeof vi.fn>;
    cancel: ReturnType<typeof vi.fn>;
    shutdown: ReturnType<typeof vi.fn>;
    send: ReturnType<typeof vi.fn>;
    selectTeam: ReturnType<typeof vi.fn>;
    restart: ReturnType<typeof vi.fn>;
  };
  unmount: () => void;
}

const ready: Extract<CoreEvent, { type: 'ready' }> = {
  type: 'ready',
  protocol: 2,
  session_id: 's1',
  one: {
    slot: 'one',
    harness: 'claude',
    name: 'Claude',
    version: '2.1.232',
    auth: 'authenticated', available: true,
    models: ['fable', 'opus', 'sonnet', 'haiku'],
  },
  two: {
    slot: 'two',
    harness: 'codex',
    name: 'Codex',
    version: '0.146.0',
    auth: 'authenticated', available: true,
    models: ['gpt-5.3-codex', 'gpt-5-codex'],
  },
  lead_slot: 'one',
  cwd: '/home/dev/src/acme',
};

function mount(): Harness {
  const client = {
    submit: vi.fn(),
    cancel: vi.fn(),
    shutdown: vi.fn(),
    send: vi.fn(),
    selectTeam: vi.fn(),
    restart: vi.fn(),
    start: vi.fn(),
  };
  let handlers: { onEvent: (e: CoreEvent) => void } = { onEvent: () => {} };
  const instance = render(
    <App
      client={client as unknown as CoreClient}
      bind={(h) => {
        handlers = h;
      }}
    />,
  );
  return {
    lastFrame: instance.lastFrame,
    stdin: instance.stdin,
    emit: (event) => handlers.onEvent(event),
    client,
    unmount: instance.unmount,
  };
}

async function tickReact(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 25));
}

/** Poll for content instead of sleeping blind: the picker needs two render
 * cycles (reducer, then the seeding effect), and fixed ticks get marginal
 * when the whole suite loads the machine. */
async function waitForFrame(h: Harness, needle: string): Promise<void> {
  const deadline = Date.now() + 3000;
  while (!h.lastFrame()?.includes(needle) && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

const pickerCaps = {
  teammate_read_only: 'enforced',
  lead_permission_scoping: 'enforced',
  instruction_injection: 'enforced',
} as const;

const discovered: Extract<CoreEvent, { type: 'harnesses.discovered' }> = {
  type: 'harnesses.discovered',
  harnesses: [
    {
      harness: 'claude',
      command: 'claude',
      version: '2.1.232',
      auth: 'authenticated',
      available: true,
      lead_eligible: true,
      teammate_eligible: true,
      capabilities: pickerCaps,
    },
    {
      harness: 'codex',
      command: 'codex',
      version: '0.146.0',
      auth: 'authenticated',
      available: true,
      lead_eligible: true,
      teammate_eligible: true,
      capabilities: pickerCaps,
    },
  ],
  proposal: { one: 'claude', two: 'codex', lead_slot: 'one' },
  auto: false,
};

describe('team picker', () => {
  it('pick–equip–advance: enter equips each slot, then starts from continue', async () => {
    const h = mount();
    await tickReact();
    h.emit(discovered);
    await waitForFrame(h, 'slot one');
    const frame = h.lastFrame()!;
    expect(frame).toContain('pick your team');
    expect(frame).toContain('slot one');
    expect(frame).toContain('enter equip');

    // Equip slot one (claude highlighted) → focus advances to slot two.
    h.stdin.write('\r');
    await tickReact();
    expect(h.client.selectTeam).not.toHaveBeenCalled();
    // Equip slot two (codex highlighted) → focus advances to continue.
    h.stdin.write('\r');
    await tickReact();
    expect(h.client.selectTeam).not.toHaveBeenCalled();
    expect(h.lastFrame()).toContain('enter start');
    // Enter on continue starts the team.
    h.stdin.write('\r');
    await tickReact();
    expect(h.client.selectTeam).toHaveBeenCalledWith('claude', 'codex', 'one');

    h.emit(ready);
    await tickReact();
    expect(h.lastFrame()).toContain('How can we help?');
    h.unmount();
  });

  it('arrows highlight without equipping; a same-harness team builds explicitly', async () => {
    const h = mount();
    await tickReact();
    h.emit(discovered);
    await waitForFrame(h, 'slot one');

    // `c` swaps the coordinator from anywhere — no focus stop needed.
    h.stdin.write('c');
    await tickReact();
    expect(h.lastFrame()).toContain('slot two coordinates');

    // ↓ only moves the highlight onto codex — nothing is equipped yet…
    h.stdin.write('\x1b[B');
    await tickReact();
    // …enter equips codex for slot one and advances to slot two, cursor
    // seeded on slot two's equipped harness (codex).
    h.stdin.write('\r');
    await tickReact();
    // Equip codex for slot two → continue; ↑ there is a no-op, not a
    // hidden coordinator toggle.
    h.stdin.write('\r');
    await tickReact();
    h.stdin.write('\x1b[A');
    await tickReact();
    expect(h.lastFrame()).toContain('slot two coordinates');

    h.stdin.write('\r');
    await tickReact();
    expect(h.client.selectTeam).toHaveBeenCalledWith('codex', 'codex', 'two');
    h.unmount();
  });

  it('escape starts the defaults without picking', async () => {
    const h = mount();
    await tickReact();
    h.emit(discovered);
    await waitForFrame(h, 'slot one');
    h.stdin.write('\x1b[B'); // wander first — esc still means "defaults"
    await tickReact();
    h.stdin.write('\x1b');
    await tickReact();
    expect(h.client.selectTeam).toHaveBeenCalledWith('claude', 'codex', 'one');
    h.unmount();
  });

  it('a core refusal shows in place and the picker can retry', async () => {
    const h = mount();
    await tickReact();
    h.emit(discovered);
    await waitForFrame(h, 'slot one');
    // Equip both slots and start.
    h.stdin.write('\r');
    await tickReact();
    h.stdin.write('\r');
    await tickReact();
    h.stdin.write('\r');
    await tickReact();
    expect(h.client.selectTeam).toHaveBeenCalledTimes(1);

    h.emit({ type: 'error', message: 'Codex — not signed in: run `codex login`' });
    await tickReact();
    const frame = h.lastFrame()!;
    expect(frame).toContain('pick your team');
    expect(frame).toContain('not signed in');

    // Focus stayed on continue: one enter retries.
    h.stdin.write('\r');
    await tickReact();
    expect(h.client.selectTeam).toHaveBeenCalledTimes(2);
    h.emit(ready);
    await tickReact();
    expect(h.lastFrame()).toContain('How can we help?');
    h.unmount();
  });

  it('a disabled entry shows its reason and enter refuses to equip it', async () => {
    const h = mount();
    await tickReact();
    h.emit({
      ...discovered,
      harnesses: [
        discovered.harnesses[0]!,
        {
          ...discovered.harnesses[1]!,
          available: false,
          reason: 'not installed: npm i -g @openai/codex',
        },
      ],
    });
    await waitForFrame(h, 'slot one');
    expect(h.lastFrame()).toContain('not installed: npm i -g @openai/codex');

    // Enter on the disabled codex row is a no-op — focus stays put.
    h.stdin.write('\x1b[B');
    await tickReact();
    h.stdin.write('\r');
    await tickReact();
    expect(h.client.selectTeam).not.toHaveBeenCalled();

    // Recover: equip claude on both slots, then start.
    h.stdin.write('\x1b[A');
    await tickReact();
    h.stdin.write('\r'); // equip slot one → advance (cursor lands on codex, slot two's pick)
    await tickReact();
    h.stdin.write('\x1b[A'); // highlight claude
    await tickReact();
    h.stdin.write('\r'); // equip slot two → continue
    await tickReact();
    h.stdin.write('\r');
    await tickReact();
    expect(h.client.selectTeam).toHaveBeenCalledWith('claude', 'claude', 'one');
    h.unmount();
  });

  it('/team relaunches the core with the picker for a fresh session', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    h.emit({ type: 'message.user', turn_id: 't1', text: 'old conversation' });
    h.emit({
      type: 'message.final',
      turn_id: 't1',
      speaker: 'one',
      lead_slot: 'one',
      text: 'old answer',
      consultations: 0,
      duration_ms: 100,
    });
    h.emit({ type: 'turn.completed', turn_id: 't1', duration_ms: 100, consultations: 0 });
    await tickReact();

    h.stdin.write('/team');
    await tickReact();
    h.stdin.write('\r');
    await tickReact();
    expect(h.client.restart).toHaveBeenCalledTimes(1);
    // Session state reset: the old conversation is gone, startup runs again.
    expect(h.lastFrame()).not.toContain('old answer');

    h.emit(discovered);
    await waitForFrame(h, 'slot one');
    expect(h.lastFrame()).toContain('pick your team');
    h.unmount();
  });

  it('/team refuses while a turn is running', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    h.emit({ type: 'message.user', turn_id: 't1', text: 'busy now' });
    await tickReact();
    h.stdin.write('/team');
    await tickReact();
    h.stdin.write('\r');
    await tickReact();
    expect(h.client.restart).not.toHaveBeenCalled();
    expect(h.lastFrame()).toContain('/team is unavailable while a turn is running');
    h.unmount();
  });
});

describe('App', () => {
  it('startup state shows identity, project, and readiness', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    await tickReact();
    const frame = h.lastFrame()!;
    expect(frame).toContain(' mix2 ');
    expect(frame).toContain('Claude ·');
    expect(frame).toContain('Codex');
    expect(frame).toContain('src/acme');
    expect(frame).toContain('How can we help?');
    expect(frame).toContain('ready');
    expect(frame).toContain('❯');
    h.unmount();
  });

  it('teammate unavailable startup still works', async () => {
    const h = mount();
    await tickReact();
    h.emit({
      ...ready,
      two: { slot: 'two', harness: 'codex', name: 'Codex', auth: 'authenticated', available: false, reason: 'not installed' },
    });
    await tickReact();
    expect(h.lastFrame()).toContain('Codex offline');
    h.unmount();
  });

  it('typing and submitting sends the message to the core', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    await tickReact();
    h.stdin.write('hi');
    await tickReact();
    expect(h.lastFrame()).toContain('❯ hi');
    h.stdin.write('\r');
    await tickReact();
    expect(h.client.submit).toHaveBeenCalledWith('t1', 'hi');
    h.unmount();
  });

  it('renders a full single-agent turn from events', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    h.emit({ type: 'message.user', turn_id: 't1', text: 'hi' });
    h.emit({
      type: 'message.final',
      turn_id: 't1',
      speaker: 'one',
      lead_slot: 'one',
      text: 'Hey. What are we working on?',
      consultations: 0,
      duration_ms: 900,
    });
    h.emit({ type: 'turn.completed', turn_id: 't1', duration_ms: 900, consultations: 0 });
    await tickReact();
    const frame = h.lastFrame()!;
    expect(frame).toContain('❯ hi');
    expect(frame).toContain(' Team');
    expect(frame).toContain('Hey. What are we working on?');
    expect(frame).toContain('done in 1s');
    h.unmount();
  });

  it('renders team consultation activity and attribution', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    h.emit({ type: 'message.user', turn_id: 't1', text: 'postgres or dynamo?' });
    h.emit({
      type: 'consult.started',
      turn_id: 't1',
      slot: 'two',
      index: 1,
      max: 2,
      prompt: 'Independently evaluate the migration.',
    });
    await tickReact();
    expect(h.lastFrame()).toContain('↔ second opinion');
    expect(h.lastFrame()).toContain('codex reviewing');

    h.emit({
      type: 'consult.completed',
      turn_id: 't1',
      slot: 'two',
      index: 1,
      duration_ms: 8_000,
      text: 'Keep Postgres for the joins.',
    });
    h.emit({ type: 'lead.synthesizing', turn_id: 't1', slot: 'one' });
    await tickReact();
    expect(h.lastFrame()).toContain('reconciling');

    h.emit({
      type: 'message.final',
      turn_id: 't1',
      speaker: 'team',
      lead_slot: 'one',
      text: "I wouldn't replace Postgres wholesale.",
      consultations: 1,
      duration_ms: 20_000,
    });
    h.emit({ type: 'turn.completed', turn_id: 't1', duration_ms: 20_000, consultations: 1 });
    await tickReact();
    const frame = h.lastFrame()!;
    expect(frame).toContain(' Team ');
    expect(frame).toContain("I wouldn't replace Postgres wholesale.");
    expect(frame).toContain('1 consultation');
    h.unmount();
  });

  it('done summary shows disagreement count when disagreements > 0', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    h.emit({ type: 'message.user', turn_id: 't1', text: 'should we use X?' });
    h.emit({
      type: 'consult.started',
      turn_id: 't1',
      slot: 'two',
      index: 1,
      max: 1,
      prompt: 'Evaluate X.',
    });
    h.emit({
      type: 'consult.completed',
      turn_id: 't1',
      slot: 'two',
      index: 1,
      duration_ms: 5_000,
      text: 'Use X.',
    });
    h.emit({ type: 'lead.synthesizing', turn_id: 't1', slot: 'one' });
    h.emit({
      type: 'message.final',
      turn_id: 't1',
      speaker: 'team',
      lead_slot: 'one',
      text: "I recommend against X.",
      consultations: 1,
      duration_ms: 15_000,
      disagreement: {
        stances: [{ slot: 'one', position: 'Do not use X', outcome: 'chosen' }],
        resolution: 'We decided against X.',
      },
    });
    h.emit({ type: 'turn.completed', turn_id: 't1', duration_ms: 15_000, consultations: 1 });
    await tickReact();
    const frame = h.lastFrame()!;
    expect(frame).toContain('1 consultation');
    expect(frame).toContain('△ 1 disagreement');
    h.unmount();
  });

  it('done summary shows nothing extra when disagreements === 0', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    h.emit({ type: 'message.user', turn_id: 't1', text: 'hi' });
    h.emit({
      type: 'message.final',
      turn_id: 't1',
      speaker: 'one',
      lead_slot: 'one',
      text: 'answer',
      consultations: 0,
      duration_ms: 500,
    });
    h.emit({ type: 'turn.completed', turn_id: 't1', duration_ms: 500, consultations: 0 });
    await tickReact();
    const frame = h.lastFrame()!;
    expect(frame).toContain('done in');
    expect(frame).not.toContain('△');
    expect(frame).not.toContain('disagreement');
    h.unmount();
  });

  it('ctrl+t toggles the team panel', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    await tickReact();
    h.stdin.write('\x14'); // ctrl+t
    await tickReact();
    expect(h.lastFrame()).toContain('◐ team');
    h.stdin.write('\x1b'); // esc closes
    await tickReact();
    expect(h.lastFrame()).not.toContain('◐ team');
    h.unmount();
  });

  it('esc cancels an active turn', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    h.emit({ type: 'message.user', turn_id: 't1', text: 'long task' });
    await tickReact();
    h.stdin.write('\x1b');
    await tickReact();
    expect(h.client.cancel).toHaveBeenCalledWith('t1');
    h.emit({ type: 'turn.cancelled', turn_id: 't1' });
    await tickReact();
    expect(h.lastFrame()).toContain('× cancelled');
    h.unmount();
  });

  it('shows errors without losing the composer', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    h.emit({ type: 'message.user', turn_id: 't1', text: 'x' });
    h.emit({ type: 'turn.failed', turn_id: 't1', message: 'usage limit reached' });
    await tickReact();
    const frame = h.lastFrame()!;
    expect(frame).toContain('× usage limit reached');
    expect(frame).toContain('ready');
    h.unmount();
  });

  it('fatal core error shows the failure screen', async () => {
    const h = mount();
    await tickReact();
    h.emit({ type: 'fatal', message: 'Claude (the selected lead) is unavailable' });
    await tickReact();
    const frame = h.lastFrame()!;
    expect(frame).toContain('fatal error');
    expect(frame).toContain('Claude (the selected lead) is unavailable');
    h.unmount();
  });

  it('/exit quits and /help lists commands', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    await tickReact();
    h.stdin.write('/help');
    await tickReact();
    // Slash hint appears while typing a command.
    expect(h.lastFrame()).toContain('/exit · /clear · /copy · /model · /team · /activity · /help');
    h.stdin.write('\r');
    await tickReact();
    expect(h.lastFrame()).toContain('commands  /exit');
    expect(h.client.submit).not.toHaveBeenCalled();

    h.stdin.write('/exit');
    await tickReact();
    h.stdin.write('\r');
    await tickReact();
    expect(h.client.shutdown).toHaveBeenCalled();
    h.unmount();
  });

  it('/clear empties the conversation', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    h.emit({ type: 'message.user', turn_id: 't1', text: 'hello' });
    h.emit({
      type: 'message.final',
      turn_id: 't1',
      speaker: 'one',
      lead_slot: 'one',
      text: 'answer',
      consultations: 0,
      duration_ms: 100,
    });
    h.emit({ type: 'turn.completed', turn_id: 't1', duration_ms: 100, consultations: 0 });
    await tickReact();
    expect(h.lastFrame()).toContain('❯ hello');
    h.stdin.write('/clear');
    await tickReact();
    h.stdin.write('\r');
    await tickReact();
    expect(h.lastFrame()).not.toContain('❯ hello');
    expect(h.lastFrame()).toContain('How can we help?');
    h.unmount();
  });

  it('renders markdown in final answers instead of raw markers', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    h.emit({ type: 'message.user', turn_id: 't1', text: 'q' });
    h.emit({
      type: 'message.final',
      turn_id: 't1',
      speaker: 'one',
      lead_slot: 'one',
      text: '## Short answer\n\nKeep the **Rust core**.\n\n1. First point\n2. Second point',
      consultations: 0,
      duration_ms: 100,
    });
    h.emit({ type: 'turn.completed', turn_id: 't1', duration_ms: 100, consultations: 0 });
    await tickReact();
    const frame = h.lastFrame()!;
    expect(frame).toContain('Short answer');
    expect(frame).not.toContain('##');
    expect(frame).not.toContain('**');
    expect(frame).toContain('Keep the Rust core.');
    expect(frame).toContain('1  First point');
    h.unmount();
  });

  it('frames the composer so input is distinct from output', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    await tickReact();
    const frame = h.lastFrame()!;
    expect(frame).toContain('╭');
    expect(frame).toContain('╰');
    expect(frame).toContain('❯');
    h.unmount();
  });

  it('ctrl+y and /copy copy the last answer', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    await tickReact();
    h.stdin.write('\x19'); // ctrl+y with nothing to copy
    await tickReact();
    expect(h.lastFrame()).toContain('nothing to copy yet');

    h.emit({ type: 'message.user', turn_id: 't1', text: 'q' });
    h.emit({
      type: 'message.final',
      turn_id: 't1',
      speaker: 'one',
      lead_slot: 'one',
      text: 'the answer',
      consultations: 0,
      duration_ms: 100,
    });
    h.emit({ type: 'turn.completed', turn_id: 't1', duration_ms: 100, consultations: 0 });
    await tickReact();
    h.stdin.write('\x19');
    await tickReact();
    expect(h.lastFrame()).toContain('answer copied to clipboard');
    h.unmount();
  });

  it('anchors the active prompt and jumps back on click', async () => {
    const { EventEmitter } = await import('node:events');
    const mouse = new EventEmitter();
    const client = { submit: vi.fn(), cancel: vi.fn(), shutdown: vi.fn(), send: vi.fn(), start: vi.fn() };
    let handlers: { onEvent: (e: CoreEvent) => void } = { onEvent: () => {} };
    const instance = render(
      <App
        client={client as unknown as CoreClient}
        bind={(h) => {
          handlers = h;
        }}
        mouse={mouse}
      />,
    );
    await tickReact();
    handlers.onEvent(ready);
    handlers.onEvent({ type: 'message.user', turn_id: 't1', text: 'my very important question' });
    handlers.onEvent({
      type: 'message.final',
      turn_id: 't1',
      speaker: 'one',
      lead_slot: 'one',
      text: Array.from({ length: 60 }, (_, i) => `answer line ${i + 1}`).join('\n\n'),
      consultations: 0,
      duration_ms: 100,
    });
    handlers.onEvent({ type: 'turn.completed', turn_id: 't1', duration_ms: 100, consultations: 0 });
    await tickReact();
    // Stuck to the bottom of a long answer: the prompt is off-screen, so
    // the sticky bar shows it with the jump affordance.
    let frame = instance.lastFrame()!;
    expect(frame).toContain('my very important question');
    expect(frame).toContain('↑ jump');

    // Clicking the bar (row 2) jumps back to the prompt.
    mouse.emit('event', { kind: 'down', x: 5, y: 2 });
    await tickReact();
    frame = instance.lastFrame()!;
    expect(frame).toContain('❯ my very important question');
    expect(frame).not.toContain('↑ jump');
    instance.unmount();
  });

  it('drag-selecting text copies it on release', async () => {
    const { EventEmitter } = await import('node:events');
    const mouse = new EventEmitter();
    const client = { submit: vi.fn(), cancel: vi.fn(), shutdown: vi.fn(), send: vi.fn(), start: vi.fn() };
    let handlers: { onEvent: (e: CoreEvent) => void } = { onEvent: () => {} };
    const instance = render(
      <App
        client={client as unknown as CoreClient}
        bind={(h) => {
          handlers = h;
        }}
        mouse={mouse}
      />,
    );
    await tickReact();
    handlers.onEvent(ready);
    await tickReact();
    // Drag across "How can we help?" (viewport starts at screen row 3).
    mouse.emit('event', { kind: 'down', x: 3, y: 3 });
    mouse.emit('event', { kind: 'drag', x: 10, y: 3 });
    mouse.emit('event', { kind: 'up', x: 10, y: 3 });
    await tickReact();
    expect(instance.lastFrame()).toContain('selection copied');
    instance.unmount();
  });

  it('/model opens the picker; enter applies the highlight and leaves', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    await tickReact();
    h.stdin.write('/model');
    await tickReact();
    h.stdin.write('\r');
    await tickReact();
    const frame = h.lastFrame()!;
    expect(frame).toContain('◐ models');
    expect(frame).toContain('provider default');
    expect(frame).toContain('sonnet');
    expect(frame).toContain('gpt-5.3-codex');

    // ↓↓↓ to "sonnet" (default, fable, opus, sonnet); enter applies and
    // closes — select-and-leave, matching the team picker.
    h.stdin.write('\x1b[B\x1b[B\x1b[B');
    await tickReact();
    h.stdin.write('\r');
    await tickReact();
    expect(h.client.send).toHaveBeenCalledWith({
      type: 'set_model',
      slot: 'one',
      model: 'sonnet',
    });
    expect(h.lastFrame()).not.toContain('◐ models');

    // The other slot is a fresh /model: → to the codex column, ↓↓ to
    // gpt-5-codex, enter applies and leaves again.
    h.stdin.write('/model');
    await tickReact();
    h.stdin.write('\r');
    await tickReact();
    expect(h.lastFrame()).toContain('◐ models');
    h.stdin.write('\x1b[C');
    await tickReact();
    h.stdin.write('\x1b[B\x1b[B');
    await tickReact();
    h.stdin.write('\r');
    await tickReact();
    expect(h.client.send).toHaveBeenCalledWith({
      type: 'set_model',
      slot: 'two',
      model: 'gpt-5-codex',
    });
    expect(h.lastFrame()).not.toContain('◐ models');
    h.unmount();
  });

  it('/model <agent> <name> still sets directly', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    await tickReact();
    h.stdin.write('/model claude sonnet');
    await tickReact();
    h.stdin.write('\r');
    await tickReact();
    expect(h.client.send).toHaveBeenCalledWith({
      type: 'set_model',
      slot: 'one',
      model: 'sonnet',
    });
    h.emit({ type: 'agent.model', slot: 'one', model: 'sonnet', source: 'selected' });
    await tickReact();
    expect(h.lastFrame()).toContain('claude model set to sonnet');
    h.unmount();
  });

  it('/model filter narrows the list and enter applies the match', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    await tickReact();
    h.stdin.write('/model');
    await tickReact();
    h.stdin.write('\r');
    await tickReact();
    expect(h.lastFrame()).toContain('◐ models');

    // Typing filters both columns; "son" leaves only sonnet for claude.
    h.stdin.write('son');
    await tickReact();
    const frame = h.lastFrame()!;
    expect(frame).toContain('filter: son');
    expect(frame).toContain('sonnet');
    expect(frame).not.toContain('provider default');
    h.stdin.write('\r');
    await tickReact();
    expect(h.client.send).toHaveBeenCalledWith({
      type: 'set_model',
      slot: 'one',
      model: 'sonnet',
    });
    // The apply closed the panel and cleared the filter with it.
    expect(h.lastFrame()).not.toContain('◐ models');
    h.unmount();
  });

  it('ctrl+j inserts a newline instead of submitting', async () => {
    const h = mount();
    await tickReact();
    h.emit(ready);
    await tickReact();
    h.stdin.write('one');
    h.stdin.write('\n'); // ctrl+j byte
    h.stdin.write('two');
    await tickReact();
    expect(h.client.submit).not.toHaveBeenCalled();
    expect(h.lastFrame()).toContain('two');
    h.unmount();
  });
});

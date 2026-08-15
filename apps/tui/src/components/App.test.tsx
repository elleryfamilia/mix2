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
  };
  unmount: () => void;
}

const ready: CoreEvent = {
  type: 'ready',
  protocol: 1,
  session_id: 's1',
  lead: { kind: 'claude', name: 'Claude', version: '2.1.232', available: true },
  teammate: { kind: 'codex', name: 'Codex', version: '0.146.0', available: true },
  cwd: '/home/dev/src/acme',
};

function mount(): Harness {
  const client = { submit: vi.fn(), cancel: vi.fn(), shutdown: vi.fn(), send: vi.fn(), start: vi.fn() };
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
      teammate: { kind: 'codex', name: 'Codex', available: false, reason: 'not installed' },
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
      speaker: 'claude',
      lead: 'claude',
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
      agent: 'codex',
      index: 1,
      max: 2,
      prompt: 'Independently evaluate the migration.',
    });
    await tickReact();
    expect(h.lastFrame()).toContain('↔ bringing in codex');
    expect(h.lastFrame()).toContain('codex reviewing');

    h.emit({
      type: 'consult.completed',
      turn_id: 't1',
      agent: 'codex',
      index: 1,
      duration_ms: 8_000,
      text: 'Keep Postgres for the joins.',
    });
    h.emit({ type: 'lead.synthesizing', turn_id: 't1', agent: 'claude' });
    await tickReact();
    expect(h.lastFrame()).toContain('reconciling');

    h.emit({
      type: 'message.final',
      turn_id: 't1',
      speaker: 'team',
      lead: 'claude',
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
    expect(h.lastFrame()).toContain('/exit · /clear · /copy · /team · /help');
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
      speaker: 'claude',
      lead: 'claude',
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
      speaker: 'claude',
      lead: 'claude',
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
      speaker: 'claude',
      lead: 'claude',
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
      speaker: 'claude',
      lead: 'claude',
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

import { chmodSync, mkdtempSync, utimesSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import type { CoreEvent } from './protocol.js';
import { CoreClient, newestFirst } from './client.js';

describe('core locator freshness', () => {
  it('prefers the most recently built binary over the fixed release/debug order', () => {
    // Regression: a stale target/release core from before a protocol bump
    // used to shadow the debug core `pnpm dev` just built, failing the
    // initialize handshake with an unknown-field serde error.
    const dir = mkdtempSync(path.join(tmpdir(), 'mix2-locator-'));
    const release = path.join(dir, 'release-core');
    const debug = path.join(dir, 'debug-core');
    writeFileSync(release, '#!/bin/sh\n');
    writeFileSync(debug, '#!/bin/sh\n');
    const old = new Date(Date.now() - 60_000);
    const fresh = new Date();
    utimesSync(release, old, old);
    utimesSync(debug, fresh, fresh);
    expect(newestFirst([release, debug])).toEqual([debug, release]);

    // And the other way round: a fresher release build wins again.
    utimesSync(release, new Date(Date.now() + 60_000), new Date(Date.now() + 60_000));
    expect(newestFirst([release, debug])).toEqual([release, debug]);

    // Missing paths sort last instead of throwing.
    const missing = path.join(dir, 'not-built');
    expect(newestFirst([missing, debug])).toEqual([debug, missing]);
  });
});

describe('CoreClient selection handshake', () => {
  it('surfaces the discovery report and leaves selection to the app', async () => {
    // A stand-in core that reports discovery awaiting selection, then
    // stays alive long enough for a reply to be possible.
    const script = path.join(tmpdir(), `mix2-fake-core-${process.pid}.sh`);
    writeFileSync(
      script,
      '#!/bin/sh\n' +
        `echo '{"type":"harnesses.discovered","harnesses":[],"proposal":{"one":"claude","two":"codex","lead_slot":"two"},"auto":false}'\n` +
        'sleep 1\n',
    );
    chmodSync(script, 0o755);

    const events: CoreEvent[] = [];
    const client = new CoreClient(
      { corePath: script },
      { onEvent: (e) => events.push(e), onExit: () => {} },
    );
    const send = vi.spyOn(client, 'send');
    client.start();
    // Poll instead of a fixed sleep: spawn latency varies under load.
    const deadline = Date.now() + 4000;
    while (
      !events.some((e) => e.type === 'harnesses.discovered') &&
      Date.now() < deadline
    ) {
      await new Promise((resolve) => setTimeout(resolve, 25));
    }

    expect(events.some((e) => e.type === 'harnesses.discovered')).toBe(true);
    // The picker (App) owns the choice; the client must not auto-confirm.
    expect(send).not.toHaveBeenCalledWith(
      expect.objectContaining({ type: 'select_team' }),
    );
    // The explicit selection helper carries the app's choice.
    client.selectTeam('codex', 'codex', 'one');
    expect(send).toHaveBeenCalledWith({
      type: 'select_team',
      one: 'codex',
      two: 'codex',
      lead_slot: 'one',
    });
    client.shutdown();
  });
});

describe('CoreClient restart (/team)', () => {
  it('relaunches with the picker forced and never misreports the old core exit', async () => {
    const script = path.join(tmpdir(), `mix2-restart-core-${process.pid}.sh`);
    writeFileSync(script, '#!/bin/sh\nsleep 2\n');
    chmodSync(script, 0o755);

    const exits: Array<number | null> = [];
    const client = new CoreClient(
      { corePath: script, interactive: true },
      { onEvent: () => {}, onExit: (code) => exits.push(code) },
    );
    const send = vi.spyOn(client, 'send');
    client.start();
    client.restart();
    await new Promise((resolve) => setTimeout(resolve, 300));

    const inits = send.mock.calls
      .map(([cmd]) => cmd)
      .filter((cmd) => cmd.type === 'initialize');
    expect(inits).toHaveLength(2);
    expect(inits[0]).toMatchObject({ pick_team: false });
    expect(inits[1]).toMatchObject({ pick_team: true, interactive: true });
    // The deliberately-killed first core must not surface as a crash.
    expect(exits).toEqual([]);
    client.shutdown();
  });
});

describe('CoreClient lifecycle', () => {
  it('shutdown is idempotent and never throws after the stream ends', async () => {
    // `cat` stands in for the core: reads stdin, echoes, exits on EOF.
    const client = new CoreClient(
      { corePath: '/bin/cat' },
      { onEvent: () => {}, onExit: () => {} },
    );
    client.start();
    client.shutdown();
    // Regression: the quit keybinding and the waitUntilExit cleanup both
    // call shutdown; the second used to write after end and crash.
    expect(() => client.shutdown()).not.toThrow();
    // Sends after shutdown are quietly dropped.
    expect(() => client.submit('t1', 'late message')).not.toThrow();
    await new Promise((resolve) => setTimeout(resolve, 50));
  });

  it('send before start is a no-op, not a crash', () => {
    const client = new CoreClient({}, { onEvent: () => {}, onExit: () => {} });
    expect(() => client.cancel('t1')).not.toThrow();
  });
});

describe('CoreClient selectTeam', () => {
  it('carries the turns budget only when given', () => {
    const client = new CoreClient({ corePath: '/bin/cat' }, { onEvent: () => {}, onExit: () => {} });
    const send = vi.spyOn(client, 'send');
    client.selectTeam('claude', 'codex', 'one');
    expect(send).toHaveBeenLastCalledWith({
      type: 'select_team',
      one: 'claude',
      two: 'codex',
      lead_slot: 'one',
    });
    client.selectTeam('claude', 'codex', 'one', 3);
    expect(send).toHaveBeenLastCalledWith({
      type: 'select_team',
      one: 'claude',
      two: 'codex',
      lead_slot: 'one',
      max_turns: 3,
    });
  });
});

import { chmodSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import type { CoreEvent } from './protocol.js';
import { CoreClient } from './client.js';

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

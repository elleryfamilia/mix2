import { describe, expect, it } from 'vitest';
import { CoreClient } from './client.js';

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

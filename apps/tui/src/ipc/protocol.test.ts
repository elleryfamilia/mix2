import { describe, expect, it } from 'vitest';
import { parseEventLine } from './protocol.js';

describe('parseEventLine', () => {
  it('parses known events', () => {
    const event = parseEventLine(
      '{"type":"consult.started","turn_id":"t1","agent":"codex","index":1,"max":2,"prompt":"evaluate"}',
    );
    expect(event).toMatchObject({ type: 'consult.started', agent: 'codex', index: 1 });
  });

  it('rejects malformed json without throwing', () => {
    expect(parseEventLine('not json {')).toBeNull();
    expect(parseEventLine('')).toBeNull();
  });

  it('rejects unknown event types (forward compatibility)', () => {
    expect(parseEventLine('{"type":"holo.deck","x":1}')).toBeNull();
  });

  it('rejects events with wrong field shapes', () => {
    expect(parseEventLine('{"type":"message.final","turn_id":1}')).toBeNull();
  });
});

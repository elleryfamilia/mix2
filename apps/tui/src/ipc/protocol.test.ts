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

  it('parses disagreement.recorded', () => {
    const event = parseEventLine(
      '{"type":"disagreement.recorded","turn_id":"t1","stances":[{"agent":"claude","position":"use approach A","outcome":"chosen"},{"agent":"codex","position":"use approach B","outcome":"dropped"}],"resolution":"went with approach A for simplicity","revision":1}',
    );
    expect(event).toMatchObject({
      type: 'disagreement.recorded',
      turn_id: 't1',
      revision: 1,
      stances: [
        { agent: 'claude', position: 'use approach A', outcome: 'chosen' },
        { agent: 'codex', position: 'use approach B', outcome: 'dropped' },
      ],
      resolution: 'went with approach A for simplicity',
    });
  });

  it('parses message.final with disagreement payload', () => {
    const event = parseEventLine(
      '{"type":"message.final","turn_id":"t1","speaker":"team","lead":"claude","text":"done","consultations":1,"duration_ms":10,"disagreement":{"stances":[{"agent":"claude","position":"A","outcome":"chosen"},{"agent":"codex","position":"B","outcome":"deferred"}],"resolution":"picked A"}}',
    );
    expect(event).toMatchObject({
      type: 'message.final',
      disagreement: {
        stances: [
          { agent: 'claude', position: 'A', outcome: 'chosen' },
          { agent: 'codex', position: 'B', outcome: 'deferred' },
        ],
        resolution: 'picked A',
      },
    });
  });

  it('parses message.final without the field (old core)', () => {
    const event = parseEventLine(
      '{"type":"message.final","turn_id":"t1","speaker":"team","lead":"claude","text":"done","consultations":1,"duration_ms":10}',
    );
    expect(event).toMatchObject({ type: 'message.final', turn_id: 't1' });
    expect(event && 'disagreement' in event ? event.disagreement : undefined).toBeUndefined();
  });
});

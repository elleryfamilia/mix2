import { describe, expect, it } from 'vitest';
import { parseEventLine } from './protocol.js';

describe('parseEventLine', () => {
  it('parses known events', () => {
    const event = parseEventLine(
      '{"type":"consult.started","turn_id":"t1","slot":"two","index":1,"max":2,"prompt":"evaluate"}',
    );
    expect(event).toMatchObject({ type: 'consult.started', slot: 'two', index: 1 });
  });

  it('parses ready with slot-keyed participants, same-harness included', () => {
    const event = parseEventLine(
      '{"type":"ready","protocol":3,"session_id":"s1","one":{"slot":"one","harness":"codex","name":"Codex (one)","auth":"authenticated","available":true},"two":{"slot":"two","harness":"codex","name":"Codex (two)","auth":"probe_failed","available":true},"lead_slot":"two","cwd":"/repo","project":true}',
    );
    expect(event).toMatchObject({
      type: 'ready',
      lead_slot: 'two',
      one: { slot: 'one', harness: 'codex', name: 'Codex (one)' },
      two: { slot: 'two', harness: 'codex', name: 'Codex (two)' },
    });
  });

  it('parses the discovery report', () => {
    const event = parseEventLine(
      '{"type":"harnesses.discovered","harnesses":[{"harness":"codex","command":"codex","version":"0.146.0","auth":"authenticated","available":true,"lead_eligible":true,"teammate_eligible":true,"capabilities":{"teammate_read_only":"enforced","lead_permission_scoping":"unverified","instruction_injection":"enforced"}}],"proposal":{"one":"claude","two":"codex","lead_slot":"one"},"auto":true}',
    );
    expect(event).toMatchObject({
      type: 'harnesses.discovered',
      auto: true,
      proposal: { one: 'claude', two: 'codex', lead_slot: 'one' },
      harnesses: [{ harness: 'codex', auth: 'authenticated', available: true }],
    });
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

  it('rejects v1 harness-keyed events (protocol break, not a silent remap)', () => {
    expect(
      parseEventLine(
        '{"type":"consult.started","turn_id":"t1","agent":"codex","index":1,"max":2}',
      ),
    ).toBeNull();
    expect(
      parseEventLine('{"type":"agent.model","agent":"claude","model":"sonnet","source":"selected"}'),
    ).toBeNull();
  });

  it('parses disagreement.recorded', () => {
    const event = parseEventLine(
      '{"type":"disagreement.recorded","turn_id":"t1","stances":[{"slot":"one","position":"use approach A","outcome":"chosen"},{"slot":"two","position":"use approach B","outcome":"dropped"}],"resolution":"went with approach A for simplicity","revision":1}',
    );
    expect(event).toMatchObject({
      type: 'disagreement.recorded',
      turn_id: 't1',
      revision: 1,
      stances: [
        { slot: 'one', position: 'use approach A', outcome: 'chosen' },
        { slot: 'two', position: 'use approach B', outcome: 'dropped' },
      ],
      resolution: 'went with approach A for simplicity',
    });
  });

  it('parses message.final with disagreement payload', () => {
    const event = parseEventLine(
      '{"type":"message.final","turn_id":"t1","speaker":"team","lead_slot":"one","text":"done","consultations":1,"duration_ms":10,"disagreement":{"stances":[{"slot":"one","position":"A","outcome":"chosen"},{"slot":"two","position":"B","outcome":"deferred"}],"resolution":"picked A"}}',
    );
    expect(event).toMatchObject({
      type: 'message.final',
      disagreement: {
        stances: [
          { slot: 'one', position: 'A', outcome: 'chosen' },
          { slot: 'two', position: 'B', outcome: 'deferred' },
        ],
        resolution: 'picked A',
      },
    });
  });

  it('parses message.final without the field (no disagreement)', () => {
    const event = parseEventLine(
      '{"type":"message.final","turn_id":"t1","speaker":"team","lead_slot":"one","text":"done","consultations":1,"duration_ms":10}',
    );
    expect(event).toMatchObject({ type: 'message.final', turn_id: 't1' });
    expect(event && 'disagreement' in event ? event.disagreement : undefined).toBeUndefined();
  });
});

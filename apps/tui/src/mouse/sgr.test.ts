import { describe, expect, it } from 'vitest';
import { extractMouseEvents } from './sgr.js';

describe('SGR mouse parsing', () => {
  it('extracts press, drag, release and strips them from input', () => {
    const { clean, events, rest } = extractMouseEvents(
      'a\x1b[<0;5;3Mb\x1b[<32;9;4Mc\x1b[<0;9;4md',
    );
    expect(clean).toBe('abcd');
    expect(rest).toBe('');
    expect(events).toEqual([
      { kind: 'down', x: 5, y: 3 },
      { kind: 'drag', x: 9, y: 4 },
      { kind: 'up', x: 9, y: 4 },
    ]);
  });

  it('classifies wheel events', () => {
    const { events } = extractMouseEvents('\x1b[<64;1;1M\x1b[<65;1;1M');
    expect(events.map((e) => e.kind)).toEqual(['wheel-up', 'wheel-down']);
  });

  it('holds back a sequence split across chunks', () => {
    const first = extractMouseEvents('hi\x1b[<0;12');
    expect(first.clean).toBe('hi');
    expect(first.events).toEqual([]);
    expect(first.rest).toBe('\x1b[<0;12');
    const second = extractMouseEvents(first.rest + ';7M!');
    expect(second.clean).toBe('!');
    expect(second.events).toEqual([{ kind: 'down', x: 12, y: 7 }]);
  });

  it('ignores right and middle button presses', () => {
    const { events, clean } = extractMouseEvents('\x1b[<2;3;3M\x1b[<1;3;3M');
    expect(events).toEqual([]);
    expect(clean).toBe('');
  });

  it('passes ordinary escape sequences through untouched', () => {
    const arrows = '\x1b[A\x1b[B';
    const { clean, events, rest } = extractMouseEvents(arrows);
    expect(clean).toBe(arrows);
    expect(events).toEqual([]);
    expect(rest).toBe('');
  });
});

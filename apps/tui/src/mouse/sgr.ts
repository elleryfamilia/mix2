/**
 * SGR mouse-reporting parser (DECSET 1002/1006).
 *
 * With mouse tracking enabled the terminal reports presses, drags,
 * releases, and wheel ticks as `\x1b[<b;x;yM` / `\x1b[<b;x;ym` sequences
 * on stdin. This module extracts those events from the byte stream and
 * returns the remaining "clean" input for the keyboard pipeline, holding
 * back incomplete sequences that straddle chunk boundaries.
 */

export interface MouseEvent {
  kind: 'down' | 'drag' | 'up' | 'wheel-up' | 'wheel-down';
  /** 1-based terminal column. */
  x: number;
  /** 1-based terminal row. */
  y: number;
}

const COMPLETE = /\x1b\[<(\d+);(\d+);(\d+)([Mm])/g;
/** A trailing fragment that could still become a mouse sequence. */
const PARTIAL_TAIL = /\x1b(\[(<(\d{0,3}(;\d{0,3}(;\d{0,3})?)?)?)?)?$/;

export function classify(button: number, isRelease: boolean): MouseEvent['kind'] | null {
  if (button === 64) return 'wheel-up';
  if (button === 65) return 'wheel-down';
  if (isRelease) return 'up';
  if ((button & 32) !== 0) return 'drag';
  if ((button & 3) === 0) return 'down'; // left button only
  return null; // right/middle press: ignore
}

export interface ExtractResult {
  /** Input with mouse sequences removed — safe for the keyboard pipeline. */
  clean: string;
  events: MouseEvent[];
  /** Incomplete trailing sequence to prepend to the next chunk. */
  rest: string;
}

export function extractMouseEvents(chunk: string): ExtractResult {
  const events: MouseEvent[] = [];
  let clean = '';
  let last = 0;
  for (const match of chunk.matchAll(COMPLETE)) {
    const index = match.index ?? 0;
    clean += chunk.slice(last, index);
    last = index + match[0].length;
    const button = Number(match[1]);
    const kind = classify(button, match[4] === 'm');
    if (kind) {
      events.push({ kind, x: Number(match[2]), y: Number(match[3]) });
    }
  }
  let tail = chunk.slice(last);
  let rest = '';
  const partial = tail.match(PARTIAL_TAIL);
  if (partial && partial[0].length > 0) {
    rest = partial[0];
    tail = tail.slice(0, tail.length - rest.length);
  }
  clean += tail;
  return { clean, events, rest };
}

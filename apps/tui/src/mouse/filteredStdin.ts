/**
 * A stdin stand-in for Ink that filters mouse-reporting sequences out of
 * the input stream. Keyboard bytes flow through to Ink untouched (Ink 7
 * consumes via the 'readable' event + read()); mouse events surface on a
 * separate emitter for the App to consume. Raw-mode control is proxied to
 * the real TTY.
 */
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { extractMouseEvents, type MouseEvent } from './sgr.js';

/** How long a trailing partial escape sequence may wait for its remainder
 * before it is treated as plain keyboard input (a bare Esc). A terminal
 * writes a whole mouse report at once, so a split is measured in
 * microseconds; readline's own escape timeout is 500ms. */
const PARTIAL_FLUSH_MS = 50;

export class FilteredStdin extends PassThrough {
  readonly mouse = new EventEmitter();
  readonly isTTY: boolean;
  private pending = '';
  private flushTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly real: NodeJS.ReadStream;

  constructor(real: NodeJS.ReadStream) {
    super();
    this.real = real;
    this.isTTY = real.isTTY ?? false;
    real.on('data', this.handleData);
    // A 'data' listener only starts the flow if the stream was never
    // explicitly paused. The startup update prompt (readline) pauses stdin
    // when it closes, so resume on purpose — otherwise the TUI's keyboard
    // is dead after answering it.
    real.resume();
  }

  private handleData = (chunk: Buffer | string): void => {
    if (this.flushTimer) {
      clearTimeout(this.flushTimer);
      this.flushTimer = null;
    }
    const text = this.pending + chunk.toString('utf8');
    const { clean, events, rest } = extractMouseEvents(text);
    this.pending = rest;
    for (const event of events) {
      this.mouse.emit('event', event satisfies MouseEvent);
    }
    if (clean.length > 0) {
      this.write(clean);
    }
    // A trailing partial sequence is usually the terminal splitting a mouse
    // report across writes — but a bare Escape keypress looks exactly like
    // its first byte. Give the rest a moment to arrive; if nothing does, it
    // was a keypress and Ink must see it now, not glued to the next key.
    if (this.pending.length > 0) {
      this.flushTimer = setTimeout(this.flushPending, PARTIAL_FLUSH_MS);
      this.flushTimer.unref?.();
    }
  };

  private flushPending = (): void => {
    this.flushTimer = null;
    if (this.pending.length > 0) {
      const held = this.pending;
      this.pending = '';
      this.write(held);
    }
  };

  setRawMode(enabled: boolean): this {
    if (this.real.isTTY) {
      this.real.setRawMode(enabled);
    }
    return this;
  }

  ref(): this {
    this.real.ref();
    return this;
  }

  unref(): this {
    this.real.unref();
    return this;
  }

  detach(): void {
    if (this.flushTimer) clearTimeout(this.flushTimer);
    this.real.off('data', this.handleData);
  }
}

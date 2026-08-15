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

export class FilteredStdin extends PassThrough {
  readonly mouse = new EventEmitter();
  readonly isTTY: boolean;
  private pending = '';
  private readonly real: NodeJS.ReadStream;

  constructor(real: NodeJS.ReadStream) {
    super();
    this.real = real;
    this.isTTY = real.isTTY ?? false;
    real.on('data', this.handleData);
  }

  private handleData = (chunk: Buffer | string): void => {
    const text = this.pending + chunk.toString('utf8');
    const { clean, events, rest } = extractMouseEvents(text);
    this.pending = rest;
    for (const event of events) {
      this.mouse.emit('event', event satisfies MouseEvent);
    }
    if (clean.length > 0) {
      this.write(clean);
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
    this.real.off('data', this.handleData);
  }
}

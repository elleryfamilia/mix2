import { PassThrough } from 'node:stream';
import { describe, expect, it } from 'vitest';
import { FilteredStdin } from './filteredStdin.js';

describe('FilteredStdin', () => {
  it('receives input even when the real stdin was explicitly paused before it attached', async () => {
    // readline pauses stdin when the startup update prompt closes; Ink must
    // still get keystrokes afterwards.
    const real = new PassThrough();
    real.pause();
    expect(real.readableFlowing).toBe(false);
    const filtered = new FilteredStdin(real as unknown as NodeJS.ReadStream);
    const received: string[] = [];
    filtered.on('data', (chunk: Buffer) => received.push(chunk.toString()));
    real.write('q');
    await new Promise((resolve) => setImmediate(resolve));
    expect(received.join('')).toBe('q');
  });

  it('routes mouse sequences to the mouse emitter and keeps keys in the stream', async () => {
    const real = new PassThrough();
    const filtered = new FilteredStdin(real as unknown as NodeJS.ReadStream);
    const keys: string[] = [];
    let mouse = 0;
    filtered.on('data', (chunk: Buffer) => keys.push(chunk.toString()));
    filtered.mouse.on('event', () => mouse++);
    real.write('a\x1b[<0;10;5Mb');
    await new Promise((resolve) => setImmediate(resolve));
    expect(keys.join('')).toBe('ab');
    expect(mouse).toBe(1);
  });
});

describe('FilteredStdin lone escape', () => {
  it('delivers a bare Escape keypress on its own instead of holding it for the next key', async () => {
    const real = new PassThrough();
    const filtered = new FilteredStdin(real as unknown as NodeJS.ReadStream);
    const chunks: string[] = [];
    filtered.on('data', (chunk: Buffer) => chunks.push(chunk.toString()));
    real.write('\x1b');
    await new Promise((resolve) => setTimeout(resolve, 120));
    expect(chunks).toEqual(['\x1b']);
    real.write('a');
    await new Promise((resolve) => setImmediate(resolve));
    expect(chunks).toEqual(['\x1b', 'a']);
  });

  it('still reassembles a mouse sequence that arrives split across two chunks', async () => {
    const real = new PassThrough();
    const filtered = new FilteredStdin(real as unknown as NodeJS.ReadStream);
    const keys: string[] = [];
    let mouse = 0;
    filtered.on('data', (chunk: Buffer) => keys.push(chunk.toString()));
    filtered.mouse.on('event', () => mouse++);
    real.write('\x1b[<0;10');
    real.write(';5Mz');
    await new Promise((resolve) => setTimeout(resolve, 120));
    expect(mouse).toBe(1);
    expect(keys.join('')).toBe('z');
  });
});

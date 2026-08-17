import { PassThrough } from 'node:stream';
import { describe, expect, it } from 'vitest';
import { askYesNo } from './prompt.js';

function setup() {
  const input = new PassThrough();
  const output = new PassThrough();
  let written = '';
  output.on('data', (chunk: Buffer) => {
    written += chunk.toString();
  });
  const answer = askYesNo({ input, output, question: 'Update now? [y/N] ' });
  return { input, answer, written: () => written };
}

describe('askYesNo', () => {
  it('shows the question', async () => {
    const { input, answer, written } = setup();
    input.write('n\n');
    await answer;
    expect(written()).toContain('Update now? [y/N] ');
  });

  it.each(['y', 'Y', 'yes', 'YES', '  y  '])('treats %j as yes', async (text) => {
    const { input, answer } = setup();
    input.write(`${text}\n`);
    await expect(answer).resolves.toBe('yes');
  });

  it.each(['', 'n', 'N', 'no', 'maybe', 'yeah'])('treats %j as no (the default)', async (text) => {
    const { input, answer } = setup();
    input.write(`${text}\n`);
    await expect(answer).resolves.toBe('no');
  });

  it('treats end of input (Ctrl+D, or Ctrl+C closing readline) as quit', async () => {
    const { input, answer } = setup();
    input.end();
    await expect(answer).resolves.toBe('quit');
  });
});

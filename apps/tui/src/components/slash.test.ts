import { describe, expect, it } from 'vitest';
import { slashCommandLength, splitForHighlight } from './slash.js';

describe('slashCommandLength', () => {
  it('matches complete known commands including the slash', () => {
    expect(slashCommandLength('/help')).toBe(5);
    expect(slashCommandLength('/q')).toBe(2);
    expect(slashCommandLength('/team panel please')).toBe(5);
    expect(slashCommandLength('/turns 3')).toBe(6);
  });

  it('returns 0 for partial, unknown, or non-command input', () => {
    expect(slashCommandLength('/he')).toBe(0);
    expect(slashCommandLength('/helpx')).toBe(0);
    expect(slashCommandLength('help')).toBe(0);
    expect(slashCommandLength('')).toBe(0);
    expect(slashCommandLength('say /help')).toBe(0);
  });
});

describe('splitForHighlight', () => {
  it('highlights the whole token when the segment contains it', () => {
    expect(splitForHighlight('/team now', 0, 5)).toEqual(['/team', ' now']);
  });

  it('carries the highlight across the cursor split', () => {
    // Cursor mid-command: before='/te' (offset 0), at='a' (offset 3),
    // after='m rest' (offset 4), command length 5.
    expect(splitForHighlight('/te', 0, 5)).toEqual(['/te', '']);
    expect(splitForHighlight('a', 3, 5)).toEqual(['a', '']);
    expect(splitForHighlight('m rest', 4, 5)).toEqual(['m', ' rest']);
  });

  it('highlights nothing past the command', () => {
    expect(splitForHighlight(' args', 5, 5)).toEqual(['', ' args']);
    expect(splitForHighlight('anything', 0, 0)).toEqual(['', 'anything']);
  });
});

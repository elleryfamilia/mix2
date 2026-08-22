import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  entryLine,
  fileHistoryStore,
  historyFilePath,
  parseHistory,
  HISTORY_LIMIT,
} from './history.js';

describe('parseHistory', () => {
  it('keeps only this project, oldest first', () => {
    const content =
      entryLine('/a', 'one') + entryLine('/b', 'other project') + entryLine('/a', 'two');
    expect(parseHistory(content, '/a')).toEqual(['one', 'two']);
  });

  it('collapses consecutive repeats and skips malformed lines', () => {
    const content =
      entryLine('/a', 'same') +
      entryLine('/a', 'same') +
      'not json at all\n' +
      '{"cwd":"/a"}\n' + // no text
      entryLine('/a', 'next');
    expect(parseHistory(content, '/a')).toEqual(['same', 'next']);
  });

  it('caps at the recall limit, keeping the newest', () => {
    const content = Array.from({ length: HISTORY_LIMIT + 5 }, (_, i) =>
      entryLine('/a', `p${i}`),
    ).join('');
    const parsed = parseHistory(content, '/a');
    expect(parsed).toHaveLength(HISTORY_LIMIT);
    expect(parsed[parsed.length - 1]).toBe(`p${HISTORY_LIMIT + 4}`);
    expect(parsed[0]).toBe('p5');
  });
});

describe('fileHistoryStore', () => {
  it('round-trips appends through the file, per project', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mix2-history-'));
    const file = path.join(dir, 'nested', 'history.jsonl');
    const a = fileHistoryStore('/proj/a', file);
    const b = fileHistoryStore('/proj/b', file);
    a.append('hello');
    b.append('unrelated');
    a.append('multi\nline prompt');
    expect(a.load()).toEqual(['hello', 'multi\nline prompt']);
    expect(b.load()).toEqual(['unrelated']);
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it('loads nothing when the file does not exist', () => {
    const store = fileHistoryStore('/proj', '/nonexistent/mix2/history.jsonl');
    expect(store.load()).toEqual([]);
  });
});

describe('historyFilePath', () => {
  it('respects XDG_STATE_HOME and falls back to ~/.local/state', () => {
    expect(historyFilePath({ XDG_STATE_HOME: '/xdg/state' })).toBe('/xdg/state/mix2/history.jsonl');
    expect(historyFilePath({})).toBe(path.join(os.homedir(), '.local', 'state', 'mix2', 'history.jsonl'));
  });
});

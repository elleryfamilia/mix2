import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  CHECK_INTERVAL_MS,
  FAILURE_BACKOFF_MS,
  decide,
  readCache,
  writeCache,
  type UpdateCache,
} from './check.js';

const HOUR = 60 * 60 * 1000;
const NOW = 1_800_000_000_000;

describe('decide', () => {
  it('checks and does not prompt when there is no cache', () => {
    expect(decide({ cache: undefined, now: NOW, current: '0.3.0' })).toEqual({
      needsCheck: true,
      shouldPrompt: false,
    });
  });

  it('does not re-check within the interval', () => {
    const cache: UpdateCache = { checkedAt: NOW - HOUR, latest: '0.3.0' };
    expect(decide({ cache, now: NOW, current: '0.3.0' }).needsCheck).toBe(false);
  });

  it('re-checks once the interval has elapsed', () => {
    const cache: UpdateCache = { checkedAt: NOW - CHECK_INTERVAL_MS - 1, latest: '0.3.0' };
    expect(decide({ cache, now: NOW, current: '0.3.0' }).needsCheck).toBe(true);
  });

  it('prompts when a newer version is known and the user was not asked recently', () => {
    const cache: UpdateCache = { checkedAt: NOW - HOUR, latest: '0.4.0' };
    expect(decide({ cache, now: NOW, current: '0.3.0' }).shouldPrompt).toBe(true);
  });

  it('does not prompt again within the interval after a prompt', () => {
    const cache: UpdateCache = { checkedAt: NOW - HOUR, latest: '0.4.0', promptedAt: NOW - HOUR };
    expect(decide({ cache, now: NOW, current: '0.3.0' }).shouldPrompt).toBe(false);
  });

  it('prompts again once the prompt interval has elapsed, even offline', () => {
    const cache: UpdateCache = {
      checkedAt: NOW - 3 * CHECK_INTERVAL_MS,
      latest: '0.4.0',
      promptedAt: NOW - CHECK_INTERVAL_MS - 1,
    };
    expect(decide({ cache, now: NOW, current: '0.3.0' })).toEqual({
      needsCheck: true,
      shouldPrompt: true,
    });
  });

  it('never prompts when the known latest is not newer', () => {
    expect(
      decide({ cache: { checkedAt: NOW, latest: '0.3.0' }, now: NOW, current: '0.3.0' }).shouldPrompt,
    ).toBe(false);
    expect(
      decide({ cache: { checkedAt: NOW, latest: '0.2.0' }, now: NOW, current: '0.3.0' }).shouldPrompt,
    ).toBe(false);
  });

  it('treats an unparseable cached version as unknown', () => {
    expect(
      decide({ cache: { checkedAt: NOW, latest: 'nope' }, now: NOW, current: '0.3.0' }).shouldPrompt,
    ).toBe(false);
  });

  it('treats a cache timestamp from the future as stale (a wrong clock must not silence checks)', () => {
    const cache: UpdateCache = { checkedAt: NOW + HOUR, latest: '0.3.0' };
    expect(decide({ cache, now: NOW, current: '0.3.0' }).needsCheck).toBe(true);
  });

  it('backs off after a failed check instead of retrying every launch', () => {
    const recent: UpdateCache = { failedAt: NOW - FAILURE_BACKOFF_MS + 1000 };
    expect(decide({ cache: recent, now: NOW, current: '0.3.0' }).needsCheck).toBe(false);
    const old: UpdateCache = { failedAt: NOW - FAILURE_BACKOFF_MS - 1000 };
    expect(decide({ cache: old, now: NOW, current: '0.3.0' }).needsCheck).toBe(true);
  });

  it('a first launch that just learned of a newer release prompts immediately', () => {
    // The flow re-runs decide() on the merged cache after a fetch.
    const merged: UpdateCache = { checkedAt: NOW, latest: '0.4.0' };
    expect(decide({ cache: merged, now: NOW, current: '0.3.0' })).toEqual({
      needsCheck: false,
      shouldPrompt: true,
    });
  });

  it('a stale successful check plus a recent failure does not re-check yet', () => {
    const cache: UpdateCache = {
      checkedAt: NOW - 2 * CHECK_INTERVAL_MS,
      latest: '0.4.0',
      failedAt: NOW - 1000,
    };
    expect(decide({ cache, now: NOW, current: '0.3.0' })).toEqual({
      needsCheck: false,
      shouldPrompt: true,
    });
  });
});

describe('cache file', () => {
  let dir: string;
  beforeEach(() => {
    dir = mkdtempSync(path.join(tmpdir(), 'mix2-update-cache-'));
  });
  afterEach(() => {
    rmSync(dir, { recursive: true, force: true });
  });

  it('round-trips through the file', () => {
    const file = path.join(dir, 'nested', 'update-check.json');
    const cache: UpdateCache = { checkedAt: NOW, latest: '0.4.0', promptedAt: NOW };
    writeCache(file, cache);
    expect(readCache(file)).toEqual(cache);
    expect(JSON.parse(readFileSync(file, 'utf8'))).toEqual(cache);
  });

  it('returns undefined for a missing file', () => {
    expect(readCache(path.join(dir, 'missing.json'))).toBeUndefined();
  });

  it('returns undefined for corrupt or wrongly-shaped content', () => {
    const file = path.join(dir, 'update-check.json');
    writeFileSync(file, '{not json');
    expect(readCache(file)).toBeUndefined();
    writeFileSync(file, JSON.stringify({ checkedAt: 'yesterday', latest: 1 }));
    expect(readCache(file)).toBeUndefined();
  });

  it('accepts a failure-only cache', () => {
    const file = path.join(dir, 'update-check.json');
    writeCache(file, { failedAt: NOW });
    expect(readCache(file)).toEqual({ failedAt: NOW });
  });

  it('leaves no temp file behind after writing', () => {
    const file = path.join(dir, 'update-check.json');
    writeCache(file, { checkedAt: NOW, latest: '0.4.0' });
    expect(readdirSync(dir)).toEqual(['update-check.json']);
  });

  it('never throws on an unwritable location', () => {
    expect(() => writeCache('/proc/definitely/not/writable/x.json', { checkedAt: NOW, latest: '1' })).not.toThrow();
  });
});

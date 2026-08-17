/**
 * Update-check bookkeeping: what we last learned about the newest release
 * and when we last bothered the user. Pure decision logic plus a tiny
 * JSON cache file; no network here.
 */
import { mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import path from 'node:path';
import { z } from 'zod';
import { compareVersions, parseVersion } from './semver.js';

/** At most one network check and at most one prompt per interval. */
export const CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;
/** After a failed check (offline, proxy, GitHub down) wait this long
 * before paying the network timeout again. */
export const FAILURE_BACKOFF_MS = 60 * 60 * 1000;

const cacheSchema = z.object({
  /** Epoch ms of the last successful check. */
  checkedAt: z.number().optional(),
  /** Newest release version seen at that check (no `v` prefix required). */
  latest: z.string().optional(),
  /** Epoch ms of the last time the user was asked to update. */
  promptedAt: z.number().optional(),
  /** Epoch ms of the last check that failed. */
  failedAt: z.number().optional(),
});
export type UpdateCache = z.infer<typeof cacheSchema>;

export interface Decision {
  /** The cached knowledge is missing or stale: ask GitHub again. */
  needsCheck: boolean;
  /** A newer release is known and the user hasn't been asked recently. */
  shouldPrompt: boolean;
}

export function decide(input: {
  cache: UpdateCache | undefined;
  now: number;
  current: string;
}): Decision {
  const { cache, now, current } = input;
  // A timestamp from the future (clock was wrong, restored backup) must not
  // silence checks until that date arrives: treat it as stale.
  const olderThan = (at: number | undefined, ms: number) =>
    at === undefined || at > now || now - at > ms;
  const needsCheck =
    olderThan(cache?.checkedAt, CHECK_INTERVAL_MS) &&
    olderThan(cache?.failedAt, FAILURE_BACKOFF_MS);
  const latest = cache?.latest;
  const newer =
    latest !== undefined && parseVersion(latest) !== undefined && compareVersions(latest, current) > 0;
  return { needsCheck, shouldPrompt: newer && olderThan(cache?.promptedAt, CHECK_INTERVAL_MS) };
}

/** `$XDG_CACHE_HOME/mix2/update-check.json`, default `~/.cache/mix2/…`. */
export function defaultCachePath(env: NodeJS.ProcessEnv = process.env): string {
  const xdg = env['XDG_CACHE_HOME'];
  const base = xdg && path.isAbsolute(xdg) ? xdg : path.join(homedir(), '.cache');
  return path.join(base, 'mix2', 'update-check.json');
}

export function readCache(file: string): UpdateCache | undefined {
  try {
    const parsed = cacheSchema.safeParse(JSON.parse(readFileSync(file, 'utf8')));
    return parsed.success ? parsed.data : undefined;
  } catch {
    return undefined;
  }
}

/** Best effort: a cache that cannot be written just means we check again.
 * Written to a temp file and renamed so a concurrent reader never sees a
 * half-written file. */
export function writeCache(file: string, cache: UpdateCache): void {
  const tmp = `${file}.${process.pid}.tmp`;
  try {
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(tmp, `${JSON.stringify(cache)}\n`);
    renameSync(tmp, file);
  } catch {
    try {
      rmSync(tmp, { force: true });
    } catch {
      // ignore
    }
  }
}

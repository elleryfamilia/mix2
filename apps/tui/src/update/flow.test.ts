import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { CHECK_INTERVAL_MS, readCache, writeCache, type UpdateCache } from './check.js';
import { offerUpdateAtStartup, runUpdateCommand, type FlowDeps } from './flow.js';
import type { Release } from './github.js';

const NOW = 1_800_000_000_000;
const HOUR = 60 * 60 * 1000;

let dir: string;
let installDir: string;
let cachePath: string;
beforeEach(() => {
  dir = mkdtempSync(path.join(tmpdir(), 'mix2-flow-'));
  installDir = path.join(dir, 'install');
  cachePath = path.join(dir, 'cache', 'update-check.json');
  mkdirSync(installDir);
  for (const f of ['mix2', 'mix2-core', 'mix2.bundle.mjs']) writeFileSync(path.join(installDir, f), '');
  chmodSync(path.join(installDir, 'mix2'), 0o755);
});
afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

function deps(overrides: Partial<FlowDeps> = {}) {
  const out: string[] = [];
  const err: string[] = [];
  const d: FlowDeps = {
    now: () => NOW,
    env: {},
    interactive: true,
    bundleDir: installDir,
    cachePath,
    fetchLatest: vi.fn(async (): Promise<Release | undefined> => ({ tag: 'v0.4.0', version: '0.4.0' })),
    ask: vi.fn(async () => 'no' as const),
    install: vi.fn(async () => ({ ok: true }) as const),
    verify: vi.fn(() => ({ ok: true }) as const),
    reexec: vi.fn(() => 0),
    out: (t) => out.push(t),
    err: (t) => err.push(t),
    debug: false,
    current: '0.3.0',
    ...overrides,
  };
  return { d, out: () => out.join(''), err: () => err.join('') };
}

describe('offerUpdateAtStartup', () => {
  it('does nothing when disabled by MIX2_NO_UPDATE_CHECK', async () => {
    const { d } = deps({ env: { MIX2_NO_UPDATE_CHECK: '1' } });
    await expect(offerUpdateAtStartup([], d)).resolves.toEqual({ action: 'continue' });
    expect(d.fetchLatest).not.toHaveBeenCalled();
    expect(readCache(cachePath)).toBeUndefined();
  });

  it('does nothing when not attached to a terminal', async () => {
    const { d } = deps({ interactive: false });
    await expect(offerUpdateAtStartup([], d)).resolves.toEqual({ action: 'continue' });
    expect(d.fetchLatest).not.toHaveBeenCalled();
  });

  it('does nothing for a source checkout', async () => {
    const { d } = deps({ bundleDir: dir });
    await expect(offerUpdateAtStartup([], d)).resolves.toEqual({ action: 'continue' });
    expect(d.fetchLatest).not.toHaveBeenCalled();
  });

  it('first launch: checks, learns of a newer release, and asks right away', async () => {
    const { d, out } = deps();
    await expect(offerUpdateAtStartup([], d)).resolves.toEqual({ action: 'continue' });
    expect(d.fetchLatest).toHaveBeenCalledWith(2000);
    expect(d.ask).toHaveBeenCalledTimes(1);
    expect(String(vi.mocked(d.ask).mock.calls[0]?.[0])).toContain('mix2 0.4.0 is available (you have 0.3.0)');
    expect(readCache(cachePath)).toEqual({ checkedAt: NOW, latest: '0.4.0', promptedAt: NOW });
    expect(d.install).not.toHaveBeenCalled();
    expect(out()).toBe('');
  });

  it('does not check or ask again within a day of asking', async () => {
    writeCache(cachePath, { checkedAt: NOW - HOUR, latest: '0.4.0', promptedAt: NOW - HOUR });
    const { d } = deps();
    await offerUpdateAtStartup([], d);
    expect(d.fetchLatest).not.toHaveBeenCalled();
    expect(d.ask).not.toHaveBeenCalled();
  });

  it('asks again the next day', async () => {
    writeCache(cachePath, {
      checkedAt: NOW - CHECK_INTERVAL_MS - 1,
      latest: '0.4.0',
      promptedAt: NOW - CHECK_INTERVAL_MS - 1,
    });
    const { d } = deps();
    await offerUpdateAtStartup([], d);
    expect(d.fetchLatest).toHaveBeenCalledTimes(1);
    expect(d.ask).toHaveBeenCalledTimes(1);
  });

  it('is silent when already up to date', async () => {
    const { d, out, err } = deps({ fetchLatest: async () => ({ tag: 'v0.3.0', version: '0.3.0' }) });
    await expect(offerUpdateAtStartup([], d)).resolves.toEqual({ action: 'continue' });
    expect(d.ask).not.toHaveBeenCalled();
    expect(out() + err()).toBe('');
    expect(readCache(cachePath)).toEqual({ checkedAt: NOW, latest: '0.3.0' });
  });

  it('records a failed check and backs off, without bothering the user', async () => {
    const { d, out, err } = deps({ fetchLatest: async () => undefined });
    await expect(offerUpdateAtStartup([], d)).resolves.toEqual({ action: 'continue' });
    expect(readCache(cachePath)).toEqual({ failedAt: NOW });
    expect(d.ask).not.toHaveBeenCalled();
    expect(out() + err()).toBe('');
    // A launch a minute later does not pay the network timeout again.
    const again = deps({ now: () => NOW + 60_000, fetchLatest: vi.fn(async () => undefined) });
    await offerUpdateAtStartup([], again.d);
    expect(again.d.fetchLatest).not.toHaveBeenCalled();
  });

  it('a failed check still prompts from stale knowledge of a newer release', async () => {
    writeCache(cachePath, { checkedAt: NOW - 3 * CHECK_INTERVAL_MS, latest: '0.4.0' });
    const { d } = deps({ fetchLatest: async () => undefined });
    await offerUpdateAtStartup([], d);
    expect(d.ask).toHaveBeenCalledTimes(1);
    expect(readCache(cachePath)).toMatchObject({ latest: '0.4.0', failedAt: NOW, promptedAt: NOW });
  });

  it('logs the reason for skipping when --debug is on', async () => {
    const { d, err } = deps({ interactive: false, debug: true });
    await offerUpdateAtStartup([], d);
    expect(err()).toContain('[update-check] not a terminal');
  });

  it('on yes: installs the pinned tag, verifies, relaunches with the same argv', async () => {
    const { d, out } = deps({ ask: async () => 'yes', reexec: vi.fn(() => 42) });
    await expect(offerUpdateAtStartup(['--lead', 'codex'], d)).resolves.toEqual({ action: 'exit', code: 42 });
    expect(d.install).toHaveBeenCalledWith('v0.4.0', installDir);
    expect(d.verify).toHaveBeenCalledWith(installDir, '0.4.0');
    expect(d.reexec).toHaveBeenCalledWith(installDir, ['--lead', 'codex']);
    expect(out()).toContain('✓ updated to 0.4.0 — starting mix2');
  });

  it('on yes with a failed install: keeps going on the old version when it is intact', async () => {
    const { d, err } = deps({
      ask: async () => 'yes',
      install: async () => ({ ok: false, reason: 'installer exited with status 1' }),
    });
    await expect(offerUpdateAtStartup([], d)).resolves.toEqual({ action: 'continue' });
    expect(d.reexec).not.toHaveBeenCalled();
    expect(err()).toContain('mix2 update failed: installer exited with status 1');
    expect(err()).toContain('continuing with mix2 0.3.0');
  });

  it('on yes with a failed verify: does not relaunch', async () => {
    const { d, err } = deps({
      ask: async () => 'yes',
      verify: () => ({ ok: false, reason: "reports 'mix2 0.3.0', expected 'mix2 0.4.0'" }),
    });
    await expect(offerUpdateAtStartup([], d)).resolves.toEqual({ action: 'continue' });
    expect(d.reexec).not.toHaveBeenCalled();
    expect(err()).toContain("expected 'mix2 0.4.0'");
  });

  it('on yes with a failed install that left no working mix2: exits with reinstall instructions', async () => {
    const { d, err } = deps({
      ask: async () => 'yes',
      install: async () => {
        rmSync(path.join(installDir, 'mix2-core'));
        return { ok: false, reason: 'boom' };
      },
    });
    await expect(offerUpdateAtStartup([], d)).resolves.toEqual({ action: 'exit', code: 1 });
    expect(err()).toContain('curl -fsSL');
  });

  it('on yes with an installer that throws: reports it and keeps going', async () => {
    const { d, err } = deps({
      ask: async () => 'yes',
      install: async () => {
        throw new Error('ENOSPC: no space left on device');
      },
    });
    await expect(offerUpdateAtStartup([], d)).resolves.toEqual({ action: 'continue' });
    expect(err()).toContain('mix2 update failed: ENOSPC');
    expect(err()).toContain('continuing with mix2 0.3.0');
  });

  it('on quit (Ctrl+C / Ctrl+D at the prompt): exits 130', async () => {
    const { d } = deps({ ask: async () => 'quit' });
    await expect(offerUpdateAtStartup([], d)).resolves.toEqual({ action: 'exit', code: 130 });
    expect(d.install).not.toHaveBeenCalled();
  });
});

describe('runUpdateCommand', () => {
  it('refuses for a source checkout', async () => {
    const { d, err } = deps({ bundleDir: dir });
    await expect(runUpdateCommand(d)).resolves.toBe(1);
    expect(err()).toContain('only works for installs made by install.sh');
    expect(d.fetchLatest).not.toHaveBeenCalled();
  });

  it('reports when GitHub cannot be reached', async () => {
    const { d, err } = deps({ fetchLatest: async () => undefined });
    await expect(runUpdateCommand(d)).resolves.toBe(1);
    expect(err()).toContain('could not reach GitHub');
    expect(err()).toContain('curl -fsSL');
    expect(readCache(cachePath)).toEqual({ failedAt: NOW });
  });

  it('says so when up to date and refreshes the cache', async () => {
    const { d, out } = deps({ fetchLatest: async () => ({ tag: 'v0.3.0', version: '0.3.0' }) });
    await expect(runUpdateCommand(d)).resolves.toBe(0);
    expect(out()).toBe('mix2 0.3.0 is up to date.\n');
    expect(d.install).not.toHaveBeenCalled();
    expect(readCache(cachePath)).toEqual({ checkedAt: NOW, latest: '0.3.0' });
  });

  it('never downgrades', async () => {
    const { d, out } = deps({ fetchLatest: async () => ({ tag: 'v0.2.0', version: '0.2.0' }) });
    await expect(runUpdateCommand(d)).resolves.toBe(0);
    expect(out()).toContain('up to date');
  });

  it('installs the pinned newer release and verifies it', async () => {
    const { d, out } = deps({ fetchLatest: async () => ({ tag: 'v0.4.0', version: '0.4.0' }) });
    await expect(runUpdateCommand(d)).resolves.toBe(0);
    expect(out()).toContain('updating mix2 0.3.0 → 0.4.0');
    expect(d.install).toHaveBeenCalledWith('v0.4.0', installDir);
    expect(d.verify).toHaveBeenCalledWith(installDir, '0.4.0');
    expect(d.reexec).not.toHaveBeenCalled();
    const cache = readCache(cachePath) as UpdateCache;
    expect(cache).toEqual({ checkedAt: NOW, latest: '0.4.0' });
  });

  it('exits 1 with the reason when the installer fails', async () => {
    const { d, err } = deps({ install: async () => ({ ok: false, reason: 'installer exited with status 1' }) });
    await expect(runUpdateCommand(d)).resolves.toBe(1);
    expect(err()).toContain('mix2 update failed: installer exited with status 1');
  });

  it('does not clear an earlier prompt timestamp', async () => {
    writeCache(cachePath, { checkedAt: NOW - HOUR, latest: '0.4.0', promptedAt: NOW - HOUR });
    const { d } = deps({ fetchLatest: async () => ({ tag: 'v0.4.0', version: '0.4.0' }) });
    await runUpdateCommand(d);
    expect(readCache(cachePath)).toEqual({ checkedAt: NOW, latest: '0.4.0', promptedAt: NOW - HOUR });
  });
});

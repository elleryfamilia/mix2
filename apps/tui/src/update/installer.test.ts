import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { detectReleaseInstall, reexec, runInstaller, verifyInstalled } from './installer.js';

let dir: string;
beforeEach(() => {
  dir = mkdtempSync(path.join(tmpdir(), 'mix2-installer-'));
});
afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

function fakeRelease(root: string, version: string): void {
  mkdirSync(root, { recursive: true });
  writeFileSync(path.join(root, 'mix2-core'), '#!/bin/sh\nexit 0\n');
  writeFileSync(
    path.join(root, 'mix2'),
    `#!/bin/sh\nif [ "$1" = "--version" ]; then echo "mix2 ${version}"; exit 0; fi\necho "$@" > "$(dirname "$0")/argv.txt"\nexit 7\n`,
  );
  writeFileSync(path.join(root, 'mix2.bundle.mjs'), '');
  chmodSync(path.join(root, 'mix2'), 0o755);
  chmodSync(path.join(root, 'mix2-core'), 0o755);
}

describe('detectReleaseInstall', () => {
  it('recognises a directory laid out like a release tarball', () => {
    fakeRelease(dir, '0.3.0');
    expect(detectReleaseInstall(dir)).toBe(dir);
  });

  it('rejects a source checkout (no core binary next to the entry point)', () => {
    writeFileSync(path.join(dir, 'cli.tsx'), '');
    expect(detectReleaseInstall(dir)).toBeUndefined();
  });

  it('rejects a dist build that has the launcher but no core', () => {
    writeFileSync(path.join(dir, 'mix2'), '');
    expect(detectReleaseInstall(dir)).toBeUndefined();
  });
});

describe('runInstaller', () => {
  const script = '#!/bin/sh\necho "$MIX2_INSTALL_DIR|$MIX2_VERSION|$MIX2_NO_LINK" > "$MIX2_INSTALL_DIR/ran.txt"\n';
  const serve =
    (body: string, status = 200): typeof fetch =>
    async () =>
      new Response(body, { status });

  it('runs the fetched installer pinned to the tag, with the install dir and no-link set', async () => {
    const installDir = path.join(dir, 'mix2');
    mkdirSync(installDir);
    const urls: string[] = [];
    const fetchImpl: typeof fetch = async (url) => {
      urls.push(String(url));
      return new Response(script, { status: 200 });
    };
    const result = await runInstaller({ tag: 'v0.4.0', installDir, fetch: fetchImpl, env: {} });
    expect(result).toEqual({ ok: true });
    expect(urls).toEqual(['https://github.com/elleryfamilia/mix2/releases/download/v0.4.0/install.sh']);
    expect(readFileSync(path.join(installDir, 'ran.txt'), 'utf8').trim()).toBe(`${installDir}|v0.4.0|1`);
  });

  it('honours MIX2_INSTALLER_URL', async () => {
    const installDir = path.join(dir, 'mix2');
    mkdirSync(installDir);
    const urls: string[] = [];
    const fetchImpl: typeof fetch = async (url) => {
      urls.push(String(url));
      return new Response(script, { status: 200 });
    };
    await runInstaller({
      tag: 'v0.4.0',
      installDir,
      fetch: fetchImpl,
      env: { MIX2_INSTALLER_URL: 'http://127.0.0.1:1/install.sh' },
    });
    expect(urls).toEqual(['http://127.0.0.1:1/install.sh']);
  });

  it('refuses a non-2xx response instead of executing an error page', async () => {
    const installDir = path.join(dir, 'mix2');
    mkdirSync(installDir);
    const result = await runInstaller({ tag: 'v9.9.9', installDir, fetch: serve('Not Found', 404), env: {} });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason).toMatch(/404/);
    expect(existsSync(path.join(installDir, 'ran.txt'))).toBe(false);
  });

  it('refuses a body that is not a shell script', async () => {
    const installDir = path.join(dir, 'mix2');
    mkdirSync(installDir);
    const result = await runInstaller({ tag: 'v0.4.0', installDir, fetch: serve('<html>oops</html>'), env: {} });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason).toMatch(/not a shell script/);
  });

  it('reports network failures', async () => {
    const fetchImpl: typeof fetch = async () => {
      throw new Error('ECONNRESET');
    };
    const result = await runInstaller({ tag: 'v0.4.0', installDir: path.join(dir, 'x'), fetch: fetchImpl, env: {} });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason).toMatch(/ECONNRESET/);
  });

  it('reports the installer exit code', async () => {
    const installDir = path.join(dir, 'mix2');
    mkdirSync(installDir);
    const result = await runInstaller({ tag: 'v0.4.0', installDir, fetch: serve('#!/bin/sh\nexit 3\n'), env: {} });
    expect(result).toEqual({ ok: false, reason: 'installer exited with status 3' });
  });

  it('refuses up front when the install location is not writable', async () => {
    const parent = path.join(dir, 'ro');
    mkdirSync(parent);
    chmodSync(parent, 0o500);
    try {
      const result = await runInstaller({
        tag: 'v0.4.0',
        installDir: path.join(parent, 'mix2'),
        fetch: serve(script),
        env: {},
      });
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.reason).toMatch(/not writable/);
    } finally {
      chmodSync(parent, 0o700);
    }
  });
});

describe('verifyInstalled', () => {
  it('accepts an install that reports the expected version', () => {
    fakeRelease(dir, '0.4.0');
    expect(verifyInstalled({ installDir: dir, version: '0.4.0' })).toEqual({ ok: true });
  });

  it('rejects a version mismatch', () => {
    fakeRelease(dir, '0.3.0');
    const result = verifyInstalled({ installDir: dir, version: '0.4.0' });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason).toMatch(/reports 'mix2 0.3.0'/);
  });

  it('rejects a launcher that cannot run', () => {
    const result = verifyInstalled({ installDir: dir, version: '0.4.0' });
    expect(result.ok).toBe(false);
  });
});

describe('reexec', () => {
  it('runs the installed launcher with the original arguments and returns its status', () => {
    fakeRelease(dir, '0.4.0');
    expect(reexec({ installDir: dir, argv: ['--lead', 'codex'] })).toBe(7);
    expect(readFileSync(path.join(dir, 'argv.txt'), 'utf8').trim()).toBe('--lead codex');
  });
});

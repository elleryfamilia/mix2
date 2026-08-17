/**
 * Runs the real install.sh (the script `mix2 update` executes) against a
 * local HTTP server serving a fixture release, so the transactional
 * behaviour is tested rather than assumed.
 */
import { execFile, spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  readlinkSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { createServer, type Server } from 'node:http';
import { arch, platform, tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from 'vitest';

const here = path.dirname(fileURLToPath(import.meta.url));
const INSTALL_SH = path.resolve(here, '../../../../install.sh');
/** `MIX2_TEST_SH=/bin/dash` runs the script under Ubuntu's /bin/sh. */
const SH = process.env['MIX2_TEST_SH'] ?? 'sh';
const TARGET = `${platform() === 'darwin' ? 'macos' : 'linux'}-${arch() === 'arm64' ? 'arm64' : 'x64'}`;
const ASSET = `mix2-${TARGET}.tar.gz`;

/** Build a release tarball the way the workflow does; returns its bytes. */
function buildRelease(
  work: string,
  version: string,
  omit: string[] = [],
  reportedVersion: string = version,
  hangOnVersion = false,
  exitStatus = 0,
): Buffer {
  const stage = path.join(work, `mix2-${version}-${TARGET}`);
  mkdirSync(stage, { recursive: true });
  const files: Record<string, string> = {
    mix2: hangOnVersion
      ? '#!/bin/sh\nsleep 60\n'
      : `#!/bin/sh\necho "mix2 ${reportedVersion}"\nexit ${exitStatus}\n`,
    'mix2-core': '#!/bin/sh\nexit 0\n',
    'mix2-consult': '#!/bin/sh\nexit 0\n',
    'mix2.bundle.mjs': `// ${version}\n`,
    LICENSE: 'MIT\n',
    'README.md': '# fixture\n',
  };
  for (const [name, body] of Object.entries(files)) {
    if (!omit.includes(name)) writeFileSync(path.join(stage, name), body, { mode: 0o755 });
  }
  const tarball = path.join(work, ASSET);
  const tar = spawnSync('tar', ['-czf', tarball, '-C', work, path.basename(stage)]);
  if (tar.status !== 0) throw new Error(`tar failed: ${tar.stderr.toString()}`);
  return readFileSync(tarball);
}

const sha256 = (buf: Buffer) => createHash('sha256').update(buf).digest('hex');

let server: Server;
let baseUrl: string;
let served: { tarball: Buffer; checksums: string; delayMs: number } = {
  tarball: Buffer.alloc(0),
  checksums: '',
  delayMs: 0,
};

beforeAll(async () => {
  server = createServer((req, res) => {
    if (req.url === `/${ASSET}`) {
      res.writeHead(200, { 'content-type': 'application/gzip' });
      res.end(served.tarball);
    } else if (req.url === '/checksums.txt') {
      setTimeout(() => {
        res.writeHead(200, { 'content-type': 'text/plain' });
        res.end(served.checksums);
      }, served.delayMs);
    } else {
      res.writeHead(404);
      res.end('nope');
    }
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('no port');
  baseUrl = `http://127.0.0.1:${address.port}`;
});
afterAll(async () => {
  await new Promise<void>((resolve) => server.close(() => resolve()));
});

let work: string;
let installDir: string;
let binDir: string;
beforeEach(() => {
  work = mkdtempSync(path.join(tmpdir(), 'mix2-install-sh-'));
  installDir = path.join(work, 'share', 'mix2');
  binDir = path.join(work, 'bin');
});
afterEach(() => {
  rmSync(work, { recursive: true, force: true });
});

function serve(
  version: string,
  opts: {
    omit?: string[];
    corruptChecksum?: boolean;
    reportedVersion?: string;
    delayMs?: number;
    hangOnVersion?: boolean;
    exitStatus?: number;
  } = {},
) {
  const tarball = buildRelease(
    path.join(work, `build-${version}`),
    version,
    opts.omit,
    opts.reportedVersion,
    opts.hangOnVersion,
    opts.exitStatus,
  );
  const sum = opts.corruptChecksum ? '0'.repeat(64) : sha256(tarball);
  served = { tarball, checksums: `${sum}  ${ASSET}\n`, delayMs: opts.delayMs ?? 0 };
}

const execFileAsync = promisify(execFile);

/** Async on purpose: the fixture server runs on this event loop, so a
 * spawnSync here would deadlock curl. */
async function run(env: Record<string, string> = {}) {
  const options = {
    encoding: 'utf8' as const,
    timeout: 20_000,
    env: {
      PATH: process.env['PATH'] ?? '/usr/bin:/bin',
      HOME: work,
      MIX2_INSTALL_DIR: installDir,
      MIX2_BIN_DIR: binDir,
      MIX2_RELEASE_BASE_URL: baseUrl,
      MIX2_VERIFY_TIMEOUT: '2',
      ...env,
    },
  };
  try {
    const { stdout, stderr } = await execFileAsync(SH, [INSTALL_SH], options);
    return { status: 0, out: stdout, err: stderr };
  } catch (error) {
    const e = error as { code?: number | string; stdout?: string; stderr?: string; message: string };
    return {
      status: typeof e.code === 'number' ? e.code : -1,
      out: e.stdout ?? '',
      err: `${e.stderr ?? ''}${typeof e.code === 'number' ? '' : `\n${e.message}`}`,
    };
  }
}

/** Anything install.sh should have cleaned up next to the install dir. */
const leftovers = () =>
  readdirSync(path.dirname(installDir)).filter((n) => n !== path.basename(installDir));

const installedVersion = () =>
  spawnSync('sh', [path.join(installDir, 'mix2')], { encoding: 'utf8' }).stdout.trim();

describe('install.sh', () => {
  it('installs a fresh copy and links the launcher', async () => {
    serve('0.4.0');
    const r = await run();
    expect(r.status, r.err).toBe(0);
    expect(r.out).toContain('installing mix2 0.4.0');
    expect(installedVersion()).toBe('mix2 0.4.0');
    expect(readlinkSync(path.join(binDir, 'mix2'))).toBe(path.join(installDir, 'mix2'));
    expect(leftovers()).toEqual([]);
  });

  it('replaces an existing install completely and leaves no staging dirs behind', async () => {
    mkdirSync(installDir, { recursive: true });
    writeFileSync(path.join(installDir, 'stale-file'), 'old');
    serve('0.4.0');
    const r = await run();
    expect(r.status, r.err).toBe(0);
    expect(installedVersion()).toBe('mix2 0.4.0');
    expect(existsSync(path.join(installDir, 'stale-file'))).toBe(false);
    expect(leftovers()).toEqual([]);
  });

  it('pins to MIX2_VERSION and refuses a download of a different version', async () => {
    serve('0.4.0');
    const ok = await run({ MIX2_VERSION: 'v0.4.0' });
    expect(ok.status, ok.err).toBe(0);
    expect(ok.out).toContain('release v0.4.0');
    const bad = await run({ MIX2_VERSION: '0.5.0' });
    expect(bad.status).toBe(1);
    expect(bad.err).toContain('asked for 0.5.0 but the download contains 0.4.0');
    expect(installedVersion()).toBe('mix2 0.4.0');
    expect(leftovers()).toEqual([]);
  });

  it('a checksum mismatch leaves the existing install untouched', async () => {
    serve('0.3.0');
    expect((await run()).status).toBe(0);
    serve('0.4.0', { corruptChecksum: true });
    const r = await run();
    expect(r.status).toBe(1);
    expect(r.err).toContain('checksum mismatch');
    expect(installedVersion()).toBe('mix2 0.3.0');
    expect(leftovers()).toEqual([]);
  });

  it('an archive missing a required file leaves the existing install untouched', async () => {
    serve('0.3.0');
    expect((await run()).status).toBe(0);
    serve('0.4.0', { omit: ['mix2-core'] });
    const r = await run();
    expect(r.status).toBe(1);
    expect(r.err).toContain('missing mix2-core');
    expect(installedVersion()).toBe('mix2 0.3.0');
    expect(leftovers()).toEqual([]);
  });

  it('tolerates trailing slashes in MIX2_INSTALL_DIR (lock and staging dirs stay siblings)', async () => {
    serve('0.3.0');
    expect((await run({ MIX2_INSTALL_DIR: `${installDir}//` })).status).toBe(0);
    serve('0.4.0');
    const r = await run({ MIX2_INSTALL_DIR: `${installDir}/` });
    expect(r.status, r.err).toBe(0);
    expect(installedVersion()).toBe('mix2 0.4.0');
    expect(leftovers()).toEqual([]);
    expect(readdirSync(installDir).filter((n) => n.startsWith('.'))).toEqual([]);
  });

  it('MIX2_NO_LINK installs without touching the bin dir', async () => {
    serve('0.4.0');
    const r = await run({ MIX2_NO_LINK: '1' });
    expect(r.status, r.err).toBe(0);
    expect(existsSync(binDir)).toBe(false);
    expect(r.out).not.toContain('on your PATH');
  });

  it('rolls back to the previous install when the new one does not run as the right version', async () => {
    serve('0.3.0');
    expect((await run()).status).toBe(0);
    serve('0.4.0', { reportedVersion: '0.3.9' });
    const r = await run();
    expect(r.status).toBe(1);
    expect(r.err).toContain("reported 'mix2 0.3.9', expected 'mix2 0.4.0'");
    expect(r.err).toContain('previous version was restored');
    expect(installedVersion()).toBe('mix2 0.3.0');
    expect(leftovers()).toEqual([]);
  });

  const start = () =>
    spawn(SH, [INSTALL_SH], {
      env: {
        PATH: process.env['PATH'] ?? '/usr/bin:/bin',
        HOME: work,
        MIX2_INSTALL_DIR: installDir,
        MIX2_BIN_DIR: binDir,
        MIX2_RELEASE_BASE_URL: baseUrl,
      },
      stdio: 'ignore',
    });
  const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

  it('cleans up the lock and keeps the old install when killed mid-download (SIGTERM)', async () => {
    serve('0.3.0');
    expect((await run()).status).toBe(0);
    serve('0.4.0', { delayMs: 2000 });
    const child = start();
    await sleep(500);
    expect(existsSync(`${installDir}.lock`)).toBe(true);
    child.kill('SIGTERM');
    const code = await new Promise<number | null>((resolve) => child.on('exit', (c) => resolve(c)));
    expect(code).toBe(143);
    expect(existsSync(`${installDir}.lock`)).toBe(false);
    expect(installedVersion()).toBe('mix2 0.3.0');
    expect(leftovers()).toEqual([]);
  });

  it('rejects a launcher whose --version prints the right thing but exits non-zero', async () => {
    serve('0.3.0');
    expect((await run()).status).toBe(0);
    serve('0.4.0', { exitStatus: 3 });
    const r = await run();
    expect(r.status).toBe(1);
    expect(r.err).toContain('--version exited with status 3');
    expect(installedVersion()).toBe('mix2 0.3.0');
    expect(leftovers()).toEqual([]);
  });

  it('gives up on a launcher that hangs on --version and restores the old install', async () => {
    serve('0.3.0');
    expect((await run()).status).toBe(0);
    serve('0.4.0', { hangOnVersion: true });
    const started = Date.now();
    const r = await run();
    expect(r.status).toBe(1);
    expect(r.err).toContain('did not answer --version within the deadline');
    expect(r.err).toContain('previous version was restored');
    expect(Date.now() - started).toBeLessThan(8_000);
    expect(installedVersion()).toBe('mix2 0.3.0');
    expect(leftovers()).toEqual([]);
  }, 15_000);

  it('a signal after the swap (during verification) still restores the old install', async () => {
    serve('0.3.0');
    expect((await run()).status).toBe(0);
    serve('0.4.0', { hangOnVersion: true });
    const child = start();
    // Wait until the swap has happened (the hanging launcher is running).
    for (let i = 0; i < 50 && !existsSync(path.join(installDir, 'mix2.bundle.mjs')); i++) await sleep(100);
    await sleep(300);
    child.kill('SIGTERM');
    const code = await new Promise<number | null>((resolve) => child.on('exit', (c) => resolve(c)));
    expect(code).toBe(143);
    expect(installedVersion()).toBe('mix2 0.3.0');
    expect(existsSync(`${installDir}.lock`)).toBe(false);
    expect(leftovers()).toEqual([]);
  });

  it('a signal during verification of a fresh install leaves no unverified tree behind', async () => {
    serve('0.4.0', { hangOnVersion: true });
    const child = start();
    for (let i = 0; i < 50 && !existsSync(path.join(installDir, 'mix2.bundle.mjs')); i++) await sleep(100);
    await sleep(300);
    child.kill('SIGTERM');
    const code = await new Promise<number | null>((resolve) => child.on('exit', (c) => resolve(c)));
    expect(code).toBe(143);
    expect(existsSync(installDir)).toBe(false);
    expect(leftovers()).toEqual([]);
  });

  it('refuses to run while another installer holds the lock', async () => {
    serve('0.4.0');
    mkdirSync(`${installDir}.lock`, { recursive: true });
    const r = await run();
    expect(r.status).toBe(1);
    expect(r.err).toContain('another mix2 install is in progress');
    expect(existsSync(installDir)).toBe(false);
    rmSync(`${installDir}.lock`, { recursive: true });
  });
});

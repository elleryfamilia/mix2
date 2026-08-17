/**
 * The side-effecting half of self-update: recognising a release install,
 * running the release's own `install.sh` pinned to a tag, checking the
 * result, and handing over to the new launcher.
 */
import { spawnSync } from 'node:child_process';
import { accessSync, constants, existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { constants as osConstants, tmpdir } from 'node:os';
import path from 'node:path';
import { installerUrl } from './github.js';

/** The files `install.sh` lays down next to the bundle. */
const RELEASE_MARKERS = ['mix2-core', 'mix2'];

/**
 * A release install is a directory that holds the bundle *and* its
 * siblings from the tarball. A source checkout (`tsx src/cli.tsx`) or a
 * `dist/cli.js` build has neither, so it is never offered an update.
 */
export function detectReleaseInstall(bundleDir: string): string | undefined {
  return RELEASE_MARKERS.every((f) => existsSync(path.join(bundleDir, f))) ? bundleDir : undefined;
}

export type Outcome = { ok: true } | { ok: false; reason: string };

export interface RunInstallerOptions {
  /** Release tag to install, e.g. `v0.4.0`. */
  tag: string;
  installDir: string;
  fetch: typeof fetch;
  env: NodeJS.ProcessEnv;
  /** Time allowed to download the installer script itself. */
  timeoutMs?: number;
}

/**
 * Download the installer that shipped with `tag` and run it against
 * `installDir`. The whole script is fetched and checked before `sh` sees
 * any of it; the child inherits the terminal so the user watches the
 * installer's own progress. Returns when the installer exits.
 */
export async function runInstaller(options: RunInstallerOptions): Promise<Outcome> {
  const { tag, installDir, env } = options;
  for (const dir of [path.dirname(installDir), installDir]) {
    if (!existsSync(dir)) continue;
    try {
      accessSync(dir, constants.W_OK);
    } catch {
      return { ok: false, reason: `${dir} is not writable by this user` };
    }
  }

  const url = env['MIX2_INSTALLER_URL'] || installerUrl(tag);
  let script: string;
  try {
    const response = await options.fetch(url, {
      redirect: 'follow',
      signal: AbortSignal.timeout(options.timeoutMs ?? 30_000),
      headers: { 'user-agent': 'mix2-update' },
    });
    if (!response.ok) {
      return { ok: false, reason: `could not download the installer (HTTP ${response.status} for ${url})` };
    }
    script = await response.text();
  } catch (error) {
    return { ok: false, reason: `could not download the installer: ${errorMessage(error)}` };
  }
  if (!script.startsWith('#!/bin/sh')) {
    return { ok: false, reason: `the download from ${url} is not a shell script — not running it` };
  }

  const scratch = mkdtempSync(path.join(tmpdir(), 'mix2-update-'));
  const file = path.join(scratch, 'install.sh');
  try {
    writeFileSync(file, script, { mode: 0o600 });
    const child = spawnSync('sh', [file], {
      stdio: 'inherit',
      env: {
        ...env,
        MIX2_INSTALL_DIR: installDir,
        MIX2_VERSION: tag,
        // The existing `mix2` symlink already points into installDir, which
        // keeps its path across the swap; relinking could only create a
        // stray link for someone who chose a custom bin dir.
        MIX2_NO_LINK: '1',
      },
    });
    if (child.error) return { ok: false, reason: `could not run sh: ${errorMessage(child.error)}` };
    const status = exitStatus(child.status, child.signal);
    return status === 0 ? { ok: true } : { ok: false, reason: `installer exited with status ${status}` };
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

/** Prove the new install runs and is the version we meant to install. */
export function verifyInstalled(options: { installDir: string; version: string }): Outcome {
  const launcher = path.join(options.installDir, 'mix2');
  const child = spawnSync(launcher, ['--version'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    encoding: 'utf8',
    timeout: 10_000,
  });
  if (child.error) return { ok: false, reason: `${launcher} could not run: ${errorMessage(child.error)}` };
  const reported = (child.stdout ?? '').trim();
  const expected = `mix2 ${options.version}`;
  if (child.status !== 0 || reported !== expected) {
    return {
      ok: false,
      reason: `${launcher} reports '${reported || child.stderr?.trim() || `exit ${child.status}`}', expected '${expected}'`,
    };
  }
  return { ok: true };
}

/** Hand the terminal to the freshly installed launcher; returns its exit code. */
export function reexec(options: { installDir: string; argv: string[] }): number {
  const child = spawnSync(path.join(options.installDir, 'mix2'), options.argv, { stdio: 'inherit' });
  if (child.error) return 1;
  return exitStatus(child.status, child.signal);
}

/** Shell convention: a signal death is 128 + the signal number. */
function exitStatus(status: number | null, signal: NodeJS.Signals | null): number {
  if (status !== null) return status;
  const signo = signal ? osConstants.signals[signal] : undefined;
  return 128 + (signo ?? 0);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

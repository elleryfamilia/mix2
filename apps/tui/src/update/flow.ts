/**
 * The two user-facing update flows, written against injectable
 * dependencies so every branch is unit-testable:
 *
 *  - `runUpdateCommand`     — `mix2 update`
 *  - `offerUpdateAtStartup` — the daily check + prompt before the TUI starts
 */
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { VERSION } from '../version.js';
import { decide, defaultCachePath, readCache, writeCache, type UpdateCache } from './check.js';
import { fetchLatestRelease, INSTALL_ONE_LINER, type Release } from './github.js';
import {
  detectReleaseInstall,
  reexec,
  runInstaller,
  verifyInstalled,
  type Outcome,
} from './installer.js';
import { askYesNo, type YesNo } from './prompt.js';
import { compareVersions } from './semver.js';

export interface FlowDeps {
  now: () => number;
  env: NodeJS.ProcessEnv;
  /** stdin and stdout are both terminals (we can ask a question). */
  interactive: boolean;
  /** Directory the running code lives in; a release install has the
   * core and launcher next to it. */
  bundleDir: string;
  cachePath: string;
  fetchLatest: (timeoutMs: number) => Promise<Release | undefined>;
  ask: (question: string) => Promise<YesNo>;
  install: (tag: string, installDir: string) => Promise<Outcome>;
  verify: (installDir: string, version: string) => Outcome;
  reexec: (installDir: string, argv: string[]) => number;
  out: (text: string) => void;
  err: (text: string) => void;
  debug: boolean;
  current: string;
}

export function defaultDeps(overrides: Partial<FlowDeps> = {}): FlowDeps {
  return {
    now: () => Date.now(),
    env: process.env,
    interactive: Boolean(process.stdin.isTTY && process.stdout.isTTY),
    bundleDir: path.dirname(fileURLToPath(import.meta.url)),
    cachePath: defaultCachePath(),
    fetchLatest: (timeoutMs) => fetchLatestRelease({ fetch, timeoutMs }),
    ask: (question) => askYesNo({ input: process.stdin, output: process.stdout, question }),
    install: (tag, installDir) => runInstaller({ tag, installDir, fetch, env: process.env }),
    verify: (installDir, version) => verifyInstalled({ installDir, version }),
    reexec: (installDir, argv) => reexec({ installDir, argv }),
    out: (text) => process.stdout.write(text),
    err: (text) => process.stderr.write(text),
    debug: false,
    current: VERSION,
    ...overrides,
  };
}

const NOT_A_RELEASE = (dir: string) =>
  `mix2 update only works for installs made by install.sh (this mix2 runs from ${dir}).\n` +
  `From a source checkout: git pull, then pnpm build.\n`;

const REINSTALL_HINT = `You can always (re)install the latest release with:\n  ${INSTALL_ONE_LINER}\n`;

/** `mix2 update`: returns the process exit code. */
export async function runUpdateCommand(deps: FlowDeps): Promise<number> {
  const installDir = detectReleaseInstall(deps.bundleDir);
  if (!installDir) {
    deps.err(NOT_A_RELEASE(deps.bundleDir));
    return 1;
  }
  const cache = readCache(deps.cachePath) ?? {};
  const latest = await deps.fetchLatest(10_000);
  if (!latest) {
    writeCache(deps.cachePath, { ...cache, failedAt: deps.now() });
    deps.err(`mix2 update: could not reach GitHub to check for a newer release.\n${REINSTALL_HINT}`);
    return 1;
  }
  writeCache(deps.cachePath, { ...cache, checkedAt: deps.now(), latest: latest.version, failedAt: undefined });
  if (compareVersions(latest.version, deps.current) <= 0) {
    deps.out(`mix2 ${deps.current} is up to date.\n`);
    return 0;
  }
  deps.out(`updating mix2 ${deps.current} → ${latest.version}\n`);
  const outcome = await installAndVerify(deps, latest, installDir);
  if (!outcome.ok) {
    deps.err(`mix2 update failed: ${outcome.reason}\n${REINSTALL_HINT}`);
    return 1;
  }
  return 0;
}

export type StartupAction = { action: 'continue' } | { action: 'exit'; code: number };

/**
 * Before the TUI starts: refresh what we know about the newest release (at
 * most daily, with a short network budget), and if it is newer than us,
 * ask once a day whether to install it now. `argv` is re-used to relaunch
 * the new version so the user lands where they were going.
 */
export async function offerUpdateAtStartup(argv: string[], deps: FlowDeps): Promise<StartupAction> {
  const proceed: StartupAction = { action: 'continue' };
  const trace = (why: string) => {
    if (deps.debug) deps.err(`[update-check] ${why}\n`);
  };
  // The README documents `=1`; any set value disables except the obvious
  // "off" spellings, so `=0` / `=false` behave the way they read.
  const noCheck = deps.env['MIX2_NO_UPDATE_CHECK'];
  if (noCheck !== undefined && !['', '0', 'false'].includes(noCheck.toLowerCase())) {
    trace('disabled by MIX2_NO_UPDATE_CHECK');
    return proceed;
  }
  if (!deps.interactive) {
    trace('not a terminal');
    return proceed;
  }
  const installDir = detectReleaseInstall(deps.bundleDir);
  if (!installDir) {
    trace(`not a release install (${deps.bundleDir})`);
    return proceed;
  }

  let cache: UpdateCache = readCache(deps.cachePath) ?? {};
  let decision = decide({ cache, now: deps.now(), current: deps.current });
  if (decision.needsCheck) {
    const latest = await deps.fetchLatest(2_000);
    if (latest) {
      cache = { ...cache, checkedAt: deps.now(), latest: latest.version, failedAt: undefined };
    } else {
      trace('check failed; backing off');
      cache = { ...cache, failedAt: deps.now() };
    }
    writeCache(deps.cachePath, cache);
    decision = decide({ cache, now: deps.now(), current: deps.current });
  }
  if (!decision.shouldPrompt || cache.latest === undefined) return proceed;

  const latest: Release = { version: cache.latest, tag: `v${cache.latest}` };
  writeCache(deps.cachePath, { ...cache, promptedAt: deps.now() });
  const answer = await deps.ask(
    `\nmix2 ${latest.version} is available (you have ${deps.current}).\nUpdate now? [y/N] `,
  );
  if (answer === 'quit') return { action: 'exit', code: 130 };
  if (answer !== 'yes') return proceed;

  const outcome = await installAndVerify(deps, latest, installDir);
  if (outcome.ok) {
    deps.out(`✓ updated to ${latest.version} — starting mix2\n`);
    return { action: 'exit', code: deps.reexec(installDir, argv) };
  }
  deps.err(`mix2 update failed: ${outcome.reason}\n`);
  if (detectReleaseInstall(installDir)) {
    deps.err(`continuing with mix2 ${deps.current}\n`);
    return proceed;
  }
  deps.err(`mix2 is no longer installed correctly.\n${REINSTALL_HINT}`);
  return { action: 'exit', code: 1 };
}

async function installAndVerify(deps: FlowDeps, release: Release, installDir: string): Promise<Outcome> {
  // Anything the installer throws (rather than reports) is still just a
  // failed update: the caller decides whether to continue or exit.
  try {
    const installed = await deps.install(release.tag, installDir);
    if (!installed.ok) return installed;
    return deps.verify(installDir, release.version);
  } catch (error) {
    return { ok: false, reason: error instanceof Error ? error.message : String(error) };
  }
}

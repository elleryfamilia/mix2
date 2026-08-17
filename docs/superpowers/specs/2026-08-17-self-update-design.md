# Self-update: `mix2 update` and the startup update check

Date: 2026-08-17 · Status: revised after critic + Codex review (see "Review revisions")

## Objective

1. `mix2 update` upgrades a release install of mix2 in place.
2. When `mix2` starts, it tells the user if a newer release exists and lets
   them choose to install it before the app continues.
3. Housekeeping: move the release workflow's GitHub Actions from their
   Node 20 majors to the current Node 24 majors (clears CI deprecation
   warnings).

## Context

- mix2 is distributed only as GitHub Release tarballs
  (`mix2-<target>.tar.gz` + `checksums.txt`, tags `vX.Y.Z`). `install.sh`
  downloads the latest tarball, verifies its checksum, extracts it to
  `~/.local/share/mix2` (`$MIX2_INSTALL_DIR`), and links
  `~/.local/bin/mix2` → `<install dir>/mix2` (a sh launcher that `exec`s
  `node mix2.bundle.mjs`, exporting `MIX2_CORE_BIN`).
- The running version is a hardcoded string in `apps/tui/src/cli.tsx`,
  bumped by each `chore(release)` commit.
- `https://github.com/elleryfamilia/mix2/releases/latest` answers
  `302 Location: …/releases/tag/vX.Y.Z` — no auth, not subject to the
  REST API's 60 req/h unauthenticated limit.
- The Rust core has no HTTP client. Node 22 has `fetch` built in.
- The TUI has no user-facing "pre-app" screen: `cli.tsx` parses args,
  enters the alternate screen, spawns the core, and renders Ink.

## Approaches considered

**A. Do it in the sh launcher.** Check + prompt in `scripts/mix2-launcher.sh`
before `exec node`. Rejected: shell semver/caching/prompting is fragile and
untestable with the repo's test setup; `pnpm dev` would never exercise it;
`mix2 update` would still need TS-side arg handling.

**B. Do it in the Rust core, surface it as an event, prompt inside the TUI.**
Rejected: adds an HTTP client dependency to the core; the "install now"
path must leave the alt screen, stop the core, run the installer, and
re-exec — the most machinery for the least benefit.

**C. Do it in the TS entry point before the TUI starts (chosen).** A small
`apps/tui/src/update/` module: check the latest release (with a 24h cache),
prompt on the normal screen with a plain y/N, run the installer, re-exec.
Uses Node's `fetch`, `readline`, `child_process`; unit-testable with
injected dependencies. No Rust changes. No new npm dependencies.

## Design

### Module layout (`apps/tui/src/`)

- `version.ts` — `export const VERSION = '0.3.0'`. Single source of truth
  for the running version (moved out of `cli.tsx`; release bumps edit it).
- `update/semver.ts` — `compareVersions(a, b): -1 | 0 | 1`. Numeric
  compare of dotted components; a pre-release suffix (`-rc.1`) sorts below
  the bare version; leading `v` tolerated.
- `update/check.ts` — the cache and the decision:
  - Cache file `$XDG_CACHE_HOME/mix2/update-check.json` (default
    `~/.cache/mix2/…`): `{ checkedAt?: ms, latest?: string, promptedAt?: ms,
    failedAt?: ms }`, validated with zod; unreadable/invalid → treated as
    absent; written via temp file + rename; write failures ignored.
  - `decide({ cache, now, current })` → `{ needsCheck, shouldPrompt }`:
    `needsCheck` when no successful check (`checkedAt`) within 24h **and**
    no failed check (`failedAt`) within 1h — the backoff keeps offline or
    proxied machines from paying the network timeout on every launch;
    `shouldPrompt` when `latest` is known, newer than `current`, and
    `promptedAt` is absent or older than 24h.
- `update/installer.ts` — side effects, each taking injectable deps:
  - `detectReleaseInstall(dir)`: true when `dir` contains both `mix2-core`
    and the `mix2` launcher. `dir` = directory of the running bundle
    (`import.meta.url`). Dev checkouts (`tsx src/cli.tsx`, `dist/cli.js`)
    are not release installs.
  - `fetchLatestVersion({ fetch, timeoutMs })`: GET
    `…/releases/latest` with `redirect: 'manual'`, parse the `Location`
    header into `{ tag: 'v0.4.0', version: '0.4.0' }`. Any failure →
    `undefined`.
  - `runInstaller({ tag, installDir })`: fetch the installer **pinned to
    that tag** — `https://github.com/elleryfamilia/mix2/releases/download/<tag>/install.sh`
    (override: `MIX2_INSTALLER_URL`, for testing) — require a 2xx and read
    the whole body before anything runs; sanity-check it starts with
    `#!/bin/sh`; write it to a temp file and run `sh <file>` with
    inherited stdio (`spawnSync`, so the parent never competes for the
    terminal) and env `MIX2_INSTALL_DIR=<installDir>`,
    `MIX2_VERSION=<tag>`, `MIX2_NO_LINK=1`, so the user sees the
    installer's own progress. Before running, checks the install dir's
    parent is writable and reports that plainly instead of a mid-script
    error. Returns the exit code (a signal death maps to 128 + signo).
  - `verifyInstalled({ installDir, version })`: runs
    `<installDir>/mix2 --version` (10s timeout) and requires
    `mix2 <version>` — proves the new install actually runs and is the
    version we said we were installing.
- `update/prompt.ts` — `askYesNo({ input, output, question })` →
  `'yes' | 'no' | 'quit'`. Only `y`/`yes` (case-insensitive) is yes; a
  closed stream (Ctrl+D, or Ctrl+C — readline closes the interface when
  no SIGINT listener is registered) is `quit`. The interface is closed
  before any child process runs so nothing competes for stdin.
- `update/flow.ts` — two entry points:
  - `runUpdateCommand()` for `mix2 update`.
  - `offerUpdateAtStartup(argv)` for a normal launch.

### `mix2 update`

1. Not a release install → print
   "mix2 update only works for installs made by install.sh (running from
   <dir>). From a source checkout, pull and run `pnpm build`." → exit 1.
2. Fetch latest (10s timeout). Failure → "could not reach GitHub to check
   for updates" → exit 1.
3. Latest ≤ current → "mix2 <v> is up to date." → cache updated → exit 0.
4. Otherwise print "updating mix2 <cur> → <latest>", run the installer
   pinned to the resolved tag, verify the installed version, update the
   cache on success, exit 0; any failure prints the reason plus the manual
   fallback (`curl … | sh`) and exits 1.

### Startup check (plain `mix2`, no subcommand)

Skipped entirely when any of: `MIX2_NO_UPDATE_CHECK` is set (non-empty),
stdin or stdout is not a TTY, not a release install.

1. Load cache. If `needsCheck`: fetch latest with a **2s** timeout; on
   success merge `{ …cache, checkedAt: now, latest }` (preserving
   `promptedAt`) and **re-run `decide`** on the merged cache, so a first
   launch that discovers a newer release prompts immediately; on failure
   keep the old cache (offline is silent; `--debug` logs the reason to
   stderr).
2. If `shouldPrompt`: write `promptedAt = now` (whatever the answer), then
   ask on the normal screen:

   ```
   mix2 0.4.0 is available (you have 0.3.0).
   Update now? [y/N]
   ```

   - `y` / `yes` (case-insensitive) → run the installer pinned to the
     tag, verify. Success → print "✓ updated to 0.4.0 — starting mix2"
     and re-exec `<installDir>/mix2` with the original argv (stdio
     inherited); exit with its status. Failure → print the error and
     "continuing with mix2 0.3.0"; the app starts normally.
   - Enter or anything else → the app starts normally. (Default is *no*:
     keystrokes typed while mix2 is starting are still buffered in the
     terminal, and a stray Enter must not run an installer.)
   - EOF / Ctrl+C at the prompt → exit 130 (the user asked to leave).
3. Only after this does `cli.tsx` enter the alternate screen and start
   the core.

Cadence: at most one network check per 24h and at most one prompt per
24h. Declining does not suppress future prompts forever: the next day's
check re-asks (and picks up any newer release published since).

### `cli.tsx` changes

- `parseArgs` recognises the positional `update` subcommand (must be
  first). `--help` lists it. `-V` prints `VERSION`.
- Startup becomes `await offerUpdateAtStartup(process.argv.slice(2))`
  before `enterAltScreen()`; the module already runs as ESM so top-level
  `await` is fine.

### Release workflow

- Bump `actions/checkout@v7`, `actions/setup-node@v7`,
  `actions/upload-artifact@v7`, `actions/download-artifact@v8`,
  `pnpm/action-setup@v6`, `softprops/action-gh-release@v3` (all Node 24;
  `dtolnay/rust-toolchain@stable` is composite and unaffected).
- Attach `install.sh` to every release (the publish job checks out the
  repo — *before* downloading artifacts, so checkout's clean step cannot
  remove `dist/` — and adds it to `files:`), so `mix2 update` runs the
  installer that matches the release it installs, independent of branch
  names.
- `softprops/action-gh-release` gets `prerelease: ${{ contains(github.ref_name, '-') }}`
  so a `v0.5.0-rc1` tag never becomes `releases/latest` — otherwise every
  user would be prompted to install a release candidate.
- Version guard: the build job fails unless the tag (`vX.Y.Z`),
  `apps/tui/package.json`, `Cargo.toml` (`workspace.package.version`) and
  `apps/tui/src/version.ts` all agree — a stale copy would ship a release
  that forever offers itself as an update. `version.test.ts` enforces the
  package.json ↔ version.ts half locally too.

### `install.sh` hardening (used by both fresh installs and updates)

- `MIX2_VERSION=<tag>` pins the release (`releases/download/<tag>/…`
  instead of `latest`); the tarball's embedded version must match or the
  install fails. `MIX2_RELEASE_BASE_URL` overrides the download base
  (tests only).
- Transactional: extract into a sibling staging dir
  (`<INSTALL_DIR>.new.XXXXXX`, same filesystem), check the expected files
  are present (`mix2`, `mix2-core`, `mix2-consult`, `mix2.bundle.mjs`),
  then `mv` the old dir aside, `mv` the staging dir into place, and delete
  the old one. If the swap fails the old dir is moved back. A failure at
  any earlier step leaves the existing install untouched — no more
  `rm -rf` before extraction.
- `: "${HOME:?}"` up front, so an unset `HOME` fails with a clear message
  instead of a `set -u` abort halfway through.
- A `mkdir`-based lock (`<INSTALL_DIR>.lock`) serialises concurrent
  installers; a stale lock is reported with the path to remove.
- `MIX2_NO_LINK=1` skips the `~/.local/bin/mix2` symlink and the PATH
  hint. `mix2 update` sets it: the existing link already points at
  `<INSTALL_DIR>/mix2`, which the swap keeps valid, and a user who
  installed with a custom `MIX2_BIN_DIR` must not get a stray link.

### Docs

README "Install" gains an "Update" note: `mix2 update`; the daily startup
check; `MIX2_NO_UPDATE_CHECK=1` to disable.

## Assumptions

- Enter defaults to "no" at the prompt (see the startup section for why).
- Once per 24h is the right nag cadence; the user's ask ("let the user
  know … choose before continuing") wants a real prompt, not a footer
  notice, but not on every launch.
- Node's `fetch` (no proxy env support) is acceptable for the check; the
  installer itself uses `curl`, which honours proxies. A failed check is
  silent at startup and gives a clear message + the manual `curl … | sh`
  fallback in `mix2 update`.
- Custom `MIX2_INSTALL_DIR` installs are handled because the install dir is
  derived from where the running bundle lives, not from a default path.

## Risks

- The install dir is swapped while the old bundle is executing. Safe on
  Unix: Node has fully loaded the ESM bundle, the launcher `exec`ed node
  (no sh process is reading the script), and the core is not running
  during the update. Re-exec uses the *new* launcher.
- Until the first release that ships this feature, no release carries an
  `install.sh` asset; `mix2 update` from an older build was never possible
  anyway, and the new build only fetches `releases/download/<tag>/install.sh`
  when a newer release (which carries the asset) exists.
- The version check and the installer are separate requests, so a release
  published between them cannot cause a mismatch: the installer is pinned
  to the tag the check resolved, and the result is verified.
- A slow network can hold startup for up to 2s once per day (once per
  hour while checks keep failing). Offline machines fail DNS immediately
  and pay nothing. Accepted.
- Updating while another mix2 session is running: that session's old
  core keeps working (its binaries stay open), but its lead's next
  `mix2-consult` call resolves to the *new* helper binary. The consult
  protocol is stable across releases today; if it ever changes, that
  session's consultations fail until it is restarted. Documented, not
  guarded.

## Validation

- `vitest`: semver compare (lengths, pre-release, `v` prefix); cache
  decision with a fake clock (fresh/stale/prompted-recently/no cache/older
  latest/first launch discovers newer → prompt); `detectReleaseInstall` on temp dirs; `fetchLatestVersion` with a
  fake fetch (302 + Location, 200 without Location, throw, timeout);
  `runUpdateCommand`/`offerUpdateAtStartup` with injected fetch/prompt/
  installer/reexec (asserts cache writes, prompt shown/skipped, re-exec
  args, exit codes); `parseArgs` accepts `update`.
- `pnpm check` (typecheck, tests, cargo fmt/clippy/test) green.
- Integration (vitest, Unix): run the real `install.sh` against a local
  `node:http` fixture server serving a fixture tarball + `checksums.txt`
  into a temp `MIX2_INSTALL_DIR`: fresh install; update over an existing
  install; checksum mismatch leaves the old install intact; `MIX2_VERSION`
  mismatch fails; `MIX2_NO_LINK` makes no symlink; a held lock refuses.
- Manual: build a bundle, lay it out as a release install in a temp dir,
  run `MIX2_INSTALLER_URL=<raw install.sh URL> <tmp>/mix2 update`.

## Rollback

Revert the commit. The only persistent artefact is
`~/.cache/mix2/update-check.json`, which nothing else reads.

## First implementation step

Create `apps/tui/src/version.ts` and `update/semver.ts` with tests
(TDD), then `check.ts`, `installer.ts`, `flow.ts`, then wire `cli.tsx`.

## Review revisions

Codex (cross-model review) and a fresh critic subagent reviewed the draft.
Changes made in response:

- Install is now transactional (staging dir + swap + lock) instead of
  `rm -rf` then extract, so an interrupted or failed update never leaves
  the user without a working `mix2`.
- The installer is pinned to the tag the check resolved
  (`MIX2_VERSION`, tag-specific URLs) and the installed version is
  verified with `mix2 --version`; a release landing mid-update can no
  longer install one version while we report another.
- The startup flow re-runs the decision after a fresh check (first launch
  prompts immediately) and preserves `promptedAt` when writing.
- Prompt default flipped to `[y/N]`.
- `MIX2_NO_LINK` so updates never create a stray `~/.local/bin/mix2` for
  custom-bin-dir installs.
- Version consistency is enforced (unit test + release-workflow guard);
  reading `package.json` at runtime was tried and rejected because the
  JSON import shifts tsc's output layout to `dist/src/`.
- `parseArgs` moved out of `cli.tsx` into `args.ts` so it is testable.
- An integration test exercises the real `install.sh` against a fixture
  HTTP server.
- Failed checks back off for 1h (`failedAt`) instead of retrying on every
  launch; readline close/Ctrl+C handled explicitly; installer and re-exec
  use `spawnSync` with inherited stdio; `-`-suffixed tags are marked
  prerelease so they never become `latest`; write-permission pre-check.

A post-implementation review (fresh subagent, two-stage: spec compliance
then code quality) found one real bug and several small ones, all fixed:

- readline pauses `process.stdin` when the prompt closes, and a paused
  stream does not resume just because a `'data'` listener is attached, so
  the TUI's keyboard was dead after answering the prompt. `FilteredStdin`
  now calls `resume()` explicitly (regression test + verified through a
  real pty by typing into the composer).
- `install.sh`: trailing slash in `MIX2_INSTALL_DIR` put the lock/staging
  dirs *inside* the install dir (stripped now); the EXIT trap is installed
  right after the lock is taken so a failing `mktemp` cannot leave a stale
  lock; the trap moves the old install back if the process dies between
  the two `mv`s.
- Future-dated cache timestamps are treated as stale, not fresh.
- `mix2 --version` verification timeout is 10s as specified; the
  unwritable-directory message names the directory that failed.

Codex's review of the pull request (PR #2) added two `install.sh` fixes:

- `sh` does not run the EXIT trap when killed by SIGHUP/SIGINT/SIGTERM,
  so a closed terminal mid-update left a stale lock (or, between the two
  `mv`s, no install at all). HUP/INT/TERM now `exit` through the EXIT
  trap; an integration test kills the installer mid-download and checks
  the lock is gone and the old install intact.
- The old install was deleted before anything proved the new one runs.
  `install.sh` now runs `<new>/mix2 --version` (when Node ≥ 22 is
  present) before discarding the previous install and swaps it back on a
  mismatch; the TS-side `verifyInstalled` remains as a second check.

Round two of Codex's PR review, also fixed:

- A signal *after* the swap (while the new launcher was being probed) left
  the unverified tree in place. The EXIT trap now restores the previous
  install whenever it has been moved aside and the new one has not been
  accepted, wherever the exit happens.
- The shell-side `--version` probe had no deadline; it now runs under a
  watchdog (10s, `MIX2_VERIFY_TIMEOUT` for tests). Both cases have
  integration tests.

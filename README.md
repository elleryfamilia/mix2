# Cladex

Cladex is a terminal interface for Claude Code and Codex that turns them
into a small AI engineering team. Choose the lead, talk normally, and the
lead decides when bringing in the other agent for a second opinion,
challenge, or review would improve the result.

You talk to one team, not two chat windows:

```text
$ cladex --lead claude

  cladex   Claude lead · Codex teammate                    ~/src/acme
────────────────────────────────────────────────────────────────────────

  ❯ I'm thinking about replacing Postgres with DynamoDB. What do you think?

  ● Claude — investigating                                        ⠸ 0:14
    ├ read src/db/session.ts
    └ search "SessionManager" — 14 matches

  ↔ bringing in codex

  ⇄ conferred
  ╭─  Claude  ⇄  Codex  ─────────────────────────────────────── 0:31 ─╮
  │ ● independently evaluate DynamoDB for this repository             │
  │ ○ your session queries lean on joins you'd have to denormalize    │
  ╰────────────────────────────────────────────────────────────────────╯

   Team  claude + codex

  I wouldn't replace Postgres wholesale. …
```

## Why

Two frontier coding agents disagree in useful ways — but only if the
second one forms its opinion independently, and only if someone owns the
final answer. Cladex is not "run Claude and Codex at the same time": it is
**adaptive collaboration**. The lead answers trivial things alone, brings
the teammate in for judgment calls, challenges material disagreements
once, and gives you one coherent recommendation with the disagreement
disclosed when it matters.

## How it works

- The **lead** (your choice) owns the conversation and runs with its
  normal configuration plus appended Cladex role instructions.
- The lead gets one extra shell command, **`cladex-consult`**: pipe a
  prompt in, get the teammate's independent written assessment back. The
  lead decides *when* to use it; the Rust runtime decides *whether it is
  allowed to* (budget, recursion, availability).
- Consultations run as **fresh teammate sessions** in the same project
  directory, un-anchored by the lead's opinion.
- If at least one consultation succeeded, the answer is attributed to
  **Team**; otherwise to the lead alone. Attribution never lies.
- The runtime enforces a per-turn consultation budget (default 2) and
  refuses recursive consultation in code, not prompts.

Architecture details: [docs/architecture.md](docs/architecture.md).
Visual system: [docs/design-system.md](docs/design-system.md).

```text
Ink UI (TypeScript + React)  ↕ JSONL  Rust core  →  claude / codex CLIs
```

## Requirements

- macOS or Linux
- Node.js ≥ 22 and pnpm
- Rust (stable) — for building the core
- [Claude Code](https://claude.com/claude-code) CLI (`claude`), logged in
- [Codex](https://developers.openai.com/codex/cli) CLI (`codex`), logged in

Only the **lead** must be installed; a missing teammate degrades
gracefully (the lead works solo and says so).

## Install & run

```bash
pnpm install
pnpm build          # release cargo build + TypeScript build

# development (debug core, tsx runner):
pnpm dev            # runs cladex in the current directory
```

The user-facing command is `cladex` (`apps/tui/dist/cli.js`, exposed as a
bin). It launches the internal `cladex-core` runtime itself — users never
interact with the core directly. In development the core is found in
`target/{debug,release}` automatically; a packaged install can point at it
with `CLADEX_CORE_BIN`.

```bash
cladex                    # lead from config, else claude
cladex --lead codex       # short: -l codex
cladex --cwd ~/src/acme   # run against another project
cladex --debug            # verbose logs + IPC trace in /tmp
```

## Configuration

`~/.config/cladex/config.toml` (respects `$XDG_CONFIG_HOME`):

```toml
lead = "claude"

[collaboration]
max_consults_per_turn = 2

[claude]
command = "claude"        # or a custom path

[codex]
command = "codex"
```

Precedence: CLI flags > user config > defaults.

## Keyboard

| Key | Action |
| --- | --- |
| `Enter` | submit |
| `Ctrl+J` (or `Shift+Enter` where supported) | newline in the composer |
| `Esc` | cancel the running turn / close the team panel |
| `Ctrl+C` | cancel; twice quits |
| `Ctrl+T` | toggle the team activity panel |
| `PageUp` / `PageDown` | scroll the conversation |
| `Ctrl+Q` | quit |

The team panel (Ctrl+T) shows participants, timings, tool counts, the
consultation exchange, and each teammate consultation's final response.
Hidden model reasoning is never shown anywhere.

## Security model

- Cladex does not touch provider authentication; both CLIs use your
  existing logins. Auth failures surface the provider's own message.
- No permission bypass flags, ever. Claude Code runs with your normal
  permission configuration plus exactly one added allowance:
  `Bash(cladex-consult:*)`, so the lead can reach its teammate.
- Codex as *teammate* runs with your default `codex exec` sandbox
  (read-only). Codex as *lead* runs with Codex's standard
  `workspace-write` sandbox (still no network) — its default read-only
  sandbox blocks the consult channel entirely; this is the one deliberate
  elevation, and it matches what interactive Codex does anyway.
- Recursion (`teammate consulting anyone`) is refused by the runtime.
- Runtime state in `/tmp/cladex/<session>/` contains no credentials and is
  removed on exit. Debug logs never include prompts, file contents, or
  teammate responses.

## Provider requirements

Verified against `claude` 2.1.x (`-p --output-format stream-json`,
`--append-system-prompt`, `--resume`) and `codex-cli` 0.146.x
(`exec --json`, `exec resume`, `-c developer_instructions=…`). Adapters
parse tolerantly: unknown event types from newer CLIs are ignored, never
fatal. If an installed CLI lacks a required capability, Cladex reports a
clear compatibility error at startup.

## Limitations (MVP)

- Two agents, one lead, one teammate; no third agent yet (the agent model
  is designed so `--team codex,gemini` can exist later).
- Teammate consultations are stateless between turns by design.
- Analysis-first: the lead can edit files only where your provider
  permissions already allow it; Cladex does not widen write access.
- Unix (macOS/Linux) only for now; process management is isolated so
  Windows support can be added.
- No slash commands, themes, or MCP integration yet.

## Development & testing

```bash
pnpm check      # typecheck + vitest + cargo fmt/clippy/test
cargo test      # Rust unit + integration suites
pnpm test       # TUI suites (vitest + ink-testing-library)
```

Automated tests never call real models: `tests/fixtures/fake-claude` and
`fake-codex` are executable stand-ins that speak each provider's exact
stream format and support scenarios (streaming, tool events, session
resume, consultation, failure, rate limit, malformed output, slow runs,
child-process trees for cancellation tests). Point Cladex at them with
`CLADEX_CLAUDE_CMD` / `CLADEX_CODEX_CMD` — the integration suite drives
the full stack this way, including both consult transports, budget
enforcement, recursion refusal, and process-tree cancellation.

> Cladex is a working name; branding is confined to the crate/package
> names and the header chip, so a rename stays shallow.

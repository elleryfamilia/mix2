# mix2

mix2 is a terminal interface for Claude Code and Codex that turns them
into a small AI engineering team. Choose the lead, talk normally, and the
lead decides when bringing in the other agent for a second opinion,
challenge, or review would improve the result.

You talk to one team, not two chat windows:

```text
$ mix2 --lead claude

  mix2   Claude lead · Codex teammate                    ~/src/acme
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
final answer. mix2 is not "run Claude and Codex at the same time": it is
**adaptive collaboration**. You came for the team — if you wanted a single
agent you would have opened it directly — so the lead consults its
teammate on every substantive request by default, challenges material
disagreements once, and gives you one coherent recommendation with the
disagreement disclosed when it matters. Only no-ops (greetings,
acknowledgements) and clarifying questions stay single-agent.

## How it works

- **Vague asks get scoped first.** For a broad request ("check for
  security issues") the team replies once with what it would do — scope,
  focus, deliverable — and waits for your go-ahead before both agents
  spend minutes and tokens. Specific or detailed requests go straight to
  work, no back-and-forth.
- **The team writes plans, not code.** `.mix2/` in your project is the
  team's scratchpad — the only place it can write (for Claude leads this
  is enforced by path-scoped permissions, not just instructions). Ask for
  an implementation and you get a reviewed, agreed plan in
  `.mix2/<topic>-plan.md` plus the exact handoff command to run in
  `claude` or `codex`, where you can steer and approve the execution.
- **No project? Still useful.** In a directory that isn't a code project,
  the team switches to general brainstorming — product ideas, business
  viability, strategy — and keeps notes in `.mix2/` when worth keeping.
- One agent (your choice via `--lead`) coordinates the team and owns the
  conversation, running with its normal configuration plus appended mix2
  role instructions. This is an internal mechanic: the UI never labels
  anyone "lead", and every answer speaks as "we".
- The coordinating agent gets one extra shell command,
  **`mix2-consult`**: pipe a prompt in, get the teammate's independent
  written assessment back. `mix2-consult start` returns a ticket
  immediately so both agents research **concurrently**, and
  `mix2-consult wait <ticket>` collects the result; the instructions
  tell the coordinator to fire the consultation first and investigate in
  parallel. It is used for anything substantive and skipped only for
  no-ops; the Rust runtime decides *whether it is allowed* (budget,
  recursion, availability).
- Both roles are told to match depth to the question: quick takes for
  conversational questions, deep review only when asked or when the
  stakes clearly demand it.
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
pnpm dev            # runs mix2 in the current directory
```

The user-facing command is `mix2` (`apps/tui/dist/cli.js`, exposed as a
bin). It launches the internal `mix2-core` runtime itself — users never
interact with the core directly. In development the core is found in
`target/{debug,release}` automatically; a packaged install can point at it
with `MIX2_CORE_BIN`.

```bash
mix2                    # lead from config, else claude
mix2 --lead codex       # short: -l codex
mix2 --cwd ~/src/acme   # run against another project
mix2 --debug            # verbose logs + IPC trace in /tmp
```

## Configuration

`~/.config/mix2/config.toml` (respects `$XDG_CONFIG_HOME`):

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
| `PageUp` / `PageDown`, mouse wheel, `↑`/`↓` (empty composer) | scroll the conversation |
| `Ctrl+Y` | copy the latest answer to the clipboard |
| `Ctrl+Q` | quit |

While you read a long answer, the question it belongs to stays anchored
under the header; it updates as you scroll through history, and clicking
it jumps back to that prompt.

Slash commands: `/exit` (also `/quit`), `/clear` (reset the visible
conversation), `/copy` (copy the latest answer), `/team` (toggle the team
panel), `/help`. Typing `/` shows the available commands in the status
bar. When you're scrolled up, the status bar shows `↓ pgdn latest`.

Copying: **drag-select any conversation text with the mouse and it is
copied the moment you release** — mix2 renders its own selection
highlight and writes the text via OSC 52 (works over SSH) plus the
platform clipboard tool, with a "selection copied" confirmation in the
status bar. `Ctrl+Y` / `/copy` grab the whole latest answer without
touching the mouse. The wheel scrolls the conversation. For your
terminal's *native* selection instead, hold Shift (Option in iTerm2)
while dragging — the standard bypass for mouse-reporting apps.

Answers render markdown natively: headings, bold/italic/inline code,
numbered and bulleted lists with hanging indents, fenced code blocks with
a hairline gutter, and blockquotes.

The team panel (Ctrl+T) shows participants, timings, tool counts, the
consultation exchange, and each teammate consultation's final response.
Hidden model reasoning is never shown anywhere.

## Security model

- mix2 does not touch provider authentication; both CLIs use your
  existing logins. Auth failures surface the provider's own message.
- No permission bypass flags, ever. Claude Code runs with your normal
  permission configuration plus exactly one added allowance:
  `Bash(mix2-consult:*)`, so the lead can reach its teammate.
- Codex as *teammate* runs with your default `codex exec` sandbox
  (read-only). Codex as *lead* runs with Codex's standard
  `workspace-write` sandbox (still no network) — its default read-only
  sandbox blocks the consult channel entirely; this is the one deliberate
  elevation, and it matches what interactive Codex does anyway.
- Recursion (`teammate consulting anyone`) is refused by the runtime.
- Runtime state in `/tmp/mix2/<session>/` contains no credentials and is
  removed on exit. Debug logs never include prompts, file contents, or
  teammate responses.

## Provider requirements

Verified against `claude` 2.1.x (`-p --output-format stream-json`,
`--append-system-prompt`, `--resume`) and `codex-cli` 0.146.x
(`exec --json`, `exec resume`, `-c developer_instructions=…`). Adapters
parse tolerantly: unknown event types from newer CLIs are ignored, never
fatal. If an installed CLI lacks a required capability, mix2 reports a
clear compatibility error at startup.

## Limitations (MVP)

- Two agents, one lead, one teammate; no third agent yet (the agent model
  is designed so `--team codex,gemini` can exist later).
- Teammate consultations are stateless between turns by design.
- Analysis-first: the lead can edit files only where your provider
  permissions already allow it; mix2 does not widen write access.
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
child-process trees for cancellation tests). Point mix2 at them with
`MIX2_CLAUDE_CMD` / `MIX2_CODEX_CMD` — the integration suite drives
the full stack this way, including both consult transports, budget
enforcement, recursion refusal, and process-tree cancellation.

> mix2 is a working name; branding is confined to the crate/package
> names and the header chip, so a rename stays shallow.

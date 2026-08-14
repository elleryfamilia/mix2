# Cladex architecture

```text
        Ink UI  (apps/tui — TypeScript, React, Ink)
            ↕  JSONL over stdin/stdout (protocol v1)
        Rust core  (crates/cladex-core)
          ↙                      ↘
   claude CLI                codex CLI
   (lead or teammate)        (teammate or lead)
```

The TypeScript layer is presentation: rendering, input, interaction state.
The Rust core owns everything with side effects: process management,
sessions, collaboration, budgets, provider parsing, cancellation. No
business logic is duplicated across the boundary, and raw provider JSON
never crosses it.

## Why the lead is the orchestrator

There is no classifier in front of the lead. The user's message goes
straight to the selected lead agent (Claude Code or Codex) with role
instructions appended to the provider's own system prompt. Those
instructions bias hard toward collaboration: users open Cladex to get the
team, so the lead consults on every substantive request and answers alone
only for no-ops (greetings, acknowledgements) and clarifying questions.
The lead still makes that call in context — as part of doing the actual
work — by running the `cladex-consult` command. The runtime's job is to
keep the collaboration *bounded* (budget, depth, cancellation), not to
make the call.

## The consultation path

```text
lead agent (claude -p / codex exec, CLADEX_ROLE=lead)
  └─ runs: cladex-consult <<'CONSULT' … CONSULT
       └─ connects to the runtime:
            1. Unix socket  <runtime>/consult.sock      (Claude's sandbox allows this)
            2. file mailbox <runtime>/consult/req-*.json (fallback; Codex's sandbox
               blocks sockets, so its lead runs workspace-write with the runtime
               dir added to writable roots)
            └─ runtime checks role/depth/budget, then spawns the *other*
               provider fresh, with teammate instructions, same cwd
                 └─ teammate's final response returns through the same
                    channel and is printed on cladex-consult's stdout
```

The lead only ever knows the name `cladex-consult`; the runtime resolves
who the teammate is. When the lead is Claude, consultations go to Codex,
and vice versa.

## Why teammate sessions are independent

Each consultation is a fresh provider session on purpose. The teammate
gets a standalone problem statement, inspects the repository itself, and
answers without seeing the lead's position (the lead instructions
explicitly forbid anchoring). Persistent teammate state would make the
second opinion drift toward the first one — independence is the value.

The lead, by contrast, keeps its native provider conversation across turns
(`--resume` for Claude, `exec resume` for Codex). The provider session id
is captured from the event stream (`system:init` / `thread.started`) and
stored in the Cladex session; a new Cladex session always starts clean and
can never accidentally resume an old provider conversation.

## Why collaboration is budgeted

Two agents consulting each other freely is a token-burning loop with no
convergence guarantee. The runtime owns a per-turn budget (default 2,
`[collaboration] max_consults_per_turn`), acquired atomically so parallel
consult attempts cannot exceed it. Exhausted budget returns an explicit
message telling the lead to resolve the question itself — the turn
continues normally.

## Recursion prevention

Enforced in code, twice:

- `cladex-consult` refuses immediately when `CLADEX_ROLE=teammate` or
  `CLADEX_DEPTH` ≥ 1 (both are set by the runtime when spawning agents).
- The runtime's consult server independently rejects requests whose role
  is `teammate` or whose depth exceeds the maximum, and refuses anything
  outside an active turn.

Prompts additionally tell the teammate not to delegate, but the guarantee
does not depend on the model behaving.

## Provider adapters

`agents/claude.rs` and `agents/codex.rs` hold *all* provider-specific
invocation behavior: flags, prompt transport (stdin in both cases), role
instruction injection (`--append-system-prompt` / `-c
developer_instructions=…`), session resume, and stream parsing. Parsers
are tolerant by contract — unknown event types, unknown item types, and
malformed lines produce warnings, never panics; the shapes were verified
against claude 2.1.x and codex-cli 0.146.x. Reasoning/thinking deltas are
consumed and discarded: hidden chain-of-thought never leaves the adapter.

Adding an agent later means one new adapter plus an `AgentKind` variant;
the collaboration machinery and the UI are provider-neutral (the protocol
already speaks in `lead`/`teammate` roles and semantic events like
`consult`, leaving room for future verbs such as `review` or `challenge`).

## IPC

Newline-delimited JSON on the core's stdin/stdout, versioned
(`protocol: 1` in `initialize`/`ready`; mismatches are fatal at startup).
Commands: `initialize`, `submit`, `cancel`, `shutdown`. Events are
normalized and semantic (`agent.text_delta`, `agent.tool.started`,
`consult.started/completed/failed`, `lead.synthesizing`, `message.final`,
`turn.*`). The UI validates every event with Zod and drops unknown ones,
so core and UI can evolve independently. Attribution is computed in the
core: `message.final.speaker` is `team` only when at least one
consultation actually succeeded that turn.

## Process lifecycle and cancellation

Every provider CLI is spawned in its own Unix process group. Cancelling a
turn cancels a token that:

1. kills the lead's process group (SIGTERM, then SIGKILL) — including any
   shells and helpers the provider spawned,
2. kills any in-flight teammate consultation the same way,
3. ends the turn's consult registration (later requests are refused),
4. emits `turn.cancelled` so the composer returns to ready.

If the UI dies (stdin EOF), the core cancels everything and exits, so no
`claude`/`codex` processes are orphaned. Runtime state lives in
`/tmp/cladex/<session-id>/` (socket + consult mailbox, never credentials)
and is deleted on shutdown. The process-group logic is isolated in
`process/child.rs` so a Windows Job Objects implementation can slot in
behind the same interface.

## Failure model

- Teammate missing/rate-limited/crashed → the *lead* receives a clear
  message from `cladex-consult` and continues; the turn still completes,
  attributed to the lead alone.
- Lead crash → `turn.failed` with the provider's useful stderr; the
  session and composer stay usable.
- Core crash → the UI shows a fatal screen and restores the terminal.
- Malformed provider output → parser warning (visible with `--debug`),
  never a crash.

# mix2 architecture

```text
        Ink UI  (apps/tui — TypeScript, React, Ink)
            ↕  JSONL over stdin/stdout (protocol v1)
        Rust core  (crates/mix2-core)
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
instructions bias hard toward collaboration: users open mix2 to get the
team, so the lead consults on every substantive request and answers alone
only for no-ops (greetings, acknowledgements) and clarifying questions.
The lead still makes that call in context — as part of doing the actual
work — by running the `mix2-consult` command. The runtime's job is to
keep the collaboration *bounded* (budget, depth, cancellation), not to
make the call.

## The scratchpad model

The team's durable output lives in `.mix2/` inside the working directory
— implementation plans, design notes, review findings. It is the only
place the team is told to write, and permissions back that up as far as
each provider allows — no further. Be precise about what that means:
mix2 only *adds* allowances (`Bash(mix2-consult:*)`, `Write(.mix2/**)`,
`Edit(.mix2/**)`); it cannot subtract permissions the user's own Claude
configuration already grants. With stock Claude settings, non-interactive
writes outside `.mix2/` are denied, so the boundary holds; with a
permissive user allowlist, the boundary is instruction-level. Codex leads
run in the workspace-write sandbox (the consult channel requires it), so
for them the rule is always instruction-enforced. Teammates never write; their assessment is their reply. Asked to
implement, the team reframes instead of refusing: it produces the plan in
`.mix2/` and hands the user the exact `claude`/`codex` command to execute
it interactively, where steering and approval exist.

Before both agents commit to a broad request, the lead is instructed to
qualify it once — scope, focus, deliverable, at most three questions —
because a consultation costs real minutes. Specific requests skip the
round trip. The core also reports whether the cwd looks like a software
project (git or a build manifest); when it doesn't, both role prompts
drop the code lens and treat the session as general brainstorming.

The "lead" is deliberately invisible to the user: the UI shows one team
roster, every answer carries the Team chip, and the role instructions
require the first-person-plural voice in the answer. Progress text the lead
writes between tool calls is a different register: third-person narration
that names both agents ("Codex is reading the doc; Claude is extracting the
text"), which the UI shows under the app's ` mix2 ` chip — the harness
narrating, not the lead speaking. Which agent coordinates is a config
choice, not a product concept.

## The consultation path

```text
lead agent (claude -p / codex exec, MIX2_ROLE=lead)
  └─ runs: mix2-consult <<'CONSULT' … CONSULT           (blocking)
     or:   mix2-consult start … → ticket                (concurrent)
           …keeps researching while the teammate works…
           mix2-consult wait <ticket> → assessment
       └─ connects to the runtime:
            1. Unix socket  <runtime>/consult.sock      (Claude's sandbox allows this)
            2. file mailbox <runtime>/consult/req-*.json (fallback; Codex's sandbox
               blocks sockets, so its lead runs workspace-write with the runtime
               dir added to writable roots)
            └─ runtime checks role/depth/budget, then spawns the *other*
               provider fresh, with teammate instructions, same cwd
                 └─ teammate's final response returns through the same
                    channel and is printed on mix2-consult's stdout
```

The lead only ever knows the name `mix2-consult`; the runtime resolves
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
stored in the mix2 session; a new mix2 session always starts clean and
can never accidentally resume an old provider conversation.

## Why collaboration is budgeted

Two agents consulting each other freely is a token-burning loop with no
convergence guarantee. The runtime owns a per-turn budget (default 2,
`[collaboration] max_consults_per_turn`), acquired atomically so parallel
consult attempts cannot exceed it. Exhausted budget returns an explicit
message telling the lead to resolve the question itself — the turn
continues normally.

## Recursion prevention

Enforced in code, in layers:

- **Capability token (the real authorization):** each turn mints a token
  injected only into the coordinator's environment
  (`MIX2_CONSULT_TOKEN`). Every consult request — sync, start, and wait —
  must present it; a teammate (or any other process) forging
  `role=lead, depth=0` is refused because it never received the token.
- `mix2-consult` refuses immediately when `MIX2_ROLE=teammate` or
  `MIX2_DEPTH` ≥ 1 — a fast, friendly refusal before any transport.
- The consult server independently rejects teammate-role requests,
  excessive depth, and anything outside an active turn, and every
  consultation event is scoped to its originating turn's UUID, so a late
  result from an abandoned consultation can never credit a later turn.

Prompts additionally tell the teammate not to delegate, but the
authorization does not rest on the model honoring them.

## Provider adapters: slots, descriptors, and the registry

Participant identity is the **slot** (`SlotId::{One,Two}`) — sessions,
IPC events, `/model` targeting, disagreement stances, and the TUI's
colors/glyphs all key on it. Which CLI backs a slot is the separately
chosen `HarnessKind`, and the same harness on both slots is a supported
team (the UI names them "Codex (one)" / "Codex (two)").

Each harness is a **descriptor plus a decoder** (`agents/{claude,codex,
cursor,opencode,copilot}.rs`): metadata and hints, the default binary
and its env override, structured capability facts, a pure argv builder,
version/auth probes, an optional live model-listing command, and a
tolerant stream decoder. One shared runner (`agents/runner.rs`)
implements the `Agent` trait exactly once — spawn, stream decode,
cancellation tree-kill, stderr tails, failure shaping — and
`agents/registry.rs` is the single plug-in point: a new harness is a
descriptor, a decoder, and one registry arm; the runtime, config, and
UI stay untouched. Decoders emit identity-free events; the runtime
stamps the slot from the channel an event arrived on, so a same-harness
teammate can never be mislabeled.

Parsers are tolerant by contract — malformed lines produce warnings and
unknown event types are ignored (never panics); shapes were verified
against claude 2.1.x, codex-cli 0.146.x, cursor-agent 2026.08,
opencode 1.16.x, and copilot 1.0.x. Reasoning/thinking output is
consumed and discarded: hidden chain-of-thought never leaves the
adapter.

**Capabilities are facts, not booleans** (`enforced` / `unverified` /
`unsupported`): teammate read-only enforcement, lead permission
scoping, and instruction injection. Role eligibility derives from them
— a role closes only on `unsupported` — which is why Claude and Codex
are the lead-capable pair while Cursor (`--mode plan`, but no verified
lead write-scoping), OpenCode (read-only `plan` agent), and Copilot
(write/shell denied by documented precedence, personal MCP servers
still load — disclosed at selection) consult as teammates.

**Discovery and selection.** Startup probes every candidate
`(harness, command)` pair once — version and sign-in, in parallel,
quota-free, under a strict timeout, cached — and reports
`harnesses.discovered` with the configured proposal. An explicit
`[slot.*]` config (or a non-interactive session) auto-confirms;
otherwise the TUI's team picker settles the team via `select_team`,
with invalid selections refused actionably and retryable. Auth is
five-state (`authenticated`, `unauthenticated`, `configured` — a
credential inventory, `unsupported` — no probe exists, `probe_failed`);
only an explicit `unauthenticated` ever blocks.

## IPC

Newline-delimited JSON on the core's stdin/stdout, versioned
(`protocol: 3` in `initialize`/`ready`; mismatches are fatal at startup).
Commands: `initialize`, `select_team`, `submit`, `cancel`, `set_model`,
`shutdown`. Events are
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
`/tmp/mix2/<session-id>/` (socket + consult mailbox, never credentials)
and is deleted on shutdown. The process-group logic is isolated in
`process/child.rs` so a Windows Job Objects implementation can slot in
behind the same interface.

## Failure model

- Either agent missing or signed out at startup → fatal. There is no
  solo mode — mix2 *is* the two-agent team. The startup check probes
  both CLIs (version + each CLI's quota-free auth status command) and
  the fatal screen lists per-agent status with the exact install or
  sign-in fix.
- Teammate rate-limited/crashed *mid-turn* → the *lead* receives a clear
  message from `mix2-consult` and continues; the turn still completes,
  attributed to the lead alone. (Runtime failures degrade a turn;
  only startup readiness is a hard gate.)
- Lead crash → `turn.failed` with the provider's useful stderr; the
  session and composer stay usable.
- Core crash → the UI shows a fatal screen and restores the terminal.
- Malformed provider output → parser warning (visible with `--debug`),
  never a crash.

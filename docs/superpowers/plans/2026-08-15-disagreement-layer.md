# Disagreement Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render genuine lead/teammate splits as a `△ where we split` stance block in the final answer, a ledger in the ctrl+t panel, and a status-bar count — recorded honestly through a new `mix2-consult disagree` runtime verb.

**Architecture:** The lead records the split server-side via the existing consult channel (socket/mailbox + capability token). The ConsultServer validates it (gate: ≥1 completed consultation this turn), stores it on the ActiveTurn, and `end_turn()` atomically hands it to `finish_turn`, which attaches it to `message.final`. A `disagreement.recorded` event feeds only the live team panel. The TUI renders exclusively from structured data — never parsed from prose.

**Tech Stack:** Rust (tokio, serde) in `crates/mix2-core`; TypeScript (Ink 7, React 19, zod, vitest) in `apps/tui`.

**Spec:** `docs/superpowers/plans/2026-08-15-disagreement-layer-spec.md` — read it first; every task below argues from it.

## Global Constraints

- Do NOT bump `PROTOCOL_VERSION` (`crates/mix2-core/src/ipc/mod.rs:8`). New wire surface is one optional field on `message.final` plus one new event type; both are ignorable by old peers.
- `crates/mix2-core/src/bin/mix2_consult.rs` stays std + serde_json only — no mix2-core lib import (protects the static-musl release build, commit 8226df1).
- `collaboration/disagreement.rs` stays dependency-free (serde + std only).
- Honesty invariants: no disagreement renders without a server-validated record; cancelled/failed turns never show one; positions are never silently mutated (refuse, don't truncate, in core; truncate only visibly with `…` in the TUI).
- The repo is currently on a detached HEAD; create the work branch first (Task 1, Step 1). Conventional Commits; commit at the end of every task.
- Verification gate for TS tasks: `pnpm --filter mix2 test` (vitest) and `pnpm --filter mix2 typecheck`; for Rust tasks: `cargo test -p mix2-core` and `cargo clippy --all-targets --all-features -- -D warnings`. Final gate: `pnpm check`.
- New TS runtime dependency `string-width` (Task 8) — flagged: get user approval before `pnpm add`; it is already in the tree transitively via Ink.

---

### Task 1: Grammar + types (`collaboration/disagreement.rs`)

**Files:**
- Create: `crates/mix2-core/src/collaboration/disagreement.rs`
- Modify: `crates/mix2-core/src/collaboration/mod.rs` (add `pub mod disagreement;`)

**Interfaces:**
- Produces: `Outcome { Chosen, Deferred, Dropped }` (serde kebab-case), `Stance { agent: AgentKind, position: String, outcome: Outcome }`, `DisagreementRecord { stances: Vec<Stance>, resolution: String }`, `pub const DISAGREE_EXAMPLE: &str`, `pub fn parse(text: &str, lead: AgentKind, teammate: AgentKind) -> Result<DisagreementRecord, String>`, `pub fn refusal(err: &str) -> String` (err + blank line + DISAGREE_EXAMPLE + retry stop-condition sentence).

- [ ] **Step 1: Write failing tests** in the module's `#[cfg(test)]`:

```rust
#[test]
fn parses_canonical_block() {
    let text = "claude: cache the compiled schema in-process | chosen\n\
                codex: move validation off the hot path | deferred\n\
                team: ship the cache now; file the rework as a follow-up";
    let r = parse(text, AgentKind::Claude, AgentKind::Codex).unwrap();
    assert_eq!(r.stances.len(), 2);
    assert_eq!(r.stances[0].outcome, Outcome::Chosen);
    assert_eq!(r.stances[1].outcome, Outcome::Deferred);
    assert!(r.resolution.starts_with("ship the cache"));
}

#[test]
fn splits_on_last_pipe_and_accepts_display_names() {
    let text = "Claude: use `string | null` in the schema | chosen\n\
                Codex: keep the alias | dropped\n\
                team: go with the union";
    let r = parse(text, AgentKind::Claude, AgentKind::Codex).unwrap();
    assert_eq!(r.stances[0].position, "use `string | null` in the schema");
}

#[test]
fn accepts_outcome_synonyms() {
    let text = "claude: a | shipped\ncodex: b | follow-up\nteam: call";
    let r = parse(text, AgentKind::Claude, AgentKind::Codex).unwrap();
    assert_eq!(r.stances[0].outcome, Outcome::Chosen);
    assert_eq!(r.stances[1].outcome, Outcome::Deferred);
}

#[test]
fn folds_extra_lines_into_resolution_with_cap() {
    let text = format!("claude: a | chosen\ncodex: b | deferred\nteam: first.\n{}", "word ".repeat(100));
    let r = parse(&text, AgentKind::Claude, AgentKind::Codex).unwrap();
    assert!(r.resolution.chars().count() <= 300);
    assert!(r.resolution.ends_with('…'));
}

#[test]
fn rejects_missing_team_line() {
    assert!(parse("claude: a | chosen\ncodex: b | deferred", AgentKind::Claude, AgentKind::Codex).is_err());
}

#[test]
fn rejects_unknown_outcome_naming_the_tail() {
    let err = parse("claude: a | maybe\ncodex: b | deferred\nteam: c", AgentKind::Claude, AgentKind::Codex).unwrap_err();
    assert!(err.contains("maybe"));
}

#[test]
fn rejects_identical_positions() {
    let err = parse("claude: Use the cache | chosen\ncodex: use  the cache | deferred\nteam: c", AgentKind::Claude, AgentKind::Codex).unwrap_err();
    assert!(err.contains("not a split"));
}

#[test]
fn rejects_overlong_position() {
    let text = format!("claude: {} | chosen\ncodex: b | deferred\nteam: c", "x".repeat(201));
    assert!(parse(&text, AgentKind::Claude, AgentKind::Codex).unwrap_err().contains("too long"));
}

#[test]
fn rejects_wrong_and_duplicate_agents() {
    assert!(parse("gemini: a | chosen\ncodex: b | deferred\nteam: c", AgentKind::Claude, AgentKind::Codex).is_err());
    assert!(parse("claude: a | chosen\nclaude: b | deferred\nteam: c", AgentKind::Claude, AgentKind::Codex).is_err());
}

#[test]
fn example_constant_parses() {
    let body: String = DISAGREE_EXAMPLE.lines()
        .filter(|l| !l.starts_with("mix2-consult") && *l != "SPLIT")
        .collect::<Vec<_>>().join("\n");
    parse(&body, AgentKind::Claude, AgentKind::Codex).unwrap();
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p mix2-core disagreement` → FAIL (module/function undefined).
- [ ] **Step 3: Implement.** Line-by-line over trimmed non-empty lines. `split_once(':')` → name; name matching: lowercase, compare against each session kind's `as_str()` and `display_name().to_lowercase()`, plus literal `team`. Agent lines: `rsplit_once('|')` → (position, outcome word); outcome normalization: trim, lowercase, strip trailing `.`, spaces→`-`; map via the synonym table; unknown → `Err(format!("unknown outcome '{tail}' — use chosen, deferred, or dropped"))`. `team` line starts the resolution; subsequent non-agent lines append with a space; cap at 300 chars at a word boundary appending `…`. Post-checks in order: exactly one stance per session agent (missing → "each agent needs exactly one line"; duplicate → same); empty position → error; position `chars().count() > 200` → "position too long — restate it in one line"; normalized-identical positions (lowercase, whitespace collapsed) → "both positions are the same — that's not a split; disclose the nuance in prose instead"; empty resolution → error. `DISAGREE_EXAMPLE` is the four-line heredoc invocation shown in the spec. `refusal(err)` = `format!("{err}\n\nExample:\n{DISAGREE_EXAMPLE}\n\nIf this fails twice, skip recording and state the disagreement in prose.")`.
- [ ] **Step 4: Run tests** — `cargo test -p mix2-core disagreement` → PASS.
- [ ] **Step 5: Commit** — `feat(core): disagreement grammar and record types`

---

### Task 2: Prompt rewrite + drift guard (`prompts.rs`)

**Files:**
- Modify: `crates/mix2-core/src/collaboration/prompts.rs:62-74` (the numbered consult rules, specifically rule 7) and its tests.

**Interfaces:**
- Consumes: `disagreement::DISAGREE_EXAMPLE` (Task 1).

- [ ] **Step 1: Write failing tests** (in prompts.rs test module):

```rust
#[test]
fn lead_prompt_teaches_the_disagree_verb() {
    let p = lead_instructions(AgentKind::Claude, AgentKind::Codex, true);
    assert!(p.contains("mix2-consult disagree"));
    assert!(p.contains(crate::collaboration::disagreement::DISAGREE_EXAMPLE));
    assert!(p.contains("at most one sentence"));
}

#[test]
fn teammate_prompt_does_not_teach_disagree() {
    let p = teammate_instructions(AgentKind::Claude, AgentKind::Codex, true);
    assert!(!p.contains("mix2-consult disagree"));
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p mix2-core prompts` → FAIL.
- [ ] **Step 3: Implement.** Replace rule 7's text ("Never manufacture consensus. If an important disagreement remains, disclose it: state each agent's position...") with (interpolating `{example}` = DISAGREE_EXAMPLE via the existing `format!`):

```text
7. Never manufacture consensus. If an important disagreement remains after
   reconciliation, record it BEFORE writing your final answer:

{example}

   One line per agent — `<agent>: <one-line position> | <outcome>` with outcome
   `chosen`, `deferred`, or `dropped` — then a `team:` line stating the call.
   The interface renders the recorded split beside your answer, so in your
   prose mention the disagreement in at most one sentence (the team's call);
   the rest of your answer is unaffected. Recording requires a completed
   consultation this turn; record only after you have read your teammate's
   assessment, and only a genuine split that survived reconciliation —
   recording agreement as disagreement is exactly as dishonest as manufactured
   consensus. If the command refuses or fails twice, skip recording and
   disclose the disagreement in prose instead.
```

- [ ] **Step 4: Run** — `cargo test -p mix2-core prompts` → PASS (existing prompt tests must also still pass).
- [ ] **Step 5: Commit** — `feat(core): teach the lead to record disagreements`

---

### Task 3: Helper verb (`mix2_consult.rs`)

**Files:**
- Modify: `crates/mix2-core/src/bin/mix2_consult.rs`

**Interfaces:**
- Produces (wire): request JSON gains `"mode": "disagree"` and `"disagreement_text": "<raw stdin>"`. No parsing in the helper.

- [ ] **Step 1:** Extend `enum Mode` with `Disagree` and the arg match (`mix2_consult.rs:82-92`) with `Some("disagree") => Mode::Disagree`. Read stdin for it exactly as Sync does (reuse the existing stdin branch; empty stdin → local error "Nothing to record. Pipe the split on stdin, e.g.\nmix2-consult disagree <<'SPLIT'\n...\nSPLIT"). In the request JSON (`mix2_consult.rs:126-135`): `"mode"` maps `Mode::Disagree => "disagree"`; add `"disagreement_text"` carrying the raw text (prompt stays empty for this mode); `"prompt"` requirement must not apply. Transport untouched: socket first, then `try_files` (Disagree is NOT a Wait — it must go through `try_files`, not `poll_done_file`). Success path: server's `text` field prints as-is; refusal path: existing exit-2 handling already prints `error`.
- [ ] **Step 2:** Build check — `cargo build -p mix2-core --bins`; confirm no new `use` of the mix2-core lib appears in the file (global constraint).
- [ ] **Step 3:** Manual smoke (no server): `echo hi | MIX2_RUNTIME_DIR=/nonexistent target/debug/mix2-consult disagree` → exits 2 with the unreachable-runtime message.
- [ ] **Step 4: Commit** — `feat(core): mix2-consult disagree transport mode`

(Behavioral tests for this verb live in Task 5's integration scenarios — the helper is transport-only.)

---

### Task 4: Server-side recording (`consult.rs`)

**Files:**
- Modify: `crates/mix2-core/src/collaboration/consult.rs`

**Interfaces:**
- Consumes: `disagreement::{parse, refusal, DisagreementRecord}` (Task 1).
- Produces: `ActiveTurn` gains `completed_consults: Arc<AtomicU32>` and `disagreement: Arc<StdMutex<Option<(DisagreementRecord, u32)>>>` (revision starts at 1); `ConsultRequest` gains `#[serde(default)] disagreement_text: Option<String>`; new `ConsultUpdate::DisagreementRecorded { turn_id: Uuid, record: DisagreementRecord, revision: u32 }`; `ConsultServer::end_turn()` signature becomes `pub async fn end_turn(&self) -> Option<DisagreementRecord>`.

- [ ] **Step 1: Write failing tests.** consult.rs has in-crate unit tests around `handle_request`; follow the existing test-construction pattern there (build a `Shared` with a stub teammate, begin a turn with a known token). Cases:

```rust
// names indicative; adapt to the file's existing test helpers
#[tokio::test] async fn disagree_refused_without_completed_consult() // gate message verbatim
#[tokio::test] async fn disagree_records_after_completed_consult()   // returns ok:true, update emitted, revision 1
#[tokio::test] async fn disagree_identical_rerecord_is_idempotent()  // ok, still revision 1, no second update
#[tokio::test] async fn disagree_distinct_rerecord_bumps_revision_and_caps_at_3()
#[tokio::test] async fn disagree_requires_token_and_valid_agents()
#[tokio::test] async fn end_turn_returns_the_record_and_then_refuses_new_ones()
#[tokio::test] async fn disagree_parse_error_refusal_embeds_example()
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p mix2-core consult` → FAIL.
- [ ] **Step 3: Implement.**
  - `ActiveTurn` fields as above (`use std::sync::{atomic::AtomicU32, Mutex as StdMutex}`); construction site in runtime.rs updated by Task 5 (compiler will point at it — initialize with `Arc::new(AtomicU32::new(0))` / `Arc::new(StdMutex::new(None))`).
  - In the spawned consult task (consult.rs:431-476): capture the turn's `completed_consults` Arc when the request is admitted (same place `budget` is cloned, consult.rs:382-393); on the `Ok` branch increment with `fetch_add(1, Ordering::SeqCst)` BEFORE the `updates.send(Completed…)`, done-file write, and `result_tx.send`.
  - New branch in `handle_request` after the mode validation (consult.rs:375): `if mode == "disagree"`. Under a single `shared.active.read().await` guard held for the whole block (no awaits inside): token check (same refusal as consults); gate `completed_consults.load(SeqCst) >= 1` else `refuse("no completed consultation this turn — disclose the disagreement in prose instead.")`; `parse(&request.disagreement_text.unwrap_or_default(), shared.lead_kind, shared.teammate_kind)` — `Err(e)` → `refuse(refusal(&e))`; revision transaction under the `StdMutex`: equal record → respond ok (idempotent, no update); different + revision ≥ 3 → `refuse("revision limit reached — the earlier record stands; note any change in prose.")`; else store `(record, rev+1)` (or `(record, 1)` when None). Clone `(record, revision)` out, drop both guards, THEN `shared.updates.send(ConsultUpdate::DisagreementRecorded{…}).await`. Respond `ok: true` with `text: Some("Recorded — the interface renders the split beside your answer. In your final answer, cover the disagreement itself in at most one sentence (the team's call); the rest of your answer is unaffected.")`.
  - `end_turn` (consult.rs:232): take the ActiveTurn under the write lock, pull the record out of its `StdMutex`, keep the existing pending-clear and mailbox sweep, return `Option<DisagreementRecord>`.
- [ ] **Step 4: Run** — `cargo test -p mix2-core consult` → PASS; `cargo clippy --all-targets --all-features -- -D warnings` clean (watch: no await while holding the read guard or the StdMutex).
- [ ] **Step 5: Commit** — `feat(core): record disagreements through the consult server`

---

### Task 5: Runtime + events + integration (`runtime.rs`, `events.rs`, fixtures)

**Files:**
- Modify: `crates/mix2-core/src/runtime.rs` (ActiveTurn construction ~:309-311 area where the turn begins; `update_turn_id` :36; `handle_consult_update` :499-524; `finish_turn` :527-570)
- Modify: `crates/mix2-core/src/ipc/events.rs`
- Modify: `tests/fixtures/fake-claude` (new scenarios), `crates/mix2-core/tests/integration.rs`

**Interfaces:**
- Consumes: Task 4's `ConsultUpdate::DisagreementRecorded`, `end_turn() -> Option<DisagreementRecord>`.
- Produces (wire): new event `{"type":"disagreement.recorded","turn_id":…,"stances":[{"agent":"claude","position":…,"outcome":"chosen"},…],"resolution":…,"revision":1}`; `message.final` gains optional `"disagreement":{"stances":[…],"resolution":…}`.

- [ ] **Step 1: Failing serialization tests** in events.rs (mirror `event_serialization_shape`): `disagreement_recorded_shape` asserting the exact JSON above; `message_final_omits_disagreement_when_none` asserting the key is absent; `message_final_disagreement_round_trips`.
- [ ] **Step 2:** `cargo test -p mix2-core events` → FAIL.
- [ ] **Step 3: Implement events.** In events.rs: `Event::DisagreementRecorded { turn_id: String, stances: Vec<Stance>, resolution: String, revision: u32 }` with `#[serde(rename = "disagreement.recorded")]`; `MessageFinal` gains `#[serde(skip_serializing_if = "Option::is_none")] disagreement: Option<DisagreementRecord>` (re-export the types from `collaboration::disagreement`). `turn.completed` unchanged.
- [ ] **Step 4: Implement runtime.** New arm in `update_turn_id` (:36) and in `handle_consult_update`: emit the event mapping Uuid→`turn.ui_id` like the existing arms. In `finish_turn` (:527): `let disagreement = self.consult_server.end_turn().await;` replaces the bare call; attach `disagreement` to `MessageFinal` in the `Ok` non-cancelled arm only (cancelled/failed paths drop it). Fix the `ActiveTurn` construction for the new fields.
- [ ] **Step 5:** `cargo test -p mix2-core events runtime` → PASS.
- [ ] **Step 6: Fixture scenarios.** In `tests/fixtures/fake-claude` add, following the existing consult subprocess pattern (:124-174):
  - `SCENARIO:disagree` — run `mix2-consult` sync once, then `mix2-consult disagree` with input `"claude: cache in-process | chosen\ncodex: move validation off the hot path | deferred\nteam: ship the cache; file the rework\n"`, capture its exit code and stdout; final text appends `[disagree:<exitcode>]`.
  - `SCENARIO:disagree_solo` — run `mix2-consult disagree` (same input) WITHOUT consulting first; final text appends `[disagree:<exitcode>:<first line of stdout>]`.
- [ ] **Step 7: Failing integration tests** in integration.rs (existing Core harness):

```rust
#[test]
fn disagreement_flows_to_message_final() {
    let mut core = Core::start(CoreOptions::default());
    core.submit("t1", "SCENARIO:disagree p99 question");
    let events = core.events_until("turn.completed", LONG);
    assert!(events.iter().any(|e| e["type"] == "disagreement.recorded"));
    let fin = events.iter().find(|e| e["type"] == "message.final").unwrap();
    assert_eq!(fin["disagreement"]["stances"].as_array().unwrap().len(), 2);
    assert!(fin["text"].as_str().unwrap().contains("[disagree:0]"));
}

#[test]
fn disagree_without_consult_is_refused() {
    let mut core = Core::start(CoreOptions::default());
    core.submit("t1", "SCENARIO:disagree_solo attempt");
    let events = core.events_until("turn.completed", LONG);
    assert!(!events.iter().any(|e| e["type"] == "disagreement.recorded"));
    let fin = events.iter().find(|e| e["type"] == "message.final").unwrap();
    assert!(fin.get("disagreement").is_none());
    assert!(fin["text"].as_str().unwrap().contains("[disagree:2:"));
}
```

- [ ] **Step 8:** `cargo test -p mix2-core --test integration disagree` → PASS.
- [ ] **Step 9: Commit** — `feat(core): emit recorded disagreements on the wire`

---

### Task 6: TUI protocol (`protocol.ts`)

**Files:**
- Modify: `apps/tui/src/ipc/protocol.ts`, `apps/tui/src/ipc/protocol.test.ts`

**Interfaces:**
- Produces: `export type StanceOutcome = 'chosen' | 'deferred' | 'dropped'`; `export type Stance = { agent: 'claude' | 'codex'; position: string; outcome: StanceOutcome }`; `export type Disagreement = { stances: Stance[]; resolution: string }` (z.infer); `message.final` schema gains `disagreement: disagreementSchema.optional()`; new event schema `disagreement.recorded` with `turn_id, stances, resolution, revision: z.number()`.

- [ ] **Step 1: Failing tests:** `parses disagreement.recorded`; `parses message.final with disagreement payload`; `parses message.final without the field (old core)` — construct JSON lines and assert `parseEventLine` output shape.
- [ ] **Step 2:** `pnpm --filter mix2 test protocol` → FAIL.
- [ ] **Step 3: Implement** the schemas next to the existing consult schemas (protocol.ts:67-116).
- [ ] **Step 4:** test + `pnpm --filter mix2 typecheck` → PASS.
- [ ] **Step 5: Commit** — `feat(tui): disagreement wire types`

---

### Task 7: Store reducer (`store.ts`)

**Files:**
- Modify: `apps/tui/src/state/store.ts`, `apps/tui/src/state/store.test.ts`

**Interfaces:**
- Consumes: Task 6 types.
- Produces: `export interface DisagreementState { stances: Stance[]; resolution: string; revision: number }`; `ActiveTurn.disagreement?: DisagreementState`; `ConversationItem` 'final' variant gains `disagreement?: Disagreement`; `TurnRecord.disagreement?: Disagreement`; `lastSummary` becomes `{ durationMs: number; consultations: number; disagreements: number }`.

- [ ] **Step 1: Failing tests** (existing reducer-test style: fold events through `apply`):
  - `disagreement.recorded attaches to the live turn`;
  - `stale revision is ignored` (rev 2 then rev 1 → still rev 2);
  - `message.final payload lands on the final item and overwrites live state`;
  - `turn.completed carries it into lastTurn and lastSummary.disagreements === 1`;
  - `absent payload yields disagreements === 0`;
  - `turn.cancelled clears it and lastTurn carries none`;
  - `turn.failed clears it and lastTurn carries none`.
- [ ] **Step 2:** `pnpm --filter mix2 test store` → FAIL.
- [ ] **Step 3: Implement.** New case `'disagreement.recorded'`: guard `turn && turn.id === event.turn_id`; ignore when `event.revision <= (turn.disagreement?.revision ?? 0)`; else set `{stances, resolution, revision}`. In `'message.final'` (store.ts:388-411): the final item gains `disagreement: event.disagreement`; return `turn: { ...turn, disagreement: event.disagreement ? { ...event.disagreement, revision: turn.disagreement?.revision ?? 1 } : undefined }` (authoritative overwrite, absent clears). In `'turn.completed'` (:413): `lastSummary: { durationMs, consultations, disagreements: turn.disagreement ? 1 : 0 }`. `recordTurn(turn, now, outcome)` copies `disagreement` only when `outcome === 'completed'` (strip stale revision into the plain `Disagreement` shape).
- [ ] **Step 4:** tests + typecheck → PASS.
- [ ] **Step 5: Commit** — `feat(tui): track recorded disagreements in state`

---

### Task 8: Stance block renderer (`conversation.ts` + width helper)

**Files:**
- Modify: `apps/tui/package.json` (add `string-width` — **needs user approval first**), `apps/tui/src/render/lines.ts`, `apps/tui/src/render/conversation.ts`, `apps/tui/src/render/conversation.test.ts`

**Interfaces:**
- Consumes: Task 7's final-item `disagreement` field; `glyphs.disagree` (theme.ts:50), `agentGlyph`/`agentColor`, `spread`/`pad`/`span`.
- Produces: `export function displayWidth(text: string): number` and `export function truncateDisplay(text: string, max: number): string` in lines.ts (string-width-backed, `…` suffix); `stanceLines(d: Disagreement, ctx: RenderContext): Line[]` in conversation.ts, consumed by `finalLines`.

- [ ] **Step 1:** `pnpm --filter mix2 add string-width` (after approval).
- [ ] **Step 2: Failing tests:**
  - stance block renders after the answer body: header `△ where we split` present, one row per stance plus the team row;
  - arrow labels right-aligned at width 92 (`← shipped` line ends at the content edge; assert via rendered text padding);
  - long position truncates with `…` and the row width never exceeds the frame at 80 and 50 cols;
  - CJK position (`"キャッシュを使う"`) still fits the frame at 50 cols (displayWidth, not .length);
  - final item without `disagreement` renders no block.
- [ ] **Step 3:** run → FAIL.
- [ ] **Step 4: Implement.** In `finalLines` (conversation.ts:104-126), after the markdown loop: `if (item.disagreement) { lines.push(BLANK); lines.push(...stanceLines(item.disagreement, ctx)); }`. `stanceLines`: width `min(ctx.width, MAX_CONTENT_WIDTH)`; header `pad(span(glyphs.disagree + ' where we split', { color: theme.agent.team, bold: true }))`; per stance: `spread(pad(span(agentGlyph(a), {color: agentColor(a)}), span(' ' + a.padEnd(7), {color: agentColor(a), bold: true}), span(truncateDisplay(position, room), {color: theme.text.secondary})), [span(arrow, {color: theme.text.faint})], width)` where `room` = width − INDENT − 2 (glyph+space) − 8 (padded name) − displayWidth(arrow) − 2, and arrow = `← shipped` (chosen) / `→ follow-up` (deferred) / `→ set aside` (dropped); team row: `pad(span(glyphs.team, mauve), span(' team   ', mauve bold), span(truncateDisplay(resolution, …), secondary))`. Switch only this renderer's measurements to `displayWidth`/`truncateDisplay` (TUI-wide adoption is a documented follow-up, spec §TUI).
- [ ] **Step 5:** tests + typecheck → PASS.
- [ ] **Step 6: Commit** — `feat(tui): render the where-we-split stance block`

---

### Task 9: Team-panel ledger (`teamPanel.ts`)

**Files:**
- Modify: `apps/tui/src/render/teamPanel.ts`, `apps/tui/src/render/teamPanel.test.ts`

**Interfaces:**
- Consumes: `state.turn?.disagreement` (live) and `state.lastTurn?.disagreement` (settled) from Task 7.

- [ ] **Step 1: Failing tests:** ledger renders while live (feed `disagreement.recorded`, no final yet); ledger renders settled (after turn.completed); full position is NOT truncated (long position wraps to a second line); ledger renders even when the consult list is empty (defensive — construct the state directly); no ledger without a record.
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3: Implement.** Extend the live `record` object (teamPanel.ts:52-58) with `disagreement: state.turn.disagreement`; settled path already carries it via TurnRecord. Insert the ledger AFTER the consultation-count line (:108-116) and BEFORE the zero-consult early return (:118-122): blank line, header `pad(span('△ disagreement', { color: theme.agent.team, bold: true }))`, then per stance the glyph+name line followed by `wrapText(position, contentW - 2)` indented lines in secondary and a faint outcome line (`← shipped` etc.), then `◐ team` + wrapped resolution.
- [ ] **Step 4:** tests + typecheck → PASS.
- [ ] **Step 5: Commit** — `feat(tui): disagreement ledger in the team panel`

---

### Task 10: Status-bar count (`StatusBar.tsx`)

**Files:**
- Modify: `apps/tui/src/components/StatusBar.tsx:80-90`, its test file (or add cases to the existing App/StatusBar suite next to it).

- [ ] **Step 1: Failing tests:** done summary shows `△ 1 disagreement` when `lastSummary.disagreements === 1`; shows nothing extra when 0; idle/working states unchanged.
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3: Implement.** In the `lastSummary` branch (:80-90): `const splitNote = disagreements > 0 ? ` ${glyphs.dot} ${glyphs.disagree} ${disagreements} disagreement${disagreements === 1 ? '' : 's'}` : '';` appended after `consultNote`.
- [ ] **Step 4:** tests + typecheck → PASS.
- [ ] **Step 5: Commit** — `feat(tui): disagreement count in the done summary`

---

### Task 11: Docs, full gate, wrap-up

**Files:**
- Modify: `docs/design-system.md` (:235-245 stance-block section; :263-275 status-bar table; team-panel section :277+)

- [ ] **Step 1:** Rewrite the stance-block paragraph: it is no longer "part of the lead's own final text, taught not enforced" — it renders from the runtime's validated record (`mix2-consult disagree`), with the exact block shape and arrows; add the `· △ 1 disagreement` done-state row to the status-bar table; document the team-panel ledger section.
- [ ] **Step 2:** Full gate: `pnpm check` (typecheck, vitest, cargo fmt --check, clippy -D warnings, cargo test). Fix anything it surfaces.
- [ ] **Step 3:** Commit — `docs: disagreement layer in the design system`.
- [ ] **Step 4:** Report results honestly: paste the gate output summary; list any skipped/failing item plainly.

---

## Self-review (performed at plan-writing time)

- Spec coverage: stance block → Tasks 1,4,5,8; ledger → 9; counts → 7,10; honesty gates/atomicity → 4,5; prompt → 2; drift guard → 1 (`example_constant_parses`) + 2; degradation → 5 (`disagree_solo`), 7 (cancel/fail), 8 (absent field). No spec section is uncovered.
- Type consistency: `Outcome`/`StanceOutcome` kebab-case values match across Rust serde and zod (`chosen|deferred|dropped`); `DisagreementRecord { stances, resolution }` ↔ zod `disagreementSchema { stances, resolution }`; revision lives only on the event + live state, never on the settled payload.
- Placeholders: none — every step names files, code, and expected output.

# Disagreement layer — design spec (v3, final)

Provenance: implements panels 4b/4c of the locked visual design (claude.ai/design
project "Cladex Directions", turn 4). Architecture chosen from three explored
candidates, revised through an internal critic pass and two codex (gpt-5.6-sol)
cross-model review rounds; v3 incorporates every non-waived finding.

## Goal

When the lead and teammate genuinely split on a recommendation, the TUI shows:

1. **Stance block** appended to the final Team answer:

```
  △ where we split
  ● claude   cache compiled schema in-process        ← shipped
  ○ codex    move validation off the hot path        → follow-up
  ◐ team     lead's call — ship now, file the rework
```

   Mauve `△` header (`glyphs.disagree`, theme.ts:50, currently unused); per-row
   agent glyph+color; right-aligned outcome arrows; positions truncated with a
   visible `…` (full text lives in the team panel).

2. **Ledger** in the ctrl+t team panel: full untruncated positions, wrapped;
   shown live and settled; scope is the current/last turn (matches the panel's
   existing "this run" scope). Must render even when the consult list is empty
   (the early return at teamPanel.ts:118 may not skip it).

3. **Count**: `· △ 1 disagreement` appended to the status-bar done summary.

**Honesty rule:** never render a disagreement that did not happen; degrade to
prose-only when the model doesn't comply; never parse disagreement structure
out of prose text.

## Mechanism

The lead records the split as a first-class runtime action through the existing
`mix2-consult` channel (JSON line over `$MIX2_RUNTIME_DIR/consult.sock`,
file-mailbox fallback, per-turn capability token).

Rejected alternatives: (A) prompt-taught textual convention parsed by the TUI
from final text — quoted-transcript false positives violate honesty, grammar
lives in two languages; (B) HTML-comment JSON sidecar stripped by core —
in-band (streams as deltas), delimiter fragility, text-mangling risk.

### CLI surface

```
mix2-consult disagree <<'SPLIT'
claude: cache the compiled schema in-process | chosen
codex: move validation off the hot path | deferred
team: ship the cache now; file the validation rework as a follow-up
SPLIT
```

The helper binary stays dumb (std + serde_json only): mode `disagree` sends
stdin verbatim as `disagreement_text` over the existing transport with the
token. ALL parsing/validation is server-side in a new dependency-free module
`collaboration/disagreement.rs`. Server refusals print and exit 2 (existing
helper behavior).

### Grammar (server-side)

- One line per session agent: `<agent>: <position> | <outcome>`. Agent names
  case-insensitive; accept kind (`claude`) and display name (`Claude`). Split
  on the LAST `|` (positions may contain pipes — TS union types are core
  subject matter). Outcome vocabulary with synonyms: chosen|shipped → Chosen;
  deferred|follow-up|followup → Deferred; dropped|set-aside → Dropped.
  Non-keyword tail → refusal naming the tail. Both stances may be Chosen
  (compromise); the resolution carries the synthesis.
- Required `team: <resolution>` line; later lines not matching `<agent>:` fold
  into the resolution, hard-capped at 300 chars at a word boundary with `…`.
- Positions are meaning-critical, never silently mutated: >200 chars is
  REFUSED ("position too long — restate it in one line"); retries are free.
- Normalized-identical positions (case/whitespace-insensitive) REFUSED: "both
  positions are the same — that's not a split; disclose the nuance in prose
  instead."
- Refusals embed a filled-in valid example (`pub const DISAGREE_EXAMPLE`) and
  end with "if this fails twice, skip recording and state the disagreement in
  prose" (bounded retry).
- DISAGREE_EXAMPLE is interpolated into the lead prompt via `format!`; a cargo
  test parses the constant with the real parser (self-verifying prompt).

### Core semantics

- **Gate:** refuse unless ≥1 consultation COMPLETED this turn, tracked in the
  ConsultServer's `ActiveTurn` (`completed_consults: Arc<AtomicU32>`,
  incremented inside the spawned consult task BEFORE result delivery/done-file
  write). A delivered-not-completed gate is unimplementable: file-mode `wait`
  polls a local done-file without contacting the server (mix2_consult.rs:137,
  226). Accepted residual risk, prompt-mitigated ("record only after you have
  read your teammate's assessment"). Refusal verbatim: "no completed
  consultation this turn — disclose the disagreement in prose instead." The
  server-side counter also sidesteps the existing race where the runtime's
  `TurnState.successful_consults` only increments when the update channel
  drains (runtime.rs:499-505) and `tokio::select!` may service
  `LeadMsg::Done` first.
- **Validation:** agent names must be exactly the session's lead+teammate, one
  line each; capability token checked as for consults.
- **Revision transaction:** equality check (idempotent re-record → ok),
  revision cap (3; further distinct records refused: "revision limit reached —
  the earlier record stands; note any change in prose"), increment, and
  replacement are ONE mutation inside the `active` RwLock read guard (no await
  while held; the update-channel send happens after the guard drops).
- **Atomic settle (fixes review-2 blocker):** `ConsultServer::end_turn()`
  changes signature to return `Option<DisagreementRecord>`: under ONE `active`
  write lock it takes the ActiveTurn and extracts its recorded disagreement.
  Because disagree commits happen inside the read guard, every commit strictly
  happens-before the take; after the take, in-flight requests see None and get
  the existing "no active mix2 turn" refusal. Success-then-not-rendered is
  impossible.
- **Success response:** "Recorded — the interface renders the split beside
  your answer. In your final answer, cover the disagreement itself in at most
  one sentence (the team's call); the rest of your answer is unaffected."

### Data flow (single serialized source)

- Live: `ConsultUpdate::DisagreementRecorded { turn_id, record, revision }` →
  runtime emits `disagreement.recorded { turn_id, stances, resolution,
  revision }` — consumed ONLY for the live team-panel ledger. The
  `update_turn_id` match (runtime.rs:36) is exhaustive; the compiler enforces
  the new arm.
- Authoritative: `finish_turn` receives the record from `end_turn()` and
  attaches it to `message.final` as `disagreement: Option<DisagreementRecord>`
  (`#[serde(skip_serializing_if = "Option::is_none")]`). This is the ONLY
  serialized source for settled UI. `turn.completed` carries NO count. The TUI
  derives the settled item, TurnRecord, and `lastSummary.disagreements` from
  the message.final payload staged on its ActiveTurn.
- PROTOCOL_VERSION is NOT bumped: one optional field + one ignorable event
  type are compatible both ways (zod z.object strips unknown keys; unknown
  event types drop to null — protocol.ts:130-141).
- Out-of-order guard (review-2 #4): the TUI reducer ignores a
  `disagreement.recorded` whose revision is ≤ the currently held revision.
- Cancelled/failed turns: cleared on both `turn.cancelled` and `turn.failed`;
  TurnRecord carries a disagreement only for completed turns.
- Prompt: rule 7 of `lead_instructions` (prompts.rs:69) rewritten to route
  through the verb — record AFTER reconciliation, BEFORE the final answer;
  prose covers the split in at most one sentence; states the gate constraint;
  embeds DISAGREE_EXAMPLE.

### TUI

- protocol.ts: zod schemas for the new event + optional message.final field.
- store.ts: `ActiveTurn.disagreement?: DisagreementState` (live event sets it
  with the revision guard; the message.final payload overwrites it as
  authoritative); TurnRecord copy gated on completed; `lastSummary` gains
  `disagreements: number`.
- conversation.ts finalLines: stance block after the markdown body (2-space
  indent, width `min(ctx.width, 92)`, name `padEnd(7)` matching teamPanel,
  arrows faint: Chosen `← shipped`, Deferred `→ follow-up`, Dropped
  `→ set aside`).
- Cell-width correctness (review-2 #8): a `displayWidth()` helper backed by
  the `string-width` package (already in the dependency tree transitively via
  Ink; becomes a direct dependency — needs user approval) is used for stance
  truncation/right-alignment so CJK/emoji in model-written positions can't
  break the block. TUI-wide adoption stays a follow-up.
- teamPanel.ts: `△ disagreement` ledger after the consultation-count line and
  before the exchange section, from live turn or settled record; restructured so
  the empty-consult early return can't skip it; the live-panel ad-hoc record
  object (teamPanel.ts:52-58) gains the field.
- StatusBar.tsx: `· △ 1 disagreement` suffix on the done summary when > 0.

### Residual risks (accepted, documented)

1. Model never calls the verb → prose-only, no block, no count. Prompt-side
   fix only; no heuristic parsing by design.
2. Completion gate (not delivery): a lead that fired `start` and never
   `wait`ed can record. Ack protocol not worth the complexity.
3. Double disclosure (block + prose restatement): cosmetic; prompt-mitigated.
4. Core validates mechanics, not truth: outcomes can be mislabeled; the UI
   renders what was asserted.

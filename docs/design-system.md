# mix2 terminal design system

This document records the design system implemented by the Ink TUI. It is
derived from **Design Direction #4** of the shared Claude Design exploration
("Cladex Directions"), which composes three earlier decisions:

- **1c "Signal"** — the app chrome: inverted name chip, hairline separators,
  a persistent live status bar, tool activity as a `├`/`└` tree.
- **3b "Merge"** — consultation rendering: while the two agents work in
  parallel they occupy side-by-side tiles in their own colors; when they
  actually exchange messages the tiles fuse into one shared mauve tile with
  the dialogue visible.
- **4a/4b/4c** — the full session flow, the in-answer disagreement stance
  block, and the ctrl+t team panel.

Future contributors should not need to reverse-engineer any of this from
components; when in doubt, this file is the authority for visuals.

## Voice & tone

Two rival labs' agents on one team is the product's standing joke — let
microcopy and the agents' own voice carry it dryly ("sworn competitors,
model colleagues"), at most one wink per screen or response, and never in
serious moments: errors, security findings, cancellations. Clarity beats
the joke everywhere they compete.

## Principles

1. The two most important things on screen are always what the user asked
   and the team's answer. Everything else is quieter than both.
2. Completed work settles: active activity is colored and animated, finished
   activity collapses to a single dim line.
3. Mauve appears **only** when the agents are actually exchanging (confer,
   team attribution, disagreement). Collaboration has one unambiguous signal.
4. Never communicate state by color alone: every colored state also has a
   glyph (`●` / `○` / `◐` / `⇄` / `↔` / `△`).
5. No boxes around everything. Borders exist in exactly three places: the
   consultation tiles, the merged confer tile, and the composer frame
   (input must read unmistakably apart from output). Everything else uses
   indentation, whitespace, and hairlines.

## Color roles

Terminal-safe hex values, taken directly from the design file. The UI reads
them through semantic roles (`theme.ts`), never as raw values in components.

| Role                  | Hex       | Usage |
| --------------------- | --------- | ----- |
| `bg`                  | `#17161b` | app background (only painted by the terminal theme; the UI does not force it) |
| `bgStatus`            | `#211f27` | status bar background |
| `text.primary`        | `#d6d3dc` | user text, final answers |
| `text.secondary`      | `#a9a5b2` | dialogue lines, stances |
| `text.muted`          | `#8d8896` | status phrases, interim notes |
| `text.faint`          | `#514d59` | tool tree, timings, keyboard hints |
| `agent.claude`        | `#e0a06a` | Claude glyph `●`, chip background, tile border |
| `agent.codex`         | `#8ab8d6` | Codex glyph `○`, chip background, tile border |
| `agent.team`          | `#b795e6` | mauve: confer glyph `⇄`, Team chip, disagreement `△` |
| `chip.appBg/appFg`    | `#d6d3dc` / `#17161b` | inverted ` mix2 ` chip |
| `chip.claudeFg`       | `#1c1208` | text on Claude chip |
| `chip.codexFg`        | `#0c141c` | text on Codex chip |
| `chip.teamFg`         | `#160e20` | text on Team chip |
| `border.subtle`       | `#2a2830` | outer frame in the mock; unused as a full frame in the real TUI |
| `border.hairline`     | `#232128` | header underline |
| `border.bridge`       | `#39353f` | `╌╌` bridge dashes |
| `status.error`        | `#e06a6a` | errors (derived: Claude hue rotated to red, same saturation family) |

Tile borders use the agent color at ~50% strength; terminals cannot blend, so
the implementation uses the agent color directly on border glyphs and keeps
the border to a thin rounded box (`╭─╮ │ ╰─╯`).

## Typography and glyph vocabulary

Everything is the user's monospace font. Weight and color carry hierarchy:

- **bold** — the `❯` prompt glyph, chip labels, agent names in tile headers
- *italic* — an agent's interim "thinking out loud" line inside a tile
  (never hidden chain-of-thought; only surfaced interim text)
- glyphs:
  - `●` Claude · `○` Codex · `◐` Team (panel header, resolution stance)
  - `❯` user prompt · `▊` input cursor
  - `├` `└` tool tree connectors
  - `↔` consultation announced ("bringing in codex")
  - `⇄` agents exchanging (confer) — always mauve
  - `╌╌` bridge dashes beside `⇄` between merged tile halves
  - `△` disagreement marker
  - `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` braille spinner (one active spinner per region)
  - `·` separator dot in status text

## Layout

Full-screen alternate-buffer app, three fixed regions:

```text
header      1 row   full-width bar in bgStatus: ` mix2 ` chip ·
                    ● Claude · ○ Codex roster (glyphs in agent colors,
                    no role labels) · right-aligned cwd
spacing     1 row   becomes the sticky prompt bar when the governing
                    prompt scrolls out of view: `❯ <prompt…>  ↑ jump`
                    in bgStatus (faint ❯, muted text); updates as you
                    scroll through history, click jumps to that prompt
conversation  *     scrollable viewport, 2-space left inset
composer    1+ rows ❯ + multiline input
status bar  1 row   bgStatus; left = state, right = keyboard hints
```

The top and bottom bars share the same `bgStatus` background so they read
as app chrome on any terminal theme — the hyprland-style frame. The
original hairline row was dropped: on non-design backgrounds it was
invisible, and the bar edge is the separator.

- Content column: 2-space indent; continuation lines of the user message
  align under the first character of the message (4 spaces).
- Max reading width ~92 columns; on wider terminals content stays
  left-anchored with the same inset (design keeps a fixed content column,
  not centered).
- Minimum supported width ~80 columns; tiles fall back to stacked
  (full-width, sequential) below 88 columns.
- One blank line between conversation items; no blank line between an
  agent's status line and its own tool tree.

## Conversation items

**User message**

```text
  ❯ our /api/sessions p99 jumped to 800ms after
    yesterday's deploy — find out why and fix it?
```

`❯` bold; message text in primary at medium weight (rendered bold-off,
color primary — terminals lack a 500 weight).

**Team working (live)** — solo work belongs to the team, never a named
agent; individual identity appears only where the work visibly splits
(tiles, trace pill, team panel, the parallel status-bar state):

```text
  ◐ Team — investigating                                      ⠸ 0:48
    ├ read src/db/session.ts
    ├ search "SessionManager" — 14 matches
    └ ↔ Codex — second opinion on the data model
```

`◐ Team` in mauve; `— status` in muted. Elapsed time right aligned in
faint. Tool tree lines in faint, latest 3 visible while live.

**Team working (settled)** — the tree collapses to one dim line:

```text
  ◐ Team — investigated
    └ 12 tool calls · routes.ts, git diff, bench · 0:48
```

Once collaboration starts, the live `◐ Team — …` block moves to the TAIL
of the body — below the tiles and the conferred tile — so the newest
thing on screen is always the rotating mark saying work continues. A
settled tile must never be the last thing visible while the team is
still reconciling.

Paired consultation tiles are always the same width *and* height (the
shorter body pads with blank rows) so they read as one unit. The status
bar mirrors reality: `⠸ · ⠧ working in parallel` (spinners in agent
colors, out of phase) when both agents are active, `⠧ codex reviewing`
only when the coordinator is idle, `⠋ team working` / `⠋ team
reconciling` (mauve) for solo phases.

**Interim finding** — the lead talks while working: plain primary text
paragraph between activity blocks.

**Consultation announced**

```text
  ↔ second opinion  · 1 of 2
  ↔ one more round  · 2 of 2
```

muted; the `↔` and phrase never mauve (no exchange yet). Two rules:
never "bringing in" (the teammate is a standing member, present from the
first keystroke), and never name who is asking whom — addressing one
agent reveals the coordinator, and the team has no visible boss. The
lead-side tile likewise says `thinking / mulling it over`, not "waiting
on <teammate>". Who-asked-whom lives only in the ctrl+t team panel.

**Parallel tiles (both agents privately working)** — side by side, each in
its own agent color border:

```text
  ╭ ● claude — drafting fix ────────── ⠸ 0:22 ╮  ╭ ○ codex — reproducing ──── ⠧ 0:15 ╮
  │ cache the compiled schema at              │  │ └ run bench: 6.1ms/req compile     │
  │ boot, keyed by route…                     │  │ └ confirmed on 3 endpoints         │
  ╰───────────────────────────────────────────╯  ╰────────────────────────────────────╯
```

Interim text italic muted; action lines faint. Below 88 columns tiles stack.

**Merged confer tile (3b)** — tiles fuse; border mauve; chips + dialogue:

```text
  ⇄ conferring
  ╭─  Claude  ⇄  Codex  ─────────────────────────────── 0:31 ─╮
  │ ● does a GSI on user_id cover the analytics path?          │
  │ ○ no — that path scans; you'd need a second GSI.           │
  ╰────────────────────────────────────────────────────────────╯
```

Dialogue lines: agent glyph in agent color, text in secondary. Show the last
4 lines while live (window), all lines in the team panel. This dialogue is
each agent's *written* consultation exchange, never hidden reasoning.

**Collapsed trace pill** — after collaboration finishes, the whole episode
settles to one faint line:

```text
  └ trace  ● 1:52  ⇄ 2 msgs  ○ 0:41   ctrl+t
```

**Final answer**

```text
   Team  claude + codex

  The regression was per-request schema compilation in
  validation middleware. Fixed by compiling once at boot.
    └ ~ src/middleware/validate.ts · ✓ 214 tests
```

Speaker chip: always ` Team ` (mauve bg) — the user talks to one team with
one voice, and the answer text says "we". The faint `claude + codex`
suffix appears only when at least one consultation actually succeeded, so
the participation signal stays honest. Body text primary at full width ≤
max reading width. Individual agent identity (names, glyphs, colors)
appears only in live activity: working lines, tiles, the trace pill, and
the team panel.

**Disagreement stance block (4b)** — this block never comes from the lead's
free-text answer. When the lead and teammate genuinely split, the lead
records the split as a runtime action, `mix2-consult disagree` — a heredoc
with one line per session agent (`<agent>: <position> | <outcome>`) and a
required closing `team: <resolution>` line. The core validates the record
before accepting it: at least one consultation must have completed this
turn, the agent names must match the session's actual lead and teammate,
the two positions can't be identical, and a turn can revise its record at
most 3 times. Once accepted, the validated record rides `message.final` as
structured data, not prose — the TUI never parses disagreement structure out
of text. If the lead never calls the verb, no block renders and no count
increments; that is the expected degradation, not a bug. Positions in the
block are truncated to fit the row, ending in a visible `…`; the
untruncated positions live in the ctrl+t team panel's ledger.

```text
  △ where we split
  ● claude   cache compiled schema in-process        ← shipped
  ○ codex    move validation off the hot path        → follow-up
  ◐ team     lead's call — ship now, file the rework
```

Mauve `△` header. Each stance row: agent glyph in agent color, name padded
to a 9-column field, truncated position, then a faint outcome arrow
right-aligned to the block's edge — `← shipped` for the chosen stance,
`→ follow-up` for deferred, `→ set aside` for dropped. The closing `◐ team`
row carries the resolution in the same name-field width, no arrow.

**Markdown in answers** — agent text renders as terminal markdown, in the
same quiet vocabulary: headings bold (marker stripped, blank line before),
numbered lists with the number in muted and a hanging indent, bullets as
muted `•`, inline code in secondary on the bar background, fenced code
behind a `│` gutter in `border.bridge` (truncated, never wrapped),
blockquotes behind `▏` in muted italic, rules as a short `─` run. No
boxes. Raw markdown markers must never reach the screen.

**Error**

```text
  × Claude failed — usage limit reached. Try again later.
```

`×` + message in `status.error`; composer returns to ready.

## Status bar states

Left segment (colored by state), right segment (faint hints):

| State        | Left                                              | Right |
| ------------ | ------------------------------------------------- | ----- |
| idle         | `ready` (muted)                                   | `ctrl+t team · /help` |
| lead working | `⠸ claude working` (agent color)                  | `esc cancel · ctrl+t` |
| consulting   | `⠧ codex reviewing` (agent color)                 | `esc cancel · ctrl+t` |
| conferring   | `⇄ conferring` (mauve)                            | `esc cancel · ctrl+t` |
| synthesizing | `⠸ claude reconciling` (agent color)              | `esc cancel · ctrl+t` |
| done         | `done in 2:33 · ⇄ 2 consultations · △ 1 disagreement` (muted) | `ctrl+t team` |
| team panel   | `◐ team — this run` (mauve)                       | `esc close` |

## Team panel (ctrl+t, 4c)

An overlay replacing the conversation viewport (chrome stays):

```text
  ◐ team — this run                                   esc close

  ● claude  lead      1:52 active   12 tools
  ○ codex   teammate  0:41 active    3 tools
  └ 2 consultations · 4 messages

  △ disagreement

  ● claude
    cache the compiled schema in-process
    ← shipped

  ○ codex
    move validation off the hot path
    → follow-up

  ◐ team
    ship the cache now, file the validation rework as a follow-up

  exchange
  ● 14:02  does per-request compile explain the p99?
  ○ 14:02  yes — repro'd at 6.1ms/req on the bench
```

Contents: participants with role/elapsed/tool counts, consultation list with
duration, the `△ disagreement` ledger when a split was recorded, and each
teammate consultation's **final response** (scrollable). Hidden reasoning is
never shown here. When the teammate is unavailable the panel says so:
`○ codex   teammate  unavailable — <reason>`.

**Disagreement ledger** — sits right after the consultation count, before
the exchange. It renders both while the turn is still live (the record is
provisional, updated in place if the lead revises it) and after the turn
settles (the record then comes from the finished turn's `message.final`,
the same authoritative payload the stance block reads). Unlike the stance
block in the final answer, nothing here is truncated: each stance gets its
own glyph-and-name line, the full position text wrapped across as many
lines as it needs, and a faint outcome line below it (`← shipped` /
`→ follow-up` / `→ set aside`); a closing `◐ team` line carries the full,
also-wrapped resolution. The ledger renders even when this turn had no
consultations to list in the exchange below it.

## Model picker (/model)

An overlay in the panel pattern: `◐ models` header, the two agents as
side-by-side columns (stacked under 88 cols) in their identity colors,
"provider default" always first, `›` inverse cursor on the focused entry,
`●` in the agent color marking the active choice. `↑↓` choose, `←→`/tab
switch agent, enter applies (panel stays open so both agents can be set),
esc closes. Selections confirm via a conversation notice.

## Composer

```text
╭──────────────────────────────────────────────────╮
│ ❯ I'm thinking about replacing Postgres with▊    │
╰──────────────────────────────────────────────────╯
```

- A rounded full-width frame separates input from output at a glance (a
  deliberate exception to the no-boxes rule, alongside the consultation
  tiles): border in `text.faint` when ready, `border.subtle` while a turn
  runs.
- `❯` bold in primary when focused/ready; faint while a turn runs.
- Multiline: wrapped visually plus hard newlines.
- Recognized slash commands highlight as you type: the command token
  renders bold in the team accent the moment it matches (/help, /team,
  ...), so a valid command is visibly acknowledged before Enter; partial
  or unknown tokens stay plain.
- Mouse: drag-selection over the conversation renders inverse-video
  highlight and copies on release (status bar flashes `selection
  copied` in mauve); the wheel scrolls the viewport. Selection state
  clears on the next click.
- Keys: `Enter` submit · `ctrl+j` (and `alt+enter` where the terminal sends
  ESC+CR) insert newline · pasted newlines are preserved · `esc` cancels the
  active turn · `ctrl+c` cancels, twice quits · `ctrl+q` quits ·
  `ctrl+t` toggles team panel · `PageUp`/`PageDown` scroll the conversation.
- The block cursor `▊` renders inverted at the insertion point.

## Animation rules

- **The living team mark**: while the team is working, the `◐` mark rotates
  clockwise through `◐ ◓ ◑ ◒` (~3 fps, one full turn ≈ 1.3s) wherever a
  team state shows — the working line, the status bar's team states, and
  the live team panel header. When work completes it settles back to the
  static `◐`. This is the product's signature motion, the terminal
  equivalent of a pulsing logo: same width every frame, constant mauve,
  alive without demanding attention.
- Exactly one animation per visual region: regions led by the rotating
  team mark carry no braille spinner (their elapsed time stands still);
  agent-owned regions (tiles, `codex reviewing`, the parallel state's
  paired spinners) use braille frames at 10 fps.
- No pulsing, no decorative motion. State transitions swap glyph + color.
- Completed items must not shift layout when they settle (tool tree collapse
  replaces lines in place; every team-mark frame is one cell wide).

## Degraded terminals

- `NO_COLOR` / 16-color terminals: agent identity survives via glyphs
  (`●`/`○`/`◐`) and text labels; chips fall back to bracketed labels
  (`[mix2]`, `[Team]`).
- Light terminals: all roles are chosen to keep ≥ AA contrast against both
  `#17161b` and light backgrounds except `text.faint`, which is decorative.

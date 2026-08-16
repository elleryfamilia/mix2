/**
 * Renders domain state into styled lines per Design #4:
 * user/answer dominance, settled-activity quieting, agent-colored tiles
 * that merge into mauve when the agents actually exchange, and a collapsed
 * trace pill once collaboration finishes.
 */
import type { Disagreement, Stance } from '../ipc/protocol.js';
import type { ActiveTurn, AppState, ConsultState, ConversationItem } from '../state/store.js';
import { formatElapsed } from '../state/store.js';
import {
  MAX_CONTENT_WIDTH,
  TILE_BREAKPOINT,
  agentColor,
  agentGlyph,
  chipFg,
  displayName,
  glyphs,
  theme,
} from '../theme/theme.js';
import { BLANK, Line, Span, displayWidth, span, spread, truncate, truncateDisplay, wrapText } from './lines.js';
import { markdownLines } from './markdown.js';

const INDENT = 2;

export interface RenderContext {
  /** Full terminal columns available to the viewport. */
  width: number;
  /** Current spinner frame glyph. */
  spinner: string;
  /** Current team-mark frame (rotating ◐ while busy); static ◐ if absent. */
  teamGlyph?: string;
  /** Current timestamp for elapsed displays. */
  now: number;
}

function contentWidth(ctx: RenderContext): number {
  return Math.min(ctx.width - INDENT, MAX_CONTENT_WIDTH);
}

function pad(...spans: Span[]): Line {
  return [span(' '.repeat(INDENT)), ...spans];
}

// ---------------------------------------------------------------- settled

function userLines(text: string, ctx: RenderContext): Line[] {
  const width = contentWidth(ctx) - 2;
  const wrapped = text.split('\n').flatMap((l) => wrapText(l, width));
  return wrapped.map((line, i) =>
    i === 0
      ? pad(span(glyphs.prompt, { bold: true, color: theme.text.primary }), span(' '), span(line, { color: theme.text.primary }))
      : pad(span('  '), span(line, { color: theme.text.primary })),
  );
}

function interimLines(item: Extract<ConversationItem, { kind: 'interim' }>, ctx: RenderContext): Line[] {
  return markdownLines(item.text, contentWidth(ctx)).map((line) => [
    span(' '.repeat(INDENT)),
    ...line,
  ]);
}

function activityLines(
  item: Extract<ConversationItem, { kind: 'activity' }>,
  ctx: RenderContext,
): Line[] {
  const details = item.details.length > 0 ? ` ${glyphs.dot} ${item.details.join(', ')}` : '';
  const summary = truncate(
    `${item.toolsCount} tool call${item.toolsCount === 1 ? '' : 's'}${details} ${glyphs.dot} ${formatElapsed(item.durationMs)}`,
    contentWidth(ctx) - 4,
  );
  return [
    pad(
      span(glyphs.team, { color: theme.agent.team }),
      span(' Team', { color: theme.agent.team, bold: true }),
      span(' — investigated', { color: theme.text.muted }),
    ),
    pad(span('  '), span(`${glyphs.treeEnd} ${summary}`, { color: theme.text.faint })),
  ];
}

function traceLines(item: Extract<ConversationItem, { kind: 'trace' }>, ctx: RenderContext): Line[] {
  const left: Line = pad(
    span(`${glyphs.treeEnd} trace`, { color: theme.text.faint }),
    span('  '),
    span(agentGlyph(item.leadAgent), { color: agentColor(item.leadAgent) }),
    span(` ${formatElapsed(item.leadMs)}`, { color: theme.text.faint }),
    span('  '),
    span(glyphs.confer, { color: theme.agent.team }),
    span(
      ` ${item.consultCount} consultation${item.consultCount === 1 ? '' : 's'}`,
      { color: theme.text.faint },
    ),
    span('  '),
    span(agentGlyph(item.teammateAgent), { color: agentColor(item.teammateAgent) }),
    span(` ${formatElapsed(item.teammateMs)}`, { color: theme.text.faint }),
  );
  return [spread(left, [span('ctrl+t ', { color: theme.text.faint })], Math.min(ctx.width, MAX_CONTENT_WIDTH))];
}

function chip(label: string, bg: string, fg: string): Span {
  return span(` ${label} `, { bgColor: bg, color: fg, bold: true });
}

// ----------------------------------------------------------- stance block

// Width of the glyph+name field a stance row shares with the team row
// below it, so their positions line up: "claude"/"codex"/"team" all fit
// with room to spare.
const STANCE_NAME_WIDTH = 9;
// Columns spent before a row's position text starts: 2-space indent, the
// glyph, and the padded name field (its own leading space + the field).
const STANCE_PREFIX_WIDTH = INDENT + 1 + 1 + STANCE_NAME_WIDTH;
// Minimum columns always kept between a truncated position and its arrow.
const STANCE_MIN_GAP = 2;

const STANCE_ARROWS: Record<Stance['outcome'], string> = {
  chosen: '← shipped',
  deferred: '→ follow-up',
  dropped: '→ set aside',
};

function stanceNameField(name: string): string {
  return ' ' + name.padEnd(STANCE_NAME_WIDTH);
}

/** Right-aligns `right` against `left` like `spread`, but measures both by
 * terminal display width instead of `.length`. `spread` (lines.ts) would
 * undercount a CJK/fullwidth position's true width and overpad the gap,
 * pushing the row past the frame — this is the one place that can't use
 * it. Callers must pre-truncate `left`'s content so `gap` never has to
 * degrade below `STANCE_MIN_GAP`. */
function stanceSpread(left: Line, right: Line, width: number): Line {
  const leftWidth = left.reduce((n, s) => n + displayWidth(s.text), 0);
  const rightWidth = right.reduce((n, s) => n + displayWidth(s.text), 0);
  const gap = Math.max(1, width - leftWidth - rightWidth);
  return [...left, span(' '.repeat(gap)), ...right];
}

/** The `△ where we split` block: one row per stance (position truncated,
 * outcome arrow right-aligned) plus the team's resolution. Only rendered
 * when the model actually recorded a split — see the honesty rule in
 * docs/superpowers/plans/2026-08-15-disagreement-layer-spec.md. */
function stanceLines(d: Disagreement, ctx: RenderContext): Line[] {
  const width = Math.min(ctx.width, MAX_CONTENT_WIDTH);
  const lines: Line[] = [
    pad(span(`${glyphs.disagree} where we split`, { color: theme.agent.team, bold: true })),
  ];
  for (const stance of d.stances) {
    const arrow = STANCE_ARROWS[stance.outcome];
    const room = Math.max(0, width - STANCE_PREFIX_WIDTH - displayWidth(arrow) - STANCE_MIN_GAP);
    const left = pad(
      span(agentGlyph(stance.agent), { color: agentColor(stance.agent) }),
      span(stanceNameField(stance.agent), { color: agentColor(stance.agent), bold: true }),
      span(truncateDisplay(stance.position, room), { color: theme.text.secondary }),
    );
    lines.push(stanceSpread(left, [span(arrow, { color: theme.text.faint })], width));
  }
  const teamRoom = Math.max(0, width - STANCE_PREFIX_WIDTH);
  lines.push(
    pad(
      span(agentGlyph('team'), { color: agentColor('team') }),
      span(stanceNameField('team'), { color: agentColor('team'), bold: true }),
      span(truncateDisplay(d.resolution, teamRoom), { color: theme.text.secondary }),
    ),
  );
  return lines;
}

function finalLines(item: Extract<ConversationItem, { kind: 'final' }>, ctx: RenderContext): Line[] {
  const lines: Line[] = [];
  // One team, one voice: every answer carries the Team chip. The roster
  // suffix appears only when both agents actually worked this turn — that
  // participation signal stays honest.
  if (item.speaker === 'team') {
    lines.push(
      pad(
        chip('Team', theme.agent.team, chipFg('team')),
        span(`  ${item.lead} + ${item.lead === 'claude' ? 'codex' : 'claude'}`, {
          color: theme.text.faint,
        }),
      ),
    );
  } else {
    lines.push(pad(chip('Team', theme.agent.team, chipFg('team'))));
  }
  lines.push(BLANK);
  for (const line of markdownLines(item.text, contentWidth(ctx))) {
    lines.push([span(' '.repeat(INDENT)), ...line]);
  }
  if (item.disagreement) {
    lines.push(BLANK);
    lines.push(...stanceLines(item.disagreement, ctx));
  }
  return lines;
}

function errorLines(item: Extract<ConversationItem, { kind: 'error' }>, ctx: RenderContext): Line[] {
  const width = contentWidth(ctx) - 2;
  return wrapText(`${glyphs.fail} ${item.text}`, width).map((line) =>
    pad(span(line, { color: theme.status.error })),
  );
}

// ------------------------------------------------------------------ live

function statusWord(turn: ActiveTurn): string {
  switch (turn.phase) {
    case 'working':
      return turn.tools.length > 0 ? 'investigating' : 'thinking';
    case 'consulting':
      return 'consulting';
    case 'synthesizing':
      return 'reconciling both perspectives';
  }
}

function leadWorkingLines(turn: ActiveTurn, ctx: RenderContext): Line[] {
  // The user talks to one team: solo work is the *team* thinking, not a
  // named agent. Individual identity appears only where the work visibly
  // splits (tiles, trace pill, team panel).
  const color = theme.agent.team;
  const width = Math.min(ctx.width, MAX_CONTENT_WIDTH);
  const elapsed = formatElapsed(ctx.now - turn.startedAt);
  // The rotating team mark is this region's one animation; the elapsed
  // time on the right stays still.
  const head = spread(
    pad(
      span(ctx.teamGlyph ?? glyphs.team, { color }),
      span(' Team', { color, bold: true }),
      span(` — ${statusWord(turn)}`, { color: theme.text.muted }),
    ),
    [span(elapsed, { color: theme.text.faint })],
    width,
  );
  const lines: Line[] = [head];

  // Live tool tree: most recent three entries.
  const recent = turn.tools.slice(-3);
  const hidden = turn.tools.length - recent.length;
  recent.forEach((tool, i) => {
    const last = i === recent.length - 1;
    const connector = last ? glyphs.treeEnd : glyphs.treeMid;
    const label = tool.detail ? `${tool.name.toLowerCase()} ${tool.detail}` : tool.name.toLowerCase();
    const extra = last && hidden > 0 ? `  +${hidden} more` : '';
    lines.push(
      pad(
        span('  '),
        span(
          `${connector} ${truncate(label, contentWidth(ctx) - 6)}${extra}`,
          { color: theme.text.faint },
        ),
      ),
    );
  });

  // The lead talking while it works (interim, not yet settled).
  const stream = turn.streamText.trim();
  if (stream) {
    lines.push(BLANK);
    for (const line of wrapText(stream, contentWidth(ctx)).slice(-4)) {
      lines.push(pad(span(line, { color: theme.text.primary })));
    }
  }
  return lines;
}

interface TileSpec {
  headerLeft: Line;
  headerRight: Line;
  body: Line[];
  borderColor: string;
}

function buildTile(spec: TileSpec, width: number): Line[] {
  const inner = width - 2;
  const border = { color: spec.borderColor };
  const top: Line = [
    span('╭ ', border),
    ...spec.headerLeft,
    span(' '),
    span(
      '─'.repeat(
        Math.max(
          1,
          inner - 2 - spec.headerLeft.reduce((n, s) => n + s.text.length, 0) - spec.headerRight.reduce((n, s) => n + s.text.length, 0) - 2,
        ),
      ),
      border,
    ),
    span(' '),
    ...spec.headerRight,
    span(' ╮', border),
  ];
  const rows = spec.body.map((line) => {
    const text = line.reduce((n, s) => n + s.text.length, 0);
    const fill = Math.max(0, inner - 2 - text);
    return [span('│ ', border), ...line, span(' '.repeat(fill)), span(' │', border)];
  });
  const bottom: Line = [span('╰' + '─'.repeat(Math.max(0, width - 2)) + '╯', border)];
  return [top, ...rows, bottom];
}

function zipTiles(left: Line[], right: Line[], gap: number): Line[] {
  const height = Math.max(left.length, right.length);
  const leftWidth = Math.max(...left.map((l) => l.reduce((n, s) => n + s.text.length, 0)));
  const out: Line[] = [];
  for (let i = 0; i < height; i++) {
    const l = left[i] ?? [span(' '.repeat(leftWidth))];
    const lw = l.reduce((n, s) => n + s.text.length, 0);
    const r = right[i] ?? [];
    out.push([span('  '), ...l, span(' '.repeat(Math.max(0, leftWidth - lw) + gap)), ...r]);
  }
  return out;
}

function consultLines(turn: ActiveTurn, consult: ConsultState, ctx: RenderContext): Line[] {
  const lines: Line[] = [];
  const teammateName = consult.agent;

  // Direction-neutral on purpose: naming who asks whom would reveal the
  // coordinator, and the user talks to one team with no visible boss. A
  // second round still reads as the challenge it is.
  const ask = consult.index > 1 ? 'one more round' : 'second opinion';
  lines.push(
    pad(
      span(`${glyphs.consult} ${ask}`, { color: theme.text.muted }),
      span(
        consult.max > 1 ? `  ${glyphs.dot} ${consult.index} of ${consult.max}` : '',
        { color: theme.text.faint },
      ),
    ),
  );

  if (consult.status === 'running') {
    lines.push(BLANK);
    lines.push(...parallelTiles(turn, consult, ctx));
  } else if (consult.status === 'done') {
    lines.push(BLANK);
    lines.push(...mergedTile(turn, consult, ctx));
  } else if (consult.status === 'failed') {
    lines.push(
      pad(
        span('  '),
        span(
          `${glyphs.treeEnd} ${truncate(consult.message ?? 'consultation failed', contentWidth(ctx) - 6)}`,
          { color: theme.text.muted },
        ),
      ),
    );
  }
  return lines;
}

function tileBody(consult: ConsultState, width: number): Line[] {
  const body: Line[] = [];
  const stream = consult.streamText.trim();
  if (stream) {
    for (const line of wrapText(stream, width).slice(-2)) {
      body.push([span(line, { color: theme.text.muted, italic: true })]);
    }
  }
  for (const tool of consult.tools.slice(-2)) {
    const label = tool.detail ? `${tool.name.toLowerCase()} ${tool.detail}` : tool.name.toLowerCase();
    // The `└ ` prefix costs 2 columns of the row budget.
    body.push([span(`${glyphs.treeEnd} ${truncate(label, width - 2)}`, { color: theme.text.faint })]);
  }
  if (body.length === 0) {
    body.push([span(`${glyphs.treeEnd} starting up`, { color: theme.text.faint })]);
  }
  return body;
}

/** Pad the shorter body with blank rows so paired tiles are the same size. */
function equalizeBodies(a: Line[], b: Line[]): void {
  while (a.length < b.length) a.push([span('')]);
  while (b.length < a.length) b.push([span('')]);
}

function parallelTiles(turn: ActiveTurn, consult: ConsultState, ctx: RenderContext): Line[] {
  const lead = turn.leadAgent;
  const teammate = consult.agent;
  const full = Math.min(ctx.width, MAX_CONTENT_WIDTH);
  const stacked = ctx.width < TILE_BREAKPOINT;
  const tileWidth = stacked ? full - INDENT : Math.floor((full - INDENT - 2) / 2);
  const innerWidth = tileWidth - 4;

  // With concurrent consultations the lead keeps researching while the
  // teammate works: show its live tools, else its interim text, else the
  // waiting state.
  const leadStream = turn.streamText.trim();
  const leadTools = turn.tools.filter((t) => !t.done).slice(-2);
  let leadBody: Line[];
  let leadStatus: string;
  if (leadTools.length > 0) {
    leadStatus = 'researching';
    leadBody = leadTools.map((tool) => {
      const label = tool.detail ? `${tool.name.toLowerCase()} ${tool.detail}` : tool.name.toLowerCase();
      return [
        span(`${glyphs.treeEnd} ${truncate(label, innerWidth - 2)}`, { color: theme.text.faint }),
      ];
    });
  } else if (leadStream) {
    leadStatus = 'drafting';
    leadBody = wrapText(leadStream, innerWidth)
      .slice(-2)
      .map((l) => [span(l, { color: theme.text.muted, italic: true })]);
  } else {
    leadStatus = 'thinking';
    leadBody = [[span(`${glyphs.treeEnd} mulling it over`, { color: theme.text.faint })]];
  }

  const teammateBody = tileBody(consult, innerWidth);
  if (!stacked) {
    // Side-by-side tiles read as one unit; equal heights keep them so.
    equalizeBodies(leadBody, teammateBody);
  }

  const leadTile = buildTile(
    {
      headerLeft: [
        span(agentGlyph(lead), { color: agentColor(lead) }),
        span(` ${lead} — ${leadStatus}`, { color: agentColor(lead), bold: true }),
      ],
      headerRight: [
        span(`${ctx.spinner} ${formatElapsed(ctx.now - turn.startedAt)}`, {
          color: theme.text.faint,
        }),
      ],
      body: leadBody,
      borderColor: agentColor(lead),
    },
    tileWidth,
  );
  const teammateTile = buildTile(
    {
      headerLeft: [
        span(agentGlyph(teammate), { color: agentColor(teammate) }),
        span(` ${teammate} — reviewing`, { color: agentColor(teammate), bold: true }),
      ],
      headerRight: [
        span(`${ctx.spinner} ${formatElapsed(ctx.now - consult.startedAt)}`, { color: theme.text.faint }),
      ],
      body: teammateBody,
      borderColor: agentColor(teammate),
    },
    tileWidth,
  );

  if (stacked) {
    return [...leadTile.map((l) => [span('  '), ...l]), ...teammateTile.map((l) => [span('  '), ...l])];
  }
  return zipTiles(leadTile, teammateTile, 2);
}

/** After the exchange actually happened, the tiles fuse into one mauve tile
 * with the dialogue visible (Design 3b), summarized to one line per side. */
function mergedTile(turn: ActiveTurn, consult: ConsultState, ctx: RenderContext): Line[] {
  const lead = turn.leadAgent;
  const teammate = consult.agent;
  const full = Math.min(ctx.width, MAX_CONTENT_WIDTH);
  const tileWidth = full - INDENT;
  const innerWidth = tileWidth - 6;

  const ask = firstMeaningfulLine(consult.prompt) ?? 'consultation request';
  const answer = firstMeaningfulLine(consult.text) ?? 'assessment returned';

  const body: Line[] = [
    [
      span(agentGlyph(lead), { color: agentColor(lead) }),
      span(` ${truncate(ask, innerWidth)}`, { color: theme.text.secondary }),
    ],
    [
      span(agentGlyph(teammate), { color: agentColor(teammate) }),
      span(` ${truncate(answer, innerWidth)}`, { color: theme.text.secondary }),
    ],
  ];

  const tile = buildTile(
    {
      headerLeft: [
        chip(displayName(lead), agentColor(lead), chipFg(lead)),
        span(` ${glyphs.confer} `, { color: theme.agent.team, bold: true }),
        chip(displayName(teammate), agentColor(teammate), chipFg(teammate)),
      ],
      headerRight: [
        span(formatElapsed(consult.durationMs ?? 0), { color: theme.text.faint }),
      ],
      body,
      borderColor: theme.agent.team,
    },
    tileWidth,
  );
  return [
    pad(span(`${glyphs.confer} conferred`, { color: theme.agent.team })),
    ...tile.map((l) => [span('  '), ...l]),
  ];
}

function firstMeaningfulLine(text?: string): string | undefined {
  if (!text) return undefined;
  for (const line of text.split('\n')) {
    const trimmed = line.trim();
    if (trimmed) return trimmed;
  }
  return undefined;
}

// -------------------------------------------------------------- assembly

export function renderItem(item: ConversationItem, ctx: RenderContext): Line[] {
  switch (item.kind) {
    case 'user':
      return userLines(item.text, ctx);
    case 'interim':
      return interimLines(item, ctx);
    case 'activity':
      return activityLines(item, ctx);
    case 'trace':
      return traceLines(item, ctx);
    case 'final':
      return finalLines(item, ctx);
    case 'error':
      return errorLines(item, ctx);
    case 'cancelled':
      return [pad(span('× cancelled', { color: theme.text.muted }))];
    case 'notice':
      return wrapText(item.text, contentWidth(ctx)).map((line) =>
        pad(span(line, { color: theme.text.muted })),
      );
  }
}

export function renderLiveTurn(turn: ActiveTurn, ctx: RenderContext): Line[] {
  const lines: Line[] = [];
  // Before any consultation, the live block leads (the design's core
  // motif: status line with the tool tree hanging under it). Once
  // collaboration starts, the live block moves to the TAIL — below the
  // tiles — so the newest thing on screen is always the rotating team
  // mark saying work continues. Otherwise a settled conferred tile at the
  // bottom reads as "finished" while the team is still reconciling.
  if (turn.consults.length === 0) {
    lines.push(...leadWorkingLines(turn, ctx));
    return lines;
  }
  for (const [i, consult] of turn.consults.entries()) {
    if (i > 0) lines.push(BLANK);
    lines.push(...consultLines(turn, consult, ctx));
  }
  lines.push(BLANK);
  lines.push(...leadWorkingLines(turn, ctx));
  return lines;
}

/** The full conversation as lines: settled items, then the live turn. */
/** Where a user prompt sits in the rendered line buffer, for the sticky
 * prompt bar and click-to-jump. */
export interface PromptAnchor {
  line: number;
  /** First line of the prompt, for the bar. */
  text: string;
}

export function renderConversationWithAnchors(
  state: AppState,
  ctx: RenderContext,
): { lines: Line[]; anchors: PromptAnchor[] } {
  const lines: Line[] = [];
  const anchors: PromptAnchor[] = [];
  if (state.items.length === 0 && !state.turn) {
    return { lines: welcomeLines(state, ctx), anchors };
  }
  for (const item of state.items) {
    if (lines.length > 0) lines.push(BLANK);
    if (item.kind === 'user') {
      anchors.push({
        line: lines.length,
        text: item.text.split('\n')[0]?.trim() ?? '',
      });
    }
    lines.push(...renderItem(item, ctx));
  }
  if (state.turn) {
    if (lines.length > 0) lines.push(BLANK);
    lines.push(...renderLiveTurn(state.turn, ctx));
  }
  return { lines, anchors };
}

export function renderConversation(state: AppState, ctx: RenderContext): Line[] {
  return renderConversationWithAnchors(state, ctx).lines;
}

/** The startup framing: one team, what it's best at, where plans land.
 * Adapts when the directory isn't a software project (general
 * brainstorming — product ideas, viability, strategy). */
function welcomeLines(state: AppState, ctx: RenderContext): Line[] {
  const width = contentWidth(ctx);
  const project = state.session?.project ?? true;
  const lines: Line[] = [];
  lines.push(pad(span('How can we help?', { color: theme.text.primary })));
  lines.push(BLANK);
  const body = project
    ? 'You are talking to Claude and Codex as one team — sworn competitors, ' +
      'model colleagues. Substantive questions engage both, working in ' +
      'parallel with independent takes. Best for brainstorming, design, code ' +
      'review, debugging, and tradeoffs. Asked to implement, the team writes ' +
      'the plan to .mix2/ instead of touching your code — ready to hand to ' +
      'claude or codex to execute.'
    : 'No project detected here, so bring anything: a product idea, business ' +
      'viability, strategy, a document. You are talking to Claude and Codex ' +
      'as one team — sworn competitors, model colleagues — and substantive ' +
      'questions engage both in parallel with independent takes. Notes and ' +
      'plans worth keeping land in .mix2/.';
  for (const line of wrapText(body, width)) {
    lines.push(pad(span(line, { color: theme.text.muted })));
  }
  lines.push(BLANK);
  lines.push(
    pad(
      span(`vague ask ${glyphs.dot} we scope it first · specific ask ${glyphs.dot} straight to work`, {
        color: theme.text.muted,
      }),
    ),
  );
  lines.push(BLANK);
  lines.push(pad(span('/help commands · ctrl+t activity', { color: theme.text.faint })));
  return lines;
}

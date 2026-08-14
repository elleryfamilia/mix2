/**
 * The optional ctrl+t team panel (Design 4c): participants + timing, the
 * consultation exchange, and each teammate consultation's final response.
 * Hidden model reasoning is never available here — only written output.
 */
import type { AppState, ConsultState } from '../state/store.js';
import { formatElapsed } from '../state/store.js';
import {
  MAX_CONTENT_WIDTH,
  agentColor,
  agentGlyph,
  displayName,
  glyphs,
  theme,
} from '../theme/theme.js';
import { BLANK, Line, span, spread, truncate, wrapText } from './lines.js';

const INDENT = 2;

function pad(...spans: Line): Line {
  return [span(' '.repeat(INDENT)), ...spans];
}

export function renderTeamPanel(state: AppState, width: number, now: number): Line[] {
  const w = Math.min(width, MAX_CONTENT_WIDTH);
  const contentW = w - INDENT;
  const lines: Line[] = [];

  lines.push(
    spread(
      pad(
        span(`${glyphs.team} team`, { color: theme.agent.team, bold: true }),
        span(state.turn ? ' — live' : ' — this run', { color: theme.text.faint }),
      ),
      [span('esc close ', { color: theme.text.faint })],
      w,
    ),
  );
  lines.push(BLANK);

  const session = state.session;
  if (!session) return lines;

  const record = state.turn
    ? {
        consults: state.turn.consults,
        durationMs: now - state.turn.startedAt,
        toolsCompleted: state.turn.toolsCompleted,
      }
    : state.lastTurn;

  const lead = session.lead;
  const teammate = session.teammate;

  const leadName = lead.kind;
  lines.push(
    pad(
      span(agentGlyph(leadName), { color: agentColor(leadName) }),
      span(` ${leadName.padEnd(7)}`, { color: agentColor(leadName), bold: true }),
      span(
        `  ${record ? formatElapsed(record.durationMs) : '—'}${state.turn ? ' active' : ''}   ${
          record ? `${record.toolsCompleted} tool${record.toolsCompleted === 1 ? '' : 's'}` : ''
        }`,
        { color: theme.text.muted },
      ),
    ),
  );
  const teammateName = teammate.kind;
  if (teammate.available) {
    const consults = record?.consults ?? [];
    const teammateMs = consults.reduce((sum, c) => sum + (c.durationMs ?? 0), 0);
    const teammateTools = consults.reduce((sum, c) => sum + c.tools.filter((t) => t.done).length, 0);
    lines.push(
      pad(
        span(agentGlyph(teammateName), { color: agentColor(teammateName) }),
        span(` ${teammateName.padEnd(7)}`, { color: agentColor(teammateName), bold: true }),
        span(
          `  ${consults.length > 0 ? formatElapsed(teammateMs) : '—'}   ${
            consults.length > 0 ? `${teammateTools} tool${teammateTools === 1 ? '' : 's'}` : 'on standby'
          }`,
          { color: theme.text.muted },
        ),
      ),
    );
  } else {
    lines.push(
      pad(
        span(agentGlyph(teammateName), { color: agentColor(teammateName) }),
        span(` ${teammateName.padEnd(7)}`, { color: agentColor(teammateName), bold: true }),
        span(
          `  offline — ${truncate(teammate.reason ?? 'not found', contentW - 24)}`,
          { color: theme.text.muted },
        ),
      ),
    );
  }

  const consults = record?.consults ?? [];
  const done = consults.filter((c) => c.status === 'done');
  lines.push(
    pad(
      span(
        `${glyphs.treeEnd} ${consults.length} consultation${consults.length === 1 ? '' : 's'}` +
          (done.length !== consults.length ? ` ${glyphs.dot} ${done.length} completed` : ''),
        { color: theme.text.faint },
      ),
    ),
  );

  if (consults.length === 0) {
    lines.push(BLANK);
    lines.push(pad(span('no consultations this run', { color: theme.text.faint })));
    return lines;
  }

  lines.push(BLANK);
  lines.push(pad(span('exchange', { color: theme.text.muted })));
  for (const consult of consults) {
    lines.push(...exchangeLines(consult, session.lead.kind, contentW));
  }

  for (const consult of done) {
    if (!consult.text) continue;
    lines.push(BLANK);
    lines.push(
      pad(
        span(`${consult.agent} — consultation ${consult.index} response`, {
          color: agentColor(consult.agent),
          bold: true,
        }),
        span(consult.durationMs ? `  ${formatElapsed(consult.durationMs)}` : '', {
          color: theme.text.faint,
        }),
      ),
    );
    for (const raw of consult.text.split('\n')) {
      if (raw.trim() === '') {
        lines.push(BLANK);
        continue;
      }
      for (const line of wrapText(raw, contentW - 2)) {
        lines.push(pad(span('  '), span(line, { color: theme.text.secondary })));
      }
    }
  }
  return lines;
}

function exchangeLines(consult: ConsultState, lead: string, contentW: number): Line[] {
  const out: Line[] = [];
  const leadName = lead as 'claude' | 'codex';
  if (consult.prompt) {
    out.push(
      pad(
        span(agentGlyph(leadName), { color: agentColor(leadName) }),
        span(`  ${truncate(firstLine(consult.prompt), contentW - 4)}`, {
          color: theme.text.secondary,
        }),
      ),
    );
  }
  if (consult.status === 'done' && consult.text) {
    out.push(
      pad(
        span(agentGlyph(consult.agent), { color: agentColor(consult.agent) }),
        span(`  ${truncate(firstLine(consult.text), contentW - 4)}`, {
          color: theme.text.secondary,
        }),
      ),
    );
  } else if (consult.status === 'failed') {
    out.push(
      pad(
        span(agentGlyph(consult.agent), { color: agentColor(consult.agent) }),
        span(`  failed — ${truncate(consult.message ?? 'unknown', contentW - 14)}`, {
          color: theme.text.muted,
        }),
      ),
    );
  } else if (consult.status === 'running') {
    out.push(
      pad(
        span(agentGlyph(consult.agent), { color: agentColor(consult.agent) }),
        span('  reviewing…', { color: theme.text.muted }),
      ),
    );
  }
  return out;
}

function firstLine(text: string): string {
  for (const line of text.split('\n')) {
    const trimmed = line.trim();
    if (trimmed) return trimmed;
  }
  return text;
}

export function teamPanelTitle(state: AppState): string {
  return state.turn ? `${displayName('team')} — live` : `${displayName('team')} — this run`;
}

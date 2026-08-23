import { Text } from 'ink';
import React from 'react';
import type { AppState } from '../state/store.js';
import { formatDuration, speakerLabel } from '../state/store.js';
import { agentColor, glyphs, spinnerFrames, theme } from '../theme/theme.js';

interface Piece {
  text: string;
  color: string;
}

type Segment = Piece[];

function segmentLength(segment: Segment): number {
  return segment.reduce((n, piece) => n + piece.text.length, 0);
}

/** Derive the status-bar state per the design's state table. The user talks
 * to one team, so solo work reads as the team; individual agent names show
 * only when the work has visibly split (parallel state, consult). Team
 * states animate the rotating team mark; agent states use braille. */
export function statusSegments(
  state: AppState,
  spinner: string,
  teamGlyph: string = glyphs.team,
): { left: Segment; right: Segment } {
  const faint = theme.text.faint;
  const muted = theme.text.muted;
  // A second spinner frame, deliberately out of phase (design 3a shows the
  // two agents' spinners desynchronized).
  const index = spinnerFrames.indexOf(spinner as (typeof spinnerFrames)[number]);
  const spinner2 = spinnerFrames[(Math.max(index, 0) + 4) % spinnerFrames.length] ?? spinner;

  if (state.phase === 'fatal') {
    return {
      left: [{ text: 'runtime error', color: theme.status.error }],
      right: [{ text: 'q quit', color: faint }],
    };
  }
  if (state.phase === 'selecting-team') {
    return {
      left: [{ text: `${glyphs.team} pick your team`, color: theme.agent.team }],
      right: [{ text: 'enter start · esc defaults', color: faint }],
    };
  }
  if (state.teamPanelOpen) {
    return {
      left: [{ text: `${glyphs.team} team`, color: theme.agent.team }],
      right: [{ text: 'esc close', color: faint }],
    };
  }
  const turn = state.turn;
  if (turn) {
    const right: Segment = [{ text: 'esc cancel · ctrl+t', color: faint }];
    const running = turn.consults.find((c) => c.status === 'running');
    const leadBusy =
      turn.tools.some((t) => !t.done) || turn.streamText.trim().length > 0;
    if (running && leadBusy) {
      return {
        left: [
          { text: spinner, color: agentColor(turn.leadSlot) },
          { text: ` ${glyphs.dot} `, color: muted },
          { text: spinner2, color: agentColor(running.slot) },
          { text: ' working in parallel', color: muted },
        ],
        right,
      };
    }
    if (running) {
      return {
        left: [
          {
            text: `${spinner} ${speakerLabel(state.session, running.slot)} reviewing`,
            color: agentColor(running.slot),
          },
        ],
        right,
      };
    }
    if (turn.phase === 'synthesizing') {
      return {
        left: [{ text: `${teamGlyph} team reconciling`, color: theme.agent.team }],
        right,
      };
    }
    return {
      left: [{ text: `${teamGlyph} team working`, color: theme.agent.team }],
      right,
    };
  }
  if (state.lastSummary) {
    const { durationMs, consultations, disagreements } = state.lastSummary;
    const consultNote =
      consultations > 0
        ? ` ${glyphs.dot} ${glyphs.confer} ${consultations} consultation${consultations === 1 ? '' : 's'}`
        : '';
    const splitNote =
      disagreements > 0
        ? ` ${glyphs.dot} ${glyphs.disagree} ${disagreements} disagreement${disagreements === 1 ? '' : 's'}`
        : '';
    return {
      left: [{ text: `done in ${formatDuration(durationMs)}${consultNote}${splitNote}`, color: muted }],
      right: [{ text: 'ctrl+t activity', color: faint }],
    };
  }
  return {
    left: [{ text: state.phase === 'ready' ? 'ready' : 'starting…', color: muted }],
    right: [{ text: 'ctrl+t activity · ctrl+q quit', color: faint }],
  };
}

export function StatusBar({
  state,
  spinner,
  teamGlyph,
  width,
  scrolledUp = false,
  slashOpen = false,
  flash = null,
  modelPanelOpen = false,
}: {
  state: AppState;
  spinner: string;
  /** Rotating team-mark frame while busy; static ◐ otherwise. */
  teamGlyph?: string;
  width: number;
  /** The viewport is not at the bottom: newer content exists below. */
  scrolledUp?: boolean;
  /** The composer starts with '/': surface the available commands. */
  slashOpen?: boolean;
  /** Transient confirmation ("selection copied"), shown briefly. */
  flash?: string | null;
  /** The /model picker is open. */
  modelPanelOpen?: boolean;
}): React.JSX.Element {
  let { left, right } = statusSegments(state, spinner, teamGlyph);
  if (modelPanelOpen) {
    left = [{ text: '◐ models', color: theme.agent.team }];
    right = [{ text: 'type to filter · ↑↓ ←→ enter · esc cancel', color: theme.text.faint }];
  }
  if (slashOpen) {
    left = [
      {
        text: '/model · /team · /clear · /exit',
        color: theme.text.muted,
      },
    ];
  }
  if (flash) {
    left = [{ text: flash, color: theme.agent.team }];
  }
  if (scrolledUp) {
    right = [{ text: '↓ pgdn latest · ', color: theme.text.faint }, ...right];
  }
  const bg = theme.status.barBg;
  const gap = Math.max(1, width - 2 - segmentLength(left) - segmentLength(right));
  return (
    <Text backgroundColor={bg} wrap="truncate">
      <Text backgroundColor={bg}> </Text>
      {left.map((piece, i) => (
        <Text key={`l${i}`} backgroundColor={bg} color={piece.color}>
          {piece.text}
        </Text>
      ))}
      <Text backgroundColor={bg}>{' '.repeat(gap)}</Text>
      {right.map((piece, i) => (
        <Text key={`r${i}`} backgroundColor={bg} color={piece.color}>
          {piece.text}
        </Text>
      ))}
      <Text backgroundColor={bg}> </Text>
    </Text>
  );
}

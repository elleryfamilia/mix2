import { Text } from 'ink';
import React from 'react';
import type { AppState } from '../state/store.js';
import { formatDuration } from '../state/store.js';
import { agentColor, glyphs, theme } from '../theme/theme.js';

interface Segment {
  text: string;
  color: string;
}

/** Derive the status-bar state per the design's state table. */
export function statusSegments(state: AppState, spinner: string): { left: Segment; right: Segment } {
  const faint = theme.text.faint;
  if (state.phase === 'fatal') {
    return {
      left: { text: 'runtime error', color: theme.status.error },
      right: { text: 'q quit', color: faint },
    };
  }
  if (state.teamPanelOpen) {
    return {
      left: { text: `${glyphs.team} team`, color: theme.agent.team },
      right: { text: 'esc close', color: faint },
    };
  }
  const turn = state.turn;
  if (turn) {
    const right = { text: 'esc cancel · ctrl+t', color: faint };
    const running = turn.consults.find((c) => c.status === 'running');
    if (running) {
      return {
        left: { text: `${spinner} ${running.agent} reviewing`, color: agentColor(running.agent) },
        right,
      };
    }
    if (turn.phase === 'synthesizing') {
      return {
        left: { text: `${spinner} ${turn.leadAgent} reconciling`, color: agentColor(turn.leadAgent) },
        right,
      };
    }
    return {
      left: { text: `${spinner} ${turn.leadAgent} working`, color: agentColor(turn.leadAgent) },
      right,
    };
  }
  if (state.lastSummary) {
    const { durationMs, consultations } = state.lastSummary;
    const consultNote =
      consultations > 0
        ? ` ${glyphs.dot} ${glyphs.confer} ${consultations} consultation${consultations === 1 ? '' : 's'}`
        : '';
    return {
      left: { text: `done in ${formatDuration(durationMs)}${consultNote}`, color: theme.text.muted },
      right: { text: 'ctrl+t team', color: faint },
    };
  }
  return {
    left: { text: state.phase === 'ready' ? 'ready' : 'starting…', color: theme.text.muted },
    right: { text: 'ctrl+t team · ctrl+q quit', color: faint },
  };
}

export function StatusBar({
  state,
  spinner,
  width,
}: {
  state: AppState;
  spinner: string;
  width: number;
}): React.JSX.Element {
  const { left, right } = statusSegments(state, spinner);
  const gap = Math.max(1, width - 2 - left.text.length - right.text.length);
  return (
    <Text backgroundColor={theme.status.barBg} wrap="truncate">
      <Text backgroundColor={theme.status.barBg} color={left.color}>{` ${left.text}`}</Text>
      <Text backgroundColor={theme.status.barBg}>{' '.repeat(gap)}</Text>
      <Text backgroundColor={theme.status.barBg} color={right.color}>{`${right.text} `}</Text>
    </Text>
  );
}

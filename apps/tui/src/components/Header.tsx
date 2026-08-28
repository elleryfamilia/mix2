import { Text } from 'ink';
import React from 'react';
import type { SessionInfo } from '../state/store.js';
import { leadInfo, otherSlot, teammateInfo } from '../state/store.js';
import { agentColor, agentGlyph, glyphs, theme } from '../theme/theme.js';

function shortenPath(cwd: string, maxLength: number): string {
  const home = process.env['HOME'];
  let short = home && cwd.startsWith(home) ? '~' + cwd.slice(home.length) : cwd;
  if (short.length > maxLength) {
    short = '…' + short.slice(short.length - maxLength + 1);
  }
  return short;
}

/**
 * App chrome, top bar (Design 1c, hyprland-flavored): a full-width bar in
 * the same background as the status bar, carrying the inverted ` mix2 `
 * chip, the team roster as colored glyphs (● Claude · ○ Codex — no role
 * labels; who coordinates is internal), the consultation budget
 * (`↔ 2 turns`, changed with /turns), and the right-aligned project
 * path. Top and bottom bars frame the conversation on any terminal
 * background.
 */
export function Header({
  session,
  width,
}: {
  session?: SessionInfo;
  width: number;
}): React.JSX.Element {
  const bg = theme.status.barBg;
  const chipLabel = ' mix2 ';

  // The team roster: no lead/teammate labels — the user talks to one team,
  // and who coordinates is an internal mechanic.
  const lead = session?.leadSlot;
  const teammate = session ? otherSlot(session.leadSlot) : undefined;
  const leadName = session ? leadInfo(session).name : '';
  const teammateLabel = session
    ? teammateInfo(session).available
      ? teammateInfo(session).name
      : `${teammateInfo(session).name} offline`
    : '';
  const turnsLabel = session
    ? `${glyphs.consult} ${session.maxTurns} turn${session.maxTurns === 1 ? '' : 's'}`
    : '';
  const identityPlain = session
    ? `  ${agentGlyph(lead!)} ${leadName} ${'·'} ${agentGlyph(teammate!)} ${teammateLabel} · ${turnsLabel}`
    : '';

  const leftLen = 1 + chipLabel.length + identityPlain.length;
  const cwdBudget = Math.max(8, width - leftLen - 3);
  const cwd = session ? shortenPath(session.cwd, cwdBudget) : '';
  const gap = Math.max(1, width - leftLen - cwd.length - 1);

  return (
    <Text backgroundColor={bg} wrap="truncate">
      <Text backgroundColor={bg}> </Text>
      <Text backgroundColor={theme.chip.appBg} color={theme.chip.appFg} bold>
        {chipLabel}
      </Text>
      {session && (
        <>
          <Text backgroundColor={bg}>  </Text>
          <Text backgroundColor={bg} color={agentColor(lead!)}>
            {agentGlyph(lead!)}
          </Text>
          <Text backgroundColor={bg} color={theme.text.muted}>
            {` ${leadName} · `}
          </Text>
          <Text backgroundColor={bg} color={agentColor(teammate!)}>
            {agentGlyph(teammate!)}
          </Text>
          <Text backgroundColor={bg} color={theme.text.muted}>
            {` ${teammateLabel} · `}
          </Text>
          <Text backgroundColor={bg} color={theme.text.faint}>
            {turnsLabel}
          </Text>
        </>
      )}
      <Text backgroundColor={bg}>{' '.repeat(gap)}</Text>
      <Text backgroundColor={bg} color={theme.text.faint}>
        {cwd}
      </Text>
      <Text backgroundColor={bg}> </Text>
    </Text>
  );
}

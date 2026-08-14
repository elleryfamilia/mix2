import { Box, Text } from 'ink';
import React from 'react';
import type { SessionInfo } from '../state/store.js';
import { displayName, theme } from '../theme/theme.js';

function shortenPath(cwd: string, maxLength: number): string {
  const home = process.env['HOME'];
  let short = home && cwd.startsWith(home) ? '~' + cwd.slice(home.length) : cwd;
  if (short.length > maxLength) {
    short = '…' + short.slice(short.length - maxLength + 1);
  }
  return short;
}

/** App chrome: inverted name chip, lead/teammate note, right-aligned cwd,
 * hairline underneath (Design 1c). */
export function Header({
  session,
  width,
}: {
  session?: SessionInfo;
  width: number;
}): React.JSX.Element {
  const parts: string[] = [];
  if (session) {
    parts.push(`${displayName(session.lead.kind)} lead`);
    parts.push(
      session.teammate.available
        ? `${displayName(session.teammate.kind)} teammate`
        : `${displayName(session.teammate.kind)} unavailable`,
    );
  }
  const identity = session ? `  ${parts.join(' · ')}` : '';
  // The header must never wrap: cap the cwd to the space left of the
  // identity block, ellipsizing from the left (the tail is the useful part).
  const cwdBudget = Math.max(8, width - 2 - ' cladex '.length - identity.length - 3);
  const cwd = session ? shortenPath(session.cwd, cwdBudget) : '';
  return (
    <Box flexDirection="column">
      <Box justifyContent="space-between" paddingX={1}>
        <Text wrap="truncate">
          <Text backgroundColor={theme.chip.appBg} color={theme.chip.appFg} bold>
            {' cladex '}
          </Text>
          {session && <Text color={theme.text.muted}>{identity}</Text>}
        </Text>
        <Text wrap="truncate" color={theme.text.faint}>
          {cwd}
        </Text>
      </Box>
      <Text color={theme.border.hairline}>{'─'.repeat(Math.max(0, width))}</Text>
    </Box>
  );
}

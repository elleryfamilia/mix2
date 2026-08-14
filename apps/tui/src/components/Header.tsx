import { Box, Text } from 'ink';
import React from 'react';
import type { SessionInfo } from '../state/store.js';
import { displayName, theme } from '../theme/theme.js';

function shortenPath(cwd: string): string {
  const home = process.env['HOME'];
  if (home && cwd.startsWith(home)) return '~' + cwd.slice(home.length);
  return cwd;
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
  const cwd = session ? shortenPath(session.cwd) : '';
  return (
    <Box flexDirection="column">
      <Box justifyContent="space-between" paddingX={1}>
        <Text>
          <Text backgroundColor={theme.chip.appBg} color={theme.chip.appFg} bold>
            {' cladex '}
          </Text>
          {session && <Text color={theme.text.muted}>  {parts.join(' · ')}</Text>}
        </Text>
        <Text color={theme.text.faint}>{cwd}</Text>
      </Box>
      <Text color={theme.border.hairline}>{'─'.repeat(Math.max(0, width))}</Text>
    </Box>
  );
}

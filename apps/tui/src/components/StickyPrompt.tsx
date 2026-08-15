import { Text } from 'ink';
import React from 'react';
import { glyphs, theme } from '../theme/theme.js';

/**
 * The anchored prompt bar: when the question you're reading the answer to
 * has scrolled out of view, its first line stays pinned directly under the
 * header. It updates as you scroll through history, and clicking it jumps
 * back to that prompt. Rendered in the chrome background so it reads as an
 * extension of the header, not a duplicated message.
 */
export function StickyPrompt({ text, width }: { text: string; width: number }): React.JSX.Element {
  const bg = theme.status.barBg;
  const hint = '↑ jump';
  const budget = Math.max(8, width - 2 - 2 - hint.length - 3);
  const prompt = text.length > budget ? text.slice(0, budget - 1) + '…' : text;
  const gap = Math.max(1, width - 2 - 2 - prompt.length - hint.length);
  return (
    <Text backgroundColor={bg} wrap="truncate">
      <Text backgroundColor={bg}> </Text>
      <Text backgroundColor={bg} color={theme.text.faint}>
        {glyphs.prompt}{' '}
      </Text>
      <Text backgroundColor={bg} color={theme.text.muted}>
        {prompt}
      </Text>
      <Text backgroundColor={bg}>{' '.repeat(gap)}</Text>
      <Text backgroundColor={bg} color={theme.text.faint}>
        {hint}
      </Text>
      <Text backgroundColor={bg}> </Text>
    </Text>
  );
}

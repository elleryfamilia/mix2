import { Box, Text } from 'ink';
import React from 'react';
import { glyphs, theme } from '../theme/theme.js';
import { splitAtCursor, type EditorState } from './editing.js';

/**
 * The input composer (Design 1c): a bold `❯` with a block cursor. Multiline
 * content renders with a two-space continuation indent; soft wrapping is
 * delegated to Ink. Editing state lives in App; this component only renders.
 */
export function Composer({
  editor,
  active,
  width,
}: {
  editor: EditorState;
  /** False while a turn is running: input is still captured but shown dim. */
  active: boolean;
  width: number;
}): React.JSX.Element {
  const { before, at, after } = splitAtCursor(editor);
  const promptColor = active ? theme.text.primary : theme.text.faint;
  const textColor = active ? theme.text.primary : theme.text.muted;
  return (
    <Box paddingX={2} width={width}>
      <Text color={promptColor} bold>
        {glyphs.prompt}{' '}
      </Text>
      <Box flexGrow={1}>
        <Text wrap="wrap">
          <Text color={textColor}>{before}</Text>
          {active ? (
            <Text color={theme.chip.appFg} backgroundColor={theme.chip.appBg}>
              {at === '\n' ? ' ' : at}
            </Text>
          ) : (
            <Text color={textColor}>{at}</Text>
          )}
          <Text color={textColor}>{after}</Text>
        </Text>
      </Box>
    </Box>
  );
}

/** Rows the composer needs at a given width (for viewport math). */
export function composerHeight(editor: EditorState, width: number): number {
  const usable = Math.max(8, width - 4);
  let rows = 0;
  for (const line of editor.text.split('\n')) {
    rows += Math.max(1, Math.ceil((line.length + 1) / usable));
  }
  return Math.max(1, rows);
}

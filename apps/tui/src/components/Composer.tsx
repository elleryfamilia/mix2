import { Box, Text } from 'ink';
import React from 'react';
import { glyphs, theme } from '../theme/theme.js';
import { splitAtCursor, type EditorState } from './editing.js';
import { slashCommandLength, splitForHighlight } from './slash.js';

/**
 * The input composer: a bold `❯` with a block cursor inside a rounded
 * frame, so what you type is visually unmistakable from what the team
 * says. Multiline content soft-wraps via Ink. Editing state lives in App;
 * this component only renders.
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
  // A recognized slash command lights up in the team accent, so valid
  // commands are visibly acknowledged before Enter.
  const commandLength = slashCommandLength(editor.text);
  const segment = (text: string, offset: number): React.ReactNode => {
    const [command, plain] = splitForHighlight(text, offset, commandLength);
    if (command.length === 0) return <Text color={textColor}>{text}</Text>;
    return (
      <>
        <Text color={theme.agent.team} bold>
          {command}
        </Text>
        <Text color={textColor}>{plain}</Text>
      </>
    );
  };
  return (
    <Box
      borderStyle="round"
      borderColor={active ? theme.text.faint : theme.border.subtle}
      paddingX={1}
      width={width}
    >
      <Text color={promptColor} bold>
        {glyphs.prompt}{' '}
      </Text>
      <Box flexGrow={1}>
        <Text wrap="wrap">
          {segment(before, 0)}
          {active ? (
            <Text color={theme.chip.appFg} backgroundColor={theme.chip.appBg}>
              {at === '\n' ? ' ' : at}
            </Text>
          ) : (
            segment(at, before.length)
          )}
          {segment(after, before.length + 1)}
        </Text>
      </Box>
    </Box>
  );
}

/** Rows the composer needs at a given width, including its frame. */
export function composerHeight(editor: EditorState, width: number): number {
  const usable = Math.max(8, width - 6);
  let rows = 0;
  for (const line of editor.text.split('\n')) {
    rows += Math.max(1, Math.ceil((line.length + 1) / usable));
  }
  return Math.max(1, rows) + 2; // top + bottom border
}

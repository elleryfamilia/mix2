import { Text } from 'ink';
import React from 'react';
import type { Line } from '../render/lines.js';

/** Render one styled line. Empty lines still occupy a row. */
export function LineView({ line }: { line: Line }): React.JSX.Element {
  const text = line.map((s) => s.text).join('');
  if (text.length === 0) {
    return <Text> </Text>;
  }
  return (
    <Text wrap="truncate">
      {line.map((s, i) => (
        <Text
          key={i}
          color={s.color}
          backgroundColor={s.bgColor}
          bold={s.bold}
          italic={s.italic}
          inverse={s.inverse}
        >
          {s.text}
        </Text>
      ))}
    </Text>
  );
}

/**
 * Slash-command vocabulary, shared by the composer's syntax highlighting
 * and the status-bar hint. Keep in sync with `runSlashCommand` in App.tsx.
 */
export const SLASH_COMMANDS = ['exit', 'quit', 'q', 'help', 'clear', 'copy', 'model', 'activity', 'team'] as const;

/**
 * If the composer text begins with a recognized slash command, return the
 * length of that command token (including the `/`); otherwise 0. Partial
 * or unknown tokens return 0 — the highlight is the "we recognize this"
 * signal, so it only appears for commands that would actually run.
 */
export function slashCommandLength(text: string): number {
  if (!text.startsWith('/')) return 0;
  const token = text.match(/^\/(\S*)/)?.[1] ?? '';
  return (SLASH_COMMANDS as readonly string[]).includes(token) ? token.length + 1 : 0;
}

/**
 * Split a composer text segment (starting at `offset` within the full
 * text) into [highlighted, plain] parts given the command token length.
 */
export function splitForHighlight(
  text: string,
  offset: number,
  commandLength: number,
): [string, string] {
  const split = Math.min(Math.max(commandLength - offset, 0), text.length);
  return [text.slice(0, split), text.slice(split)];
}

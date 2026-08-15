/**
 * Clipboard writing without capturing the mouse.
 *
 * mix2 never enables mouse tracking, so the terminal's own drag-selection
 * (and its copy-on-select setting, where the terminal offers one) keeps
 * working. For in-app copying we write OSC 52 — the terminal-native
 * clipboard escape, which works locally and over SSH in iTerm2, kitty,
 * Ghostty, WezTerm, alacritty — and additionally invoke the platform
 * clipboard tool when one exists, covering terminals without OSC 52
 * support (e.g. macOS Terminal.app via pbcopy).
 */
import { spawn } from 'node:child_process';

export function copyToClipboard(text: string): void {
  if (process.env['VITEST']) return; // never touch the real clipboard in tests

  try {
    const b64 = Buffer.from(text, 'utf8').toString('base64');
    process.stdout.write(`\x1b]52;c;${b64}\x07`);
  } catch {
    // OSC 52 is best-effort
  }

  const candidates: Array<[string, string[]]> =
    process.platform === 'darwin'
      ? [['pbcopy', []]]
      : [
          ['wl-copy', []],
          ['xclip', ['-selection', 'clipboard']],
        ];
  for (const [command, args] of candidates) {
    try {
      const child = spawn(command, args, { stdio: ['pipe', 'ignore', 'ignore'] });
      child.on('error', () => {});
      child.stdin.on('error', () => {});
      child.stdin.end(text);
      return; // first tool that spawns wins; errors fall through silently
    } catch {
      // try the next tool
    }
  }
}

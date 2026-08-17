/**
 * A one-line yes/no question on the normal screen, asked before the TUI
 * takes over the terminal. Deliberately conservative: only an explicit
 * `y`/`yes` is a yes, because keystrokes typed while mix2 was starting
 * are still buffered and a stray Enter must never launch an installer.
 */
import { createInterface } from 'node:readline';

export type YesNo = 'yes' | 'no' | 'quit';

export interface AskOptions {
  input: NodeJS.ReadableStream;
  output: NodeJS.WritableStream;
  question: string;
}

export function askYesNo(options: AskOptions): Promise<YesNo> {
  return new Promise((resolve) => {
    const rl = createInterface({ input: options.input, output: options.output });
    let settled = false;
    const finish = (value: YesNo) => {
      if (settled) return;
      settled = true;
      rl.close();
      resolve(value);
    };
    // Ctrl+D ends the stream; Ctrl+C makes readline close itself when no
    // SIGINT listener is registered. Both mean "let me out".
    rl.on('close', () => finish('quit'));
    rl.question(options.question, (answer) => {
      const text = answer.trim().toLowerCase();
      finish(text === 'y' || text === 'yes' ? 'yes' : 'no');
    });
  });
}

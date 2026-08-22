/**
 * Prompt history for the composer's up-arrow recall, persisted across
 * sessions in one JSONL file under the XDG state dir. Entries carry the
 * project cwd, so each project recalls its own prompts (the Claude Code
 * shape). The file is best-effort state: IO failures and malformed lines
 * are silently ignored — history must never break a submit.
 */
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

/** Entries offered for recall per project. */
export const HISTORY_LIMIT = 100;

export interface HistoryStore {
  /** This project's prompts, oldest first. */
  load(): string[];
  append(text: string): void;
}

export function historyFilePath(env: NodeJS.ProcessEnv = process.env): string {
  const base = env.XDG_STATE_HOME?.trim()
    ? env.XDG_STATE_HOME
    : path.join(os.homedir(), '.local', 'state');
  return path.join(base, 'mix2', 'history.jsonl');
}

/** Parse the JSONL body into this project's prompts, oldest first.
 * Consecutive repeats collapse — recalling "yes" three times is noise. */
export function parseHistory(content: string, cwd: string, limit = HISTORY_LIMIT): string[] {
  const texts: string[] = [];
  for (const line of content.split('\n')) {
    if (!line.trim()) continue;
    try {
      const entry = JSON.parse(line) as { cwd?: unknown; text?: unknown };
      if (entry.cwd !== cwd || typeof entry.text !== 'string' || entry.text === '') continue;
      if (texts[texts.length - 1] === entry.text) continue;
      texts.push(entry.text);
    } catch {
      // Malformed line — skip it, keep the rest.
    }
  }
  return texts.slice(-limit);
}

export function entryLine(cwd: string, text: string): string {
  return `${JSON.stringify({ cwd, text, at: new Date().toISOString() })}\n`;
}

export function fileHistoryStore(cwd: string, file = historyFilePath()): HistoryStore {
  return {
    load() {
      try {
        return parseHistory(fs.readFileSync(file, 'utf8'), cwd);
      } catch {
        return [];
      }
    },
    append(text: string) {
      try {
        fs.mkdirSync(path.dirname(file), { recursive: true });
        fs.appendFileSync(file, entryLine(cwd, text));
      } catch {
        // Best-effort only.
      }
    },
  };
}

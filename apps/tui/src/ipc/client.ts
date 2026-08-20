/**
 * CoreClient owns the mix2-core child process and the JSONL exchange with
 * it. The UI layer never sees provider-specific data — only validated
 * protocol events.
 */
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { existsSync } from 'node:fs';
import { appendFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { PROTOCOL_VERSION, parseEventLine, type Command, type CoreEvent } from './protocol.js';

export interface CoreClientOptions {
  lead?: string;
  cwd?: string;
  debug?: boolean;
  /** A human is at the terminal (enables the selection handshake). */
  interactive?: boolean;
  /** Force the team-selection handshake even with an explicit config. */
  pickTeam?: boolean;
  /** Explicit path to the mix2-core binary (MIX2_CORE_BIN wins). */
  corePath?: string;
  /** Path to append raw IPC traffic to, for debugging. */
  logPath?: string;
}

export interface CoreClientHandlers {
  onEvent: (event: CoreEvent) => void;
  /** Called when the core exits before shutdown was requested; `stderr`
   * is the tail of the core's stderr (the loader/panic message when the
   * binary can't run at all). */
  onExit: (code: number | null, stderr: string) => void;
}

/** Locate the mix2-core binary: explicit option, then $MIX2_CORE_BIN,
 * then alongside this file (release installs ship the core next to the
 * bundled TUI), then dev target dirs walking up from here, then PATH. */
export function locateCore(explicit?: string): string {
  const env = process.env['MIX2_CORE_BIN'];
  if (explicit) return explicit;
  if (env) return env;
  const here = path.dirname(fileURLToPath(import.meta.url));
  const candidates: string[] = [path.join(here, 'mix2-core')];
  let dir = here;
  for (let i = 0; i < 6; i++) {
    candidates.push(path.join(dir, 'target', 'release', 'mix2-core'));
    candidates.push(path.join(dir, 'target', 'debug', 'mix2-core'));
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  for (const candidate of candidates) {
    if (existsSync(candidate)) return candidate;
  }
  return 'mix2-core';
}

export class CoreClient {
  private child: ChildProcessWithoutNullStreams | null = null;
  private buffer = '';
  private stderrTail: string[] = [];
  private shuttingDown = false;
  private readonly options: CoreClientOptions;
  private readonly handlers: CoreClientHandlers;

  constructor(options: CoreClientOptions, handlers: CoreClientHandlers) {
    this.options = options;
    this.handlers = handlers;
  }

  start(): void {
    const bin = locateCore(this.options.corePath);
    const child = spawn(bin, ['serve'], {
      stdio: ['pipe', 'pipe', 'pipe'],
      cwd: this.options.cwd ?? process.cwd(),
    });
    this.child = child;

    // A dying core can EPIPE our writes asynchronously; swallow stream
    // errors so they never become uncaught exceptions.
    child.stdin.on('error', () => {});

    child.stdout.setEncoding('utf8');
    child.stdout.on('data', (chunk: string) => {
      this.buffer += chunk;
      let index: number;
      while ((index = this.buffer.indexOf('\n')) >= 0) {
        const line = this.buffer.slice(0, index);
        this.buffer = this.buffer.slice(index + 1);
        this.log('<<', line);
        const event = parseEventLine(line);
        if (!event) continue;
        this.handlers.onEvent(event);
      }
    });
    child.stderr.setEncoding('utf8');
    child.stderr.on('data', (chunk: string) => {
      this.log('!!', chunk.trimEnd());
      for (const line of chunk.split('\n')) {
        if (!line.trim()) continue;
        this.stderrTail.push(line.slice(0, 300));
        if (this.stderrTail.length > 8) this.stderrTail.shift();
      }
    });

    child.on('error', () => {
      this.handlers.onEvent({
        type: 'fatal',
        message:
          `could not start the mix2 runtime (${bin}). ` +
          'Build it with `cargo build` or set MIX2_CORE_BIN.',
      });
    });
    child.on('exit', (code) => {
      if (!this.shuttingDown) this.handlers.onExit(code, this.stderrTail.join('\n'));
    });

    this.send({
      type: 'initialize',
      protocol: PROTOCOL_VERSION,
      lead: this.options.lead,
      cwd: this.options.cwd ?? process.cwd(),
      debug: this.options.debug ?? false,
      interactive: this.options.interactive ?? false,
      pick_team: this.options.pickTeam ?? false,
    });
  }

  send(command: Command): void {
    const line = JSON.stringify(command);
    this.log('>>', line);
    // The core can die or its stdin close at any moment; failing to send
    // must never crash the UI.
    try {
      if (this.child?.stdin.writable) {
        this.child.stdin.write(line + '\n');
      }
    } catch {
      // child gone — nothing to deliver to
    }
  }

  submit(id: string, text: string): void {
    this.send({ type: 'submit', id, text });
  }

  selectTeam(one: string, two: string, leadSlot: string): void {
    this.send({ type: 'select_team', one, two, lead_slot: leadSlot });
  }

  cancel(turnId: string): void {
    this.send({ type: 'cancel', turn_id: turnId });
  }

  shutdown(): void {
    // Idempotent: both the quit keybinding and the waitUntilExit cleanup
    // call this; only the first does the work.
    if (this.shuttingDown) return;
    this.shuttingDown = true;
    try {
      this.send({ type: 'shutdown' });
      this.child?.stdin.end();
    } catch {
      // already gone
    }
    const child = this.child;
    if (child) {
      setTimeout(() => {
        if (child.exitCode === null) child.kill('SIGKILL');
      }, 1500).unref();
    }
  }

  private log(direction: string, line: string): void {
    if (!this.options.logPath || !line) return;
    try {
      appendFileSync(this.options.logPath, `${new Date().toISOString()} ${direction} ${line}\n`);
    } catch {
      // logging must never break the app
    }
  }
}

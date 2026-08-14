/**
 * CoreClient owns the cladex-core child process and the JSONL exchange with
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
  /** Explicit path to the cladex-core binary (CLADEX_CORE_BIN wins). */
  corePath?: string;
  /** Path to append raw IPC traffic to, for debugging. */
  logPath?: string;
}

export interface CoreClientHandlers {
  onEvent: (event: CoreEvent) => void;
  /** Called when the core exits before shutdown was requested. */
  onExit: (code: number | null) => void;
}

/** Locate the cladex-core binary: explicit option, then $CLADEX_CORE_BIN,
 * then dev target dirs relative to this package, then PATH. */
export function locateCore(explicit?: string): string {
  const env = process.env['CLADEX_CORE_BIN'];
  if (explicit) return explicit;
  if (env) return env;
  const here = path.dirname(fileURLToPath(import.meta.url));
  const candidates = [
    path.resolve(here, '../../../../target/release/cladex-core'),
    path.resolve(here, '../../../../target/debug/cladex-core'),
    path.resolve(here, '../../../../../target/release/cladex-core'),
    path.resolve(here, '../../../../../target/debug/cladex-core'),
  ];
  for (const candidate of candidates) {
    if (existsSync(candidate)) return candidate;
  }
  return 'cladex-core';
}

export class CoreClient {
  private child: ChildProcessWithoutNullStreams | null = null;
  private buffer = '';
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

    child.stdout.setEncoding('utf8');
    child.stdout.on('data', (chunk: string) => {
      this.buffer += chunk;
      let index: number;
      while ((index = this.buffer.indexOf('\n')) >= 0) {
        const line = this.buffer.slice(0, index);
        this.buffer = this.buffer.slice(index + 1);
        this.log('<<', line);
        const event = parseEventLine(line);
        if (event) this.handlers.onEvent(event);
      }
    });
    child.stderr.setEncoding('utf8');
    child.stderr.on('data', (chunk: string) => this.log('!!', chunk.trimEnd()));

    child.on('error', () => {
      this.handlers.onEvent({
        type: 'fatal',
        message:
          `could not start the Cladex runtime (${bin}). ` +
          'Build it with `cargo build` or set CLADEX_CORE_BIN.',
      });
    });
    child.on('exit', (code) => {
      if (!this.shuttingDown) this.handlers.onExit(code);
    });

    this.send({
      type: 'initialize',
      protocol: PROTOCOL_VERSION,
      lead: this.options.lead,
      cwd: this.options.cwd ?? process.cwd(),
      debug: this.options.debug ?? false,
    });
  }

  send(command: Command): void {
    const line = JSON.stringify(command);
    this.log('>>', line);
    this.child?.stdin.write(line + '\n');
  }

  submit(id: string, text: string): void {
    this.send({ type: 'submit', id, text });
  }

  cancel(turnId: string): void {
    this.send({ type: 'cancel', turn_id: turnId });
  }

  shutdown(): void {
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

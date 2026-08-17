#!/usr/bin/env node
/**
 * `mix2` — one conversational interface backed by two coding agents.
 * This entry point parses arguments, offers a pending update, launches the
 * Rust core, and renders the Ink app in the terminal's alternate screen
 * buffer, restoring the terminal on any exit path.
 */
import { render } from 'ink';
import path from 'node:path';
import React from 'react';
import { parseArgs } from './args.js';
import { App } from './components/App.js';
import { CoreClient } from './ipc/client.js';
import type { CoreEvent } from './ipc/protocol.js';
import { FilteredStdin } from './mouse/filteredStdin.js';
import { defaultDeps, offerUpdateAtStartup, runUpdateCommand } from './update/flow.js';

const parsed = parseArgs(process.argv.slice(2));
if (parsed.kind === 'exit') {
  if (parsed.stdout) process.stdout.write(parsed.stdout);
  if (parsed.stderr) process.stderr.write(parsed.stderr);
  process.exit(parsed.code);
}
if (parsed.kind === 'update') {
  // Explicit exit: a kept-alive HTTPS socket would otherwise hold the loop.
  process.exit(await runUpdateCommand(defaultDeps()));
}
const args = parsed.args;
const cwd = args.cwd ? path.resolve(args.cwd) : process.cwd();

// Before touching the terminal: is there a newer release, and does the
// user want it now? (Once a day at most; silent when offline or opted out.)
const startup = await offerUpdateAtStartup(process.argv.slice(2), defaultDeps({ debug: args.debug }));
if (startup.action === 'exit') process.exit(startup.code);

// 1049: alternate screen buffer.
// 1002/1006: SGR mouse reporting — drag-selection with copy-on-release and
//            wheel scrolling are handled in-app (hold Shift, or Option on
//            iTerm2, for the terminal's native selection instead).
// 1007: alternate scroll — wheel-as-arrows fallback for terminals without
//       mouse reporting; inert while 1002 is active.
const ALT_SCREEN_ON = '\x1b[?1049h\x1b[?1002h\x1b[?1006h\x1b[?1007h\x1b[H';
const ALT_SCREEN_OFF = '\x1b[?1006l\x1b[?1002l\x1b[?1007l\x1b[?1049l';
let altScreen = false;

function enterAltScreen(): void {
  if (!altScreen && process.stdout.isTTY) {
    process.stdout.write(ALT_SCREEN_ON);
    altScreen = true;
  }
}

function leaveAltScreen(): void {
  if (altScreen) {
    process.stdout.write(ALT_SCREEN_OFF);
    altScreen = false;
  }
}

// The terminal must be restored on every exit path, including crashes —
// never leave the user's shell in the alternate screen or raw mode.
process.on('exit', leaveAltScreen);
process.on('SIGTERM', () => {
  leaveAltScreen();
  process.exit(143);
});
process.on('uncaughtException', (error) => {
  leaveAltScreen();
  console.error('mix2 crashed:', error);
  process.exit(1);
});

let handlers: { onEvent: (e: CoreEvent) => void; onExit: (code: number | null, stderr: string) => void } = {
  onEvent: () => {},
  onExit: () => {},
};

const client = new CoreClient(
  {
    lead: args.lead,
    cwd,
    debug: args.debug,
    corePath: args.core,
    logPath: args.debug ? `/tmp/mix2-tui-${process.pid}.log` : undefined,
  },
  {
    onEvent: (event) => handlers.onEvent(event),
    onExit: (code, stderr) => handlers.onExit(code, stderr),
  },
);

enterAltScreen();
client.start();

// Mouse events are filtered out of stdin before Ink parses keyboard input;
// the App receives them on a side channel for selection + wheel scrolling.
const filteredStdin = process.stdin.isTTY ? new FilteredStdin(process.stdin) : undefined;

const app = render(
  <App
    client={client}
    bind={(h) => {
      handlers = h;
    }}
    mouse={filteredStdin?.mouse}
  />,
  {
    exitOnCtrlC: false,
    ...(filteredStdin ? { stdin: filteredStdin as unknown as NodeJS.ReadStream } : {}),
  },
);

app.waitUntilExit().then(() => {
  filteredStdin?.detach();
  leaveAltScreen();
  client.shutdown();
});

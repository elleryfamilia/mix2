#!/usr/bin/env node
/**
 * `mix2` — one conversational interface backed by two coding agents.
 * This entry point parses arguments, launches the Rust core, and renders
 * the Ink app in the terminal's alternate screen buffer, restoring the
 * terminal on any exit path.
 */
import { render } from 'ink';
import path from 'node:path';
import React from 'react';
import { App } from './components/App.js';
import { CoreClient } from './ipc/client.js';
import type { CoreEvent } from './ipc/protocol.js';

const HELP = `mix2 — talk to an AI engineering team in your terminal

Usage:
  mix2 [options]

Options:
  -l, --lead <agent>   Lead agent: claude or codex (default: configured, else claude)
      --cwd <path>     Project directory (default: current directory)
      --debug          Verbose runtime logging (IPC log in /tmp/mix2)
      --core <path>    Path to the mix2-core binary
  -h, --help           Show this help
  -V, --version        Show version

Keys:
  Enter submit · Ctrl+J newline · Esc cancel · Ctrl+T team panel
  PageUp/PageDown scroll · Ctrl+C cancel (twice: quit) · Ctrl+Q quit
`;

interface CliArgs {
  lead?: string;
  cwd?: string;
  debug: boolean;
  core?: string;
}

function parseArgs(argv: string[]): CliArgs {
  const args: CliArgs = { debug: false };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]!;
    switch (arg) {
      case '-l':
      case '--lead':
        args.lead = argv[++i];
        break;
      case '--cwd':
        args.cwd = argv[++i];
        break;
      case '--debug':
        args.debug = true;
        break;
      case '--core':
        args.core = argv[++i];
        break;
      case '-h':
      case '--help':
        process.stdout.write(HELP);
        process.exit(0);
        break;
      case '-V':
      case '--version':
        process.stdout.write('mix2 0.1.0\n');
        process.exit(0);
        break;
      default:
        process.stderr.write(`unknown option: ${arg}\n\n${HELP}`);
        process.exit(2);
    }
  }
  if (args.lead && !['claude', 'codex'].includes(args.lead)) {
    process.stderr.write(`invalid --lead '${args.lead}' (expected claude or codex)\n`);
    process.exit(2);
  }
  return args;
}

const args = parseArgs(process.argv.slice(2));
const cwd = args.cwd ? path.resolve(args.cwd) : process.cwd();

// 1049: alternate screen buffer. 1007: alternate scroll — the terminal
// translates mouse-wheel ticks into arrow keys while in the alt screen,
// which the app maps to conversation scrolling.
const ALT_SCREEN_ON = '\x1b[?1049h\x1b[?1007h\x1b[H';
const ALT_SCREEN_OFF = '\x1b[?1007l\x1b[?1049l';
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

let handlers: { onEvent: (e: CoreEvent) => void; onExit: (code: number | null) => void } = {
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
    onExit: (code) => handlers.onExit(code),
  },
);

enterAltScreen();
client.start();

const app = render(
  <App
    client={client}
    bind={(h) => {
      handlers = h;
    }}
  />,
  { exitOnCtrlC: false },
);

app.waitUntilExit().then(() => {
  leaveAltScreen();
  client.shutdown();
});

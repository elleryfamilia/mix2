/**
 * Command-line parsing for `mix2`. Pure: returns a result instead of
 * exiting so it can be unit-tested; `cli.tsx` acts on the result.
 */
import { VERSION } from './version.js';

export const HELP = `mix2 — talk to an AI engineering team in your terminal

Usage:
  mix2 [options]
  mix2 update          Update to the latest release

Options:
  -l, --lead <slot>    Lead slot: one or two; agent names like claude/codex
                       also work while unambiguous (default: configured)
      --cwd <path>     Project directory (default: current directory)
      --debug          Verbose runtime logging (IPC log in /tmp/mix2)
      --core <path>    Path to the mix2-core binary
  -h, --help           Show this help
  -V, --version        Show version

Keys:
  Enter submit · Ctrl+J newline · Esc cancel · Ctrl+T team panel
  PageUp/PageDown scroll · Ctrl+C cancel (twice: quit) · Ctrl+Q quit
`;

export interface CliArgs {
  lead?: string;
  cwd?: string;
  debug: boolean;
  core?: string;
}

export type ParseResult =
  | { kind: 'run'; args: CliArgs }
  | { kind: 'update' }
  | { kind: 'exit'; code: number; stdout?: string; stderr?: string };

function missingValue(flag: string): ParseResult {
  return { kind: 'exit', code: 2, stderr: `missing value for ${flag}\n\n${HELP}` };
}

export function parseArgs(argv: string[]): ParseResult {
  if (argv[0] === 'update') {
    if (argv.length > 1) {
      return { kind: 'exit', code: 2, stderr: `mix2 update takes no arguments\n\n${HELP}` };
    }
    return { kind: 'update' };
  }
  const args: CliArgs = { debug: false };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]!;
    switch (arg) {
      case '-l':
      case '--lead':
        if (argv[i + 1] === undefined) return missingValue(arg);
        args.lead = argv[++i];
        break;
      case '--cwd':
        if (argv[i + 1] === undefined) return missingValue(arg);
        args.cwd = argv[++i];
        break;
      case '--debug':
        args.debug = true;
        break;
      case '--core':
        if (argv[i + 1] === undefined) return missingValue(arg);
        args.core = argv[++i];
        break;
      case '-h':
      case '--help':
        return { kind: 'exit', code: 0, stdout: HELP };
      case '-V':
      case '-v':
      case '--version':
        return { kind: 'exit', code: 0, stdout: `mix2 ${VERSION}\n` };
      default:
        return { kind: 'exit', code: 2, stderr: `unknown option: ${arg}\n\n${HELP}` };
    }
  }
  // --lead values are passed through as-is: the core's registry owns
  // harness-name validation, so its error text always reflects what is
  // actually registered (surfaced as a fatal event before ready).
  return { kind: 'run', args };
}

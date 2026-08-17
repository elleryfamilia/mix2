import { describe, expect, it } from 'vitest';
import { parseArgs } from './args.js';
import { VERSION } from './version.js';

describe('parseArgs', () => {
  it('runs the app with defaults when given nothing', () => {
    expect(parseArgs([])).toEqual({ kind: 'run', args: { debug: false } });
  });

  it('collects the run options', () => {
    expect(parseArgs(['--lead', 'codex', '--cwd', '/x', '--debug', '--core', '/c'])).toEqual({
      kind: 'run',
      args: { lead: 'codex', cwd: '/x', debug: true, core: '/c' },
    });
  });

  it('recognises the update subcommand', () => {
    expect(parseArgs(['update'])).toEqual({ kind: 'update' });
  });

  it('rejects extra arguments after update', () => {
    const r = parseArgs(['update', '--now']);
    expect(r.kind).toBe('exit');
    if (r.kind === 'exit') expect(r.code).toBe(2);
  });

  it('prints the version for -V, -v and --version', () => {
    for (const flag of ['-V', '-v', '--version']) {
      expect(parseArgs([flag])).toEqual({ kind: 'exit', code: 0, stdout: `mix2 ${VERSION}\n` });
    }
  });

  it('mentions the update subcommand in --help', () => {
    const r = parseArgs(['--help']);
    expect(r.kind).toBe('exit');
    if (r.kind === 'exit') {
      expect(r.code).toBe(0);
      expect(r.stdout).toContain('mix2 update');
    }
  });

  it('rejects unknown options and bad leads with exit code 2', () => {
    expect(parseArgs(['--bogus'])).toMatchObject({ kind: 'exit', code: 2 });
    expect(parseArgs(['--lead', 'gemini'])).toMatchObject({ kind: 'exit', code: 2 });
  });
});

import { describe, expect, it } from 'vitest';
import { parseArgs } from './args.js';
import { VERSION } from './version.js';

describe('parseArgs', () => {
  it('runs the app with defaults when given nothing', () => {
    expect(parseArgs([])).toEqual({ kind: 'run', args: { debug: false, pickTeam: false } });
  });

  it('collects the run options', () => {
    expect(parseArgs(['--lead', 'codex', '--cwd', '/x', '--debug', '--core', '/c'])).toEqual({
      kind: 'run',
      args: { lead: 'codex', cwd: '/x', debug: true, pickTeam: false, core: '/c' },
    });
  });

  it('rejects a value-taking flag with no value', () => {
    for (const flag of ['--lead', '-l', '--cwd', '--core']) {
      const r = parseArgs([flag]);
      expect(r.kind).toBe('exit');
      if (r.kind === 'exit') {
        expect(r.code).toBe(2);
        expect(r.stderr).toContain(`missing value for ${flag}`);
      }
    }
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

  it('rejects unknown options with exit code 2', () => {
    expect(parseArgs(['--bogus'])).toMatchObject({ kind: 'exit', code: 2 });
  });

  it('passes lead values through — the core registry owns validation', () => {
    expect(parseArgs(['--lead', 'one'])).toMatchObject({
      kind: 'run',
      args: { lead: 'one' },
    });
    expect(parseArgs(['--lead', 'gemini'])).toMatchObject({
      kind: 'run',
      args: { lead: 'gemini' },
    });
  });
});

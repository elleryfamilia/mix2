import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { VERSION } from './version.js';

describe('VERSION', () => {
  it('matches package.json (a stale copy would make a release offer itself as an update)', () => {
    const here = path.dirname(fileURLToPath(import.meta.url));
    const pkg = JSON.parse(readFileSync(path.join(here, '..', 'package.json'), 'utf8')) as {
      version: string;
    };
    expect(VERSION).toBe(pkg.version);
  });
});

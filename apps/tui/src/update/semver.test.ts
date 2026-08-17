import { describe, expect, it } from 'vitest';
import { compareVersions, parseVersion } from './semver.js';

describe('parseVersion', () => {
  it('parses dotted numeric versions with an optional v prefix', () => {
    expect(parseVersion('0.3.0')).toEqual({ numbers: [0, 3, 0], prerelease: undefined });
    expect(parseVersion('v1.2.3')).toEqual({ numbers: [1, 2, 3], prerelease: undefined });
  });

  it('keeps a pre-release suffix', () => {
    expect(parseVersion('1.0.0-rc.1')).toEqual({ numbers: [1, 0, 0], prerelease: 'rc.1' });
  });

  it('rejects garbage', () => {
    expect(parseVersion('latest')).toBeUndefined();
    expect(parseVersion('')).toBeUndefined();
    expect(parseVersion('1.x')).toBeUndefined();
  });
});

describe('compareVersions', () => {
  it('orders numerically per component, not lexically', () => {
    expect(compareVersions('0.10.0', '0.9.0')).toBe(1);
    expect(compareVersions('0.9.0', '0.10.0')).toBe(-1);
    expect(compareVersions('1.0.0', '1.0.0')).toBe(0);
  });

  it('treats missing trailing components as zero', () => {
    expect(compareVersions('1.0', '1.0.0')).toBe(0);
    expect(compareVersions('1.0.1', '1.0')).toBe(1);
  });

  it('ignores a leading v', () => {
    expect(compareVersions('v0.4.0', '0.3.0')).toBe(1);
  });

  it('sorts a pre-release below its release', () => {
    expect(compareVersions('1.0.0-rc.1', '1.0.0')).toBe(-1);
    expect(compareVersions('1.0.0', '1.0.0-rc.1')).toBe(1);
    expect(compareVersions('1.0.0-rc.1', '1.0.0-rc.2')).toBe(-1);
  });

  it('throws on unparseable input so callers cannot compare garbage silently', () => {
    expect(() => compareVersions('latest', '1.0.0')).toThrow();
  });
});

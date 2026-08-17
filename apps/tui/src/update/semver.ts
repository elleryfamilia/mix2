/**
 * Minimal version ordering for release tags (`v0.3.0`, `1.2.3-rc.1`).
 * Only what the update check needs — no ranges, no build metadata.
 */

export interface ParsedVersion {
  numbers: number[];
  prerelease: string | undefined;
}

const VERSION_RE = /^v?(\d+(?:\.\d+)*)(?:-([0-9A-Za-z.-]+))?$/;

export function parseVersion(text: string): ParsedVersion | undefined {
  const match = VERSION_RE.exec(text.trim());
  if (!match) return undefined;
  const numbers = match[1]!.split('.').map((n) => Number.parseInt(n, 10));
  return { numbers, prerelease: match[2] };
}

/** -1 if a < b, 0 if equal, 1 if a > b. Throws on unparseable input. */
export function compareVersions(a: string, b: string): -1 | 0 | 1 {
  const pa = parseVersion(a);
  const pb = parseVersion(b);
  if (!pa || !pb) throw new Error(`cannot compare versions '${a}' and '${b}'`);
  const len = Math.max(pa.numbers.length, pb.numbers.length);
  for (let i = 0; i < len; i++) {
    const x = pa.numbers[i] ?? 0;
    const y = pb.numbers[i] ?? 0;
    if (x !== y) return x < y ? -1 : 1;
  }
  // A pre-release sorts below the bare version; two pre-releases compare
  // as strings (good enough for rc.1 < rc.2).
  if (pa.prerelease === pb.prerelease) return 0;
  if (pa.prerelease === undefined) return 1;
  if (pb.prerelease === undefined) return -1;
  return pa.prerelease < pb.prerelease ? -1 : 1;
}

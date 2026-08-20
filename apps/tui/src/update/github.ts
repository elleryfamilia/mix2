/**
 * Where releases live and how to ask GitHub which one is newest.
 *
 * `…/releases/latest` answers with a 302 whose Location names the tag.
 * That is unauthenticated, cheap, and not subject to the REST API's
 * per-IP rate limit — the right tool for a check that runs on startup.
 */

export const REPO = 'elleryfamilia/mix2';
export const LATEST_RELEASE_URL = `https://github.com/${REPO}/releases/latest`;
/** The README's install command — the manual fallback when self-update can't. */
export const INSTALL_ONE_LINER = `curl -fsSL https://raw.githubusercontent.com/${REPO}/main/install.sh | sh`;

export interface Release {
  /** Git tag, e.g. `v0.4.0`. */
  tag: string;
  /** Version without the `v`, e.g. `0.4.0`. */
  version: string;
}

/** The installer shipped with a given release. */
export function installerUrl(tag: string): string {
  return `https://github.com/${REPO}/releases/download/${encodeURIComponent(tag)}/install.sh`;
}

const TAG_RE = /^v(\d+(?:\.\d+)*(?:-[0-9A-Za-z.-]+)?)$/;

/** `https://github.com/<repo>/releases/tag/v0.4.0` → `{ tag, version }`. */
export function parseReleaseTag(location: string): Release | undefined {
  let url: URL;
  try {
    url = new URL(location);
  } catch {
    return undefined;
  }
  const parts = url.pathname.split('/');
  const tagIndex = parts.indexOf('tag');
  if (tagIndex === -1 || parts[tagIndex - 1] !== 'releases') return undefined;
  const tag = parts[tagIndex + 1];
  if (!tag || !TAG_RE.test(tag)) return undefined;
  return { tag, version: tag.slice(1) };
}

export interface FetchLatestOptions {
  fetch: typeof fetch;
  timeoutMs: number;
  env?: NodeJS.ProcessEnv;
}

/** The newest release, or `undefined` for any failure (offline, timeout,
 * unexpected response). Callers treat "unknown" as "nothing to do". */
export async function fetchLatestRelease(options: FetchLatestOptions): Promise<Release | undefined> {
  // MIX2_LATEST_RELEASE_URL: test-only override (mirrors install.sh's
  // MIX2_RELEASE_BASE_URL) so the whole flow can run against a local server.
  const url = (options.env ?? process.env)['MIX2_LATEST_RELEASE_URL'] || LATEST_RELEASE_URL;
  try {
    const response = await options.fetch(url, {
      redirect: 'manual',
      signal: AbortSignal.timeout(options.timeoutMs),
      headers: { 'user-agent': 'mix2-update-check' },
    });
    const location = response.headers.get('location');
    if (!location) return undefined;
    return parseReleaseTag(location);
  } catch {
    return undefined;
  }
}

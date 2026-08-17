import { describe, expect, it } from 'vitest';
import { fetchLatestRelease, LATEST_RELEASE_URL, parseReleaseTag } from './github.js';

const redirect = (location: string): typeof fetch =>
  (async () => new Response(null, { status: 302, headers: { location } })) as typeof fetch;

describe('parseReleaseTag', () => {
  it('extracts the tag and version from the releases/tag URL', () => {
    expect(parseReleaseTag('https://github.com/elleryfamilia/mix2/releases/tag/v0.4.0')).toEqual({
      tag: 'v0.4.0',
      version: '0.4.0',
    });
  });

  it('handles pre-release tags and URL encoding', () => {
    expect(parseReleaseTag('https://github.com/x/y/releases/tag/v1.0.0-rc.1')).toEqual({
      tag: 'v1.0.0-rc.1',
      version: '1.0.0-rc.1',
    });
    expect(parseReleaseTag('https://github.com/x/y/releases/tag/v1.0.0%2Bbuild')).toBeUndefined();
  });

  it('rejects anything that is not a version tag', () => {
    expect(parseReleaseTag('https://github.com/x/y/releases')).toBeUndefined();
    expect(parseReleaseTag('https://github.com/x/y/releases/tag/nightly')).toBeUndefined();
    expect(parseReleaseTag('not a url')).toBeUndefined();
  });
});

describe('fetchLatestRelease', () => {
  it('follows the redirect header to the latest tag', async () => {
    const calls: Array<{ url: string; init: RequestInit | undefined }> = [];
    const fetchImpl: typeof fetch = async (url, init) => {
      calls.push({ url: String(url), init });
      return new Response(null, {
        status: 302,
        headers: { location: 'https://github.com/elleryfamilia/mix2/releases/tag/v0.4.0' },
      });
    };
    await expect(fetchLatestRelease({ fetch: fetchImpl, timeoutMs: 1000, env: {} })).resolves.toEqual({
      tag: 'v0.4.0',
      version: '0.4.0',
    });
    expect(calls[0]?.url).toBe(LATEST_RELEASE_URL);
    expect(calls[0]?.init?.redirect).toBe('manual');
  });

  it('honours the MIX2_LATEST_RELEASE_URL test override', async () => {
    const urls: string[] = [];
    const fetchImpl: typeof fetch = async (url) => {
      urls.push(String(url));
      return new Response(null, { status: 302, headers: { location: 'https://x/releases/tag/v1.0.0' } });
    };
    await fetchLatestRelease({ fetch: fetchImpl, timeoutMs: 1000, env: { MIX2_LATEST_RELEASE_URL: 'http://127.0.0.1:1/latest' } });
    expect(urls).toEqual(['http://127.0.0.1:1/latest']);
  });

  it('returns undefined when there is no redirect (no releases yet, or GitHub changed shape)', async () => {
    const fetchImpl: typeof fetch = async () => new Response('<html>', { status: 200 });
    await expect(fetchLatestRelease({ fetch: fetchImpl, timeoutMs: 1000 })).resolves.toBeUndefined();
  });

  it('returns undefined when the redirect target is not a tag', async () => {
    await expect(
      fetchLatestRelease({ fetch: redirect('https://github.com/login'), timeoutMs: 1000 }),
    ).resolves.toBeUndefined();
  });

  it('returns undefined on network errors', async () => {
    const fetchImpl: typeof fetch = async () => {
      throw new Error('ENOTFOUND');
    };
    await expect(fetchLatestRelease({ fetch: fetchImpl, timeoutMs: 1000 })).resolves.toBeUndefined();
  });

  it('gives up after the timeout', async () => {
    const fetchImpl: typeof fetch = (_url, init) =>
      new Promise((_resolve, reject) => {
        init?.signal?.addEventListener('abort', () => reject(new Error('aborted')));
      });
    const started = Date.now();
    await expect(fetchLatestRelease({ fetch: fetchImpl, timeoutMs: 50 })).resolves.toBeUndefined();
    expect(Date.now() - started).toBeLessThan(1000);
  });
});

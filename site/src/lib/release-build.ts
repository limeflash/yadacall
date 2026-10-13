// Build-time fetch of the latest release. Never throws: any failure falls back to the design numbers
// and to the /releases/latest page, so a GitHub API outage cannot break the deploy.
import { API_LATEST } from './site';
import { FALLBACK, parseRelease, type ReleaseData } from './release';

let cached: Promise<ReleaseData> | undefined;

async function load(): Promise<ReleaseData> {
  try {
    const headers: Record<string, string> = {
      Accept: 'application/vnd.github+json',
      'User-Agent': 'yadacall-site-build',
      'X-GitHub-Api-Version': '2022-11-28',
    };
    const token = process.env.GITHUB_TOKEN;
    if (token) headers.Authorization = `Bearer ${token}`;
    const res = await fetch(API_LATEST, { headers, signal: AbortSignal.timeout(15000) });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const rel = parseRelease(await res.json());
    if (!rel) throw new Error('unexpected payload');
    console.log(`[release] baked v${rel.version}`);
    return rel;
  } catch (e) {
    console.warn(`[release] GitHub API unavailable (${e instanceof Error ? e.message : e}); using fallback ${FALLBACK.version}`);
    return { ...FALLBACK };
  }
}

export function getRelease(): Promise<ReleaseData> {
  cached ??= load();
  return cached;
}

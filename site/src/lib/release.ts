// Release data: parsing of GitHub's /releases/latest payload. Shared by the build (baked into the HTML)
// and the browser (refresh after load), so this file must stay free of Node and DOM APIs.

export type DlOs = 'mac' | 'win' | 'linux';

export interface Asset {
  url: string;
  /** bytes */
  size: number;
}

export interface ReleaseData {
  /** tag_name without the leading "v" */
  version: string;
  /** html_url of the release page */
  url: string;
  mac?: Asset;
  win?: Asset;
  linux?: Asset;
  deb?: Asset;
}

/** Design numbers, used when the release has no matching asset or the API is unreachable. */
export const FALLBACK: ReleaseData = {
  version: '0.1.1',
  url: 'https://github.com/limeflash/yadacall/releases/latest',
};
export const FALLBACK_MB = { mac: 21, win: 16 } as const;

interface RawAsset {
  name?: string;
  browser_download_url?: string;
  size?: number;
}

const PATTERNS: [keyof Pick<ReleaseData, 'mac' | 'win' | 'linux' | 'deb'>, RegExp][] = [
  ['mac', /_aarch64\.dmg$/],
  ['win', /_x64-setup\.exe$/],
  ['linux', /_amd64\.AppImage$/],
  ['deb', /_amd64\.deb$/],
];

export function parseRelease(raw: unknown): ReleaseData | null {
  if (!raw || typeof raw !== 'object') return null;
  const r = raw as { tag_name?: unknown; html_url?: unknown; assets?: unknown };
  if (typeof r.tag_name !== 'string' || !r.tag_name) return null;
  const out: ReleaseData = {
    version: r.tag_name.replace(/^v/i, ''),
    url: typeof r.html_url === 'string' && r.html_url.startsWith('https://github.com/') ? r.html_url : FALLBACK.url,
  };
  const assets: RawAsset[] = Array.isArray(r.assets) ? (r.assets as RawAsset[]) : [];
  for (const a of assets) {
    const name = a?.name;
    const url = a?.browser_download_url;
    if (typeof name !== 'string' || typeof url !== 'string' || !url.startsWith('https://github.com/')) continue;
    // never offer signatures, the updater manifest or the updater archive as a download
    if (/\.sig$/.test(name) || name === 'latest.json' || /\.app\.tar\.gz$/.test(name)) continue;
    for (const [key, re] of PATTERNS) {
      if (re.test(name) && !out[key]) out[key] = { url, size: typeof a.size === 'number' ? a.size : 0 };
    }
  }
  return out;
}

/** "21" for 21 121 640 bytes (decimal megabytes, rounded: matches the sizes in the design). */
export function mb(asset: Asset | undefined, fallback: number): number {
  return asset && asset.size > 0 ? Math.round(asset.size / 1e6) : fallback;
}

/** Download URL for an OS, falling back to the releases page. */
export function dlUrl(rel: ReleaseData, os: DlOs): string {
  return rel[os]?.url ?? rel.url ?? FALLBACK.url;
}

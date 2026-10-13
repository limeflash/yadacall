// Single place for the deployment address. Moving to a custom domain later = change these two lines
// (SITE to the domain, BASE to '').
export const SITE = 'https://limeflash.github.io';
export const BASE = '/yadacall';

export const REPO = 'https://github.com/limeflash/yadacall';
export const RELEASES_LATEST = `${REPO}/releases/latest`;
export const ISSUES = `${REPO}/issues`;
export const API_LATEST = 'https://api.github.com/repos/limeflash/yadacall/releases/latest';

/** Absolute path on this site, with the base prefix: href('/privacy/') -> '/yadacall/privacy/'. */
export function href(path: string): string {
  return `${BASE}${path}`;
}

/** Absolute URL on this site (for canonical, OG, sitemap). */
export function abs(path: string): string {
  return `${SITE}${BASE}${path}`;
}

// Download buttons: OS detection (the design's detectOs()) and a client-side refresh of the release data that was
// baked in at build time. Everything here is best-effort: any failure leaves the baked links in place.
import { API_LATEST } from '../lib/site';
import { FALLBACK_MB, dlUrl, mb, parseRelease, type DlOs, type ReleaseData } from '../lib/release';

export function detectOs(): DlOs {
  try {
    const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
    const p = ((nav.userAgentData && nav.userAgentData.platform) || navigator.platform || navigator.userAgent || '').toLowerCase();
    if (p.indexOf('win') >= 0) return 'win';
    if (p.indexOf('linux') >= 0 && p.indexOf('android') < 0) return 'linux';
  } catch (e) {
    /* fall through to the default */
  }
  return 'mac';
}

const KEY = 'yc-release-v1';
const TTL = 60 * 60 * 1000;

function readCache(): ReleaseData | null {
  try {
    const raw = sessionStorage.getItem(KEY);
    if (!raw) return null;
    const c = JSON.parse(raw) as { t: number; d: unknown };
    if (!c || typeof c.t !== 'number' || Date.now() - c.t > TTL) return null;
    return c.d as ReleaseData;
  } catch (e) {
    return null;
  }
}

function writeCache(d: ReleaseData): void {
  try {
    sessionStorage.setItem(KEY, JSON.stringify({ t: Date.now(), d }));
  } catch (e) {
    /* storage can be unavailable (private mode, blocked) */
  }
}

/** Resolves with fresh data, or null if nothing newer than the baked HTML could be obtained. */
export async function fetchRelease(): Promise<ReleaseData | null> {
  const cached = readCache();
  if (cached && cached.version) return cached;
  try {
    const res = await fetch(API_LATEST, { headers: { Accept: 'application/vnd.github+json' } });
    if (!res.ok) return null;
    const rel = parseRelease(await res.json());
    if (rel) writeCache(rel);
    return rel;
  } catch (e) {
    return null;
  }
}

export interface Ctx {
  os: DlOs;
  /** "МБ" / "MB" */
  unit: string;
  /** called with the macOS installer size in MB so the count-up stat can follow it */
  onMacMb: (n: number) => void;
}

/** Point every link, size and the version pill at `rel`. */
export function applyRelease(root: ParentNode, rel: ReleaseData, ctx: Ctx): void {
  root.querySelectorAll<HTMLElement>('[data-rel-version]').forEach((e) => (e.textContent = rel.version));
  root.querySelectorAll<HTMLAnchorElement>('[data-rel-link]').forEach((e) => (e.href = rel.url));
  (['mac', 'win', 'linux'] as DlOs[]).forEach((id) => {
    root.querySelectorAll<HTMLAnchorElement>(`[data-dl-btn="${id}"]`).forEach((e) => (e.href = dlUrl(rel, id)));
  });
  root.querySelectorAll<HTMLAnchorElement>('[data-dl-deb]').forEach((e) => (e.href = rel.deb?.url ?? rel.url));
  const macMb = mb(rel.mac, FALLBACK_MB.mac);
  const winMb = mb(rel.win, FALLBACK_MB.win);
  root.querySelectorAll<HTMLElement>('[data-chip-size="mac"]').forEach((e) => (e.textContent = `${macMb} ${ctx.unit}`));
  root.querySelectorAll<HTMLElement>('[data-chip-size="win"]').forEach((e) => (e.textContent = `${winMb} ${ctx.unit}`));
  root.querySelectorAll<HTMLElement>('[data-size-mac]').forEach((e) => (e.textContent = `${macMb} ${ctx.unit}`));
  ctx.onMacMb(macMb);
  applyOs(root, rel, ctx);
}

/** Hero button + the highlighted "your system" card. */
export function applyOs(root: ParentNode, rel: ReleaseData, ctx: Ctx, ctaFor?: string): void {
  const cta = root.querySelector<HTMLAnchorElement>('#yl-hero-cta');
  if (cta) {
    cta.href = dlUrl(rel, ctx.os);
    if (ctaFor !== undefined) {
      const label = cta.querySelector('[data-cta-text]');
      if (label) label.textContent = ctaFor + { mac: 'macOS', win: 'Windows', linux: 'Linux' }[ctx.os];
    }
  }
  root.querySelectorAll<HTMLElement>('[data-dl-card]').forEach((card) => {
    const mine = card.dataset.dlCard === ctx.os;
    card.style.borderColor = mine ? 'var(--accent-line)' : 'var(--border)';
    const glow = card.querySelector<HTMLElement>('[data-mine-glow]');
    if (glow) glow.style.display = mine ? 'block' : 'none';
    const chip = card.querySelector<HTMLElement>('[data-mine-chip]');
    if (chip) chip.style.display = mine ? 'inline-flex' : 'none';
    const tile = card.querySelector<HTMLElement>('[data-tile]');
    if (tile) {
      tile.style.background = mine ? 'var(--accent)' : 'var(--surface-2)';
      tile.style.color = mine ? 'var(--ink-on-accent)' : 'var(--text)';
    }
    const btn = card.querySelector<HTMLElement>('[data-dl-btn]');
    if (btn) {
      btn.style.background = mine ? 'var(--accent)' : 'var(--surface-2)';
      btn.style.color = mine ? 'var(--ink-on-accent)' : 'var(--text)';
    }
  });
}

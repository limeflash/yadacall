// The pure part of the design's `renderVals()`: what the hero chips, the caption and the demo transcript look like
// for a given (line, word) state. Used both at build time (first paint) and in the browser (landing.ts), so the two
// can never drift apart.
import { LINES, type Lang, type Line } from './i18n';

export function eqs(on: boolean, base: string): string[] {
  const d = ['0s', '.15s', '.3s', '.45s'];
  return d.map((x) => (on ? 'ycEq ' + base + ' ease-in-out ' + x + ' infinite' : 'none'));
}

export interface HeroVals {
  line: Line;
  speaking: boolean;
  eqMe: string[];
  eqThem: string[];
  meLine: string;
  themLine: string;
  meGlow: string;
  themGlow: string;
  cap: { name: string; nameColor: string; text: string; caret: string };
}

export function heroVals(lang: Lang, li: number, w: number, vw = 1200): HeroVals {
  const L = LINES[lang];
  const line = L[li];
  const words = line.text.split(' ');
  const speaking = w < words.length;
  const capMax = vw < 520 ? 30 : vw < 760 ? 44 : 62;
  let capText = words.slice(0, Math.max(1, w)).join(' ');
  if (capText.length > capMax) {
    let cut = capText.slice(capText.length - capMax);
    const sp = cut.indexOf(' ');
    if (sp >= 0 && sp < 14) cut = cut.slice(sp + 1);
    capText = '…' + cut;
  }
  return {
    line,
    speaking,
    eqMe: eqs(speaking && line.me, '.62s'),
    eqThem: eqs(speaking && !line.me, '.7s'),
    meLine: speaking && line.me ? 'var(--accent-line)' : 'var(--border)',
    themLine: speaking && !line.me ? 'color-mix(in srgb, var(--them) 45%, transparent)' : 'var(--border)',
    meGlow:
      speaking && line.me
        ? '0 0 0 4px color-mix(in srgb, var(--accent) 12%, transparent), 0 10px 30px -10px color-mix(in srgb, var(--accent) 60%, transparent)'
        : '0 8px 24px -14px rgba(0, 0, 0, 0.6)',
    themGlow:
      speaking && !line.me
        ? '0 0 0 4px color-mix(in srgb, var(--them) 10%, transparent), 0 10px 30px -10px color-mix(in srgb, var(--them) 45%, transparent)'
        : '0 8px 24px -14px rgba(0, 0, 0, 0.6)',
    cap: {
      name: line.name,
      nameColor: line.me ? 'var(--accent-text)' : 'var(--them)',
      text: capText,
      caret: speaking ? 'yc-caret' : '',
    },
  };
}

export interface DemoItem {
  name: string;
  text: string;
  caret: string;
  justify: string;
  bg: string;
  radius: string;
  nameColor: string;
}

export function demoVals(lang: Lang, li: number, w: number): DemoItem[] {
  const L = LINES[lang];
  const demo: DemoItem[] = [];
  for (let i = Math.max(0, li - 3); i <= li; i++) {
    const l = L[i];
    const cur = i === li;
    const ws = l.text.split(' ');
    demo.push({
      name: l.name,
      text: cur ? ws.slice(0, Math.max(1, w)).join(' ') : l.text,
      caret: cur && w < ws.length ? 'yc-caret' : '',
      justify: l.me ? 'flex-end' : 'flex-start',
      bg: l.me ? 'var(--accent-soft)' : 'var(--surface-2)',
      radius: l.me ? '16px 16px 6px 16px' : '16px 16px 16px 6px',
      nameColor: l.me ? 'var(--accent-text)' : 'var(--muted)',
    });
  }
  return demo;
}

export function two(n: number): string {
  return (n < 10 ? '0' : '') + n;
}

/** Recording timer text of the demo card header. */
export function clock(sec: number): string {
  return Math.floor(sec / 60) + ':' + two(sec % 60);
}

/** h1 words with the design's staggered animation delays. */
export function h1Words(h1: [string, number][]): { a: { t: string; d: string }[]; b: { t: string; d: string }[] } {
  let di = 0;
  let si = 0;
  const a: { t: string; d: string }[] = [];
  const b: { t: string; d: string }[] = [];
  h1.forEach((w) => {
    const d = (0.18 + di++ * 0.07).toFixed(2) + 's';
    if (w[1]) b.push({ t: w[0], d: d + ', ' + (2.2 + si++ * 0.13).toFixed(2) + 's' });
    else a.push({ t: w[0], d });
  });
  return { a, b };
}

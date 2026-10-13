// Landing motion, ported 1:1 from the design's `class Component extends DCLogic` (Landing.dc.html).
// Same state machine (li, w, hold, sec, cnt), same constants, same timings, same easing. React re-rendering is
// replaced by direct DOM updates. Additions that change nothing visually: the rAF loop and the intervals are
// paused while the hero is off-screen or the tab is hidden.
import { clock, demoVals, eqs, heroVals, type DemoItem } from '../lib/hero-state';
import type { Line } from '../lib/i18n';
import type { DlOs, ReleaseData } from '../lib/release';
import { applyOs, applyRelease, detectOs, fetchRelease, type Ctx } from './downloads';

interface Payload {
  lang: 'ru' | 'en';
  lines: Line[];
  mb: string;
  ctaFor: string;
  btn: string;
  stat: { mac: number; cpu: number; langs: number };
  rel: ReleaseData;
}

interface Particle { x: number; y: number; vx: number; vy: number; life: number; ch: string; me: boolean; rot: number }
interface Ripple { r: number; me: boolean }
interface Colors { a: string; th: string; tx: string }

const root = document.getElementById('yl-root') as HTMLElement;
const data = JSON.parse(document.getElementById('yl-data')!.textContent || '{}') as Payload;
const L = data.lines;
const lang = data.lang;

const orbEl = document.getElementById('yl-orb') as HTMLCanvasElement;
const bgEl = document.getElementById('yl-bg') as HTMLCanvasElement;
const winEl = document.getElementById('yl-win') as HTMLElement;
const demoEl = document.getElementById('yl-demo') as HTMLElement;

// ---- state (design: state = { li, w, hold, sec, os, solid, cnt }; `solid` lives in nav.ts) ----
const state = { li: 0, w: 0, hold: 0, sec: 772, os: 'mac' as DlOs, cnt: 0 };

const reduce = !!(window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches);
const lv = new Float32Array(72);
const parts: Particle[] = [];
const ripples: Ripple[] = [];
let lastRip = 0;
let colN = 0;
let col: Colors | null = null;
let macMb = data.stat.mac;
let rel: ReleaseData = data.rel;

// ======================================================================================================
// DOM updates (what renderVals() + React did on every setState)
// ======================================================================================================
const chipMe = document.getElementById('yl-chip-me') as HTMLElement;
const chipThem = document.getElementById('yl-chip-them') as HTMLElement;
const capName = document.getElementById('yl-cap-name') as HTMLElement;
const capText = document.getElementById('yl-cap-text') as HTMLElement;
const demoList = document.getElementById('yl-demo-list') as HTMLElement;
const demoTime = document.getElementById('yl-demo-time') as HTMLElement;
const eqMe = Array.from(document.querySelectorAll<HTMLElement>('[data-eq="me"] > span'));
const eqThem = Array.from(document.querySelectorAll<HTMLElement>('[data-eq="them"] > span'));
const eqCache = new WeakMap<HTMLElement, string>();

function setEq(spans: HTMLElement[], anims: string[]): void {
  // each group (hero chip, demo footer) has 4 bars; spans come in document order
  spans.forEach((s, i) => {
    const a = anims[i % 4];
    if (eqCache.get(s) === a) return;
    eqCache.set(s, a);
    s.style.animation = a;
  });
}

function setStyle(el: HTMLElement, prop: string, v: string): void {
  if (el.style.getPropertyValue(prop) !== v) el.style.setProperty(prop, v);
}

function renderHero(): void {
  const h = heroVals(lang, state.li, state.w, window.innerWidth || 1200);
  setStyle(chipMe, 'border-color', h.meLine);
  setStyle(chipMe, 'box-shadow', h.meGlow);
  setStyle(chipThem, 'border-color', h.themLine);
  setStyle(chipThem, 'box-shadow', h.themGlow);
  setEq(eqMe, eqs(h.speaking && h.line.me, '.62s'));
  setEq(eqThem, eqs(h.speaking && !h.line.me, '.7s'));
  if (capName.textContent !== h.cap.name) capName.textContent = h.cap.name;
  setStyle(capName, 'color', h.cap.nameColor);
  if (capText.textContent !== h.cap.text) capText.textContent = h.cap.text;
  if (capText.className !== h.cap.caret) capText.className = h.cap.caret;
}

// Demo transcript: React keyed the bubbles by index, so a bubble animates in (yl-up) only when its slot is new;
// slots that already exist just get new content. The same here.
interface Bubble { row: HTMLElement; bub: HTMLElement; name: HTMLElement; text: HTMLElement }
const bubbles: Bubble[] = [];

function adoptFirstBubble(): void {
  const row = demoList.firstElementChild as HTMLElement | null;
  if (!row) return;
  const bub = row.firstElementChild as HTMLElement;
  bubbles.push({ row, bub, name: bub.children[0] as HTMLElement, text: bub.children[1] as HTMLElement });
}

function makeBubble(): Bubble {
  const row = document.createElement('div');
  row.style.cssText = 'display: flex';
  const bub = document.createElement('div');
  bub.className = 'yl-up';
  bub.style.cssText = 'max-width: 84%; display: flex; flex-direction: column; gap: 3px; padding: 10px 14px; animation-duration: .45s';
  const name = document.createElement('span');
  name.style.cssText = 'font-size: 11.5px; font-weight: 700';
  const text = document.createElement('span');
  text.style.cssText = 'font-size: 14.5px; line-height: 1.5';
  bub.append(name, text);
  row.append(bub);
  demoList.append(row);
  return { row, bub, name, text };
}

function fillBubble(b: Bubble, d: DemoItem): void {
  setStyle(b.row, 'justify-content', d.justify);
  setStyle(b.bub, 'border-radius', d.radius);
  setStyle(b.bub, 'background', d.bg);
  if (b.name.textContent !== d.name) b.name.textContent = d.name;
  setStyle(b.name, 'color', d.nameColor);
  if (b.text.textContent !== d.text) b.text.textContent = d.text;
  if (b.text.className !== d.caret) b.text.className = d.caret;
}

function renderDemo(): void {
  const items = demoVals(lang, state.li, state.w);
  while (bubbles.length < items.length) bubbles.push(makeBubble());
  while (bubbles.length > items.length) bubbles.pop()!.row.remove();
  items.forEach((d, i) => fillBubble(bubbles[i], d));
}

function renderClock(): void {
  const s = clock(state.sec);
  if (demoTime.textContent !== s) demoTime.textContent = s;
}

function render(): void {
  renderHero();
  renderDemo();
}

// ---- stats count-up (design: cnt) ----
const statEls = Array.from(document.querySelectorAll<HTMLElement>('[data-stat]'));
function renderStats(): void {
  const c = state.cnt;
  const vals = [
    Math.round(macMb * c) + ' ' + data.mb,
    Math.round(data.stat.cpu * c) + ' ' + data.mb,
    String(Math.round(data.stat.langs * c)),
    '0',
  ];
  statEls.forEach((el, i) => {
    if (el.textContent !== vals[i]) el.textContent = vals[i];
  });
}

// ======================================================================================================
// Typing loop (design: tick(), every 105 ms)
// ======================================================================================================
function tick(): void {
  const s = state;
  const words = L[s.li].text.split(' ');
  if (s.w < words.length) {
    s.w += 1;
  } else {
    const pause = s.li === L.length - 1 ? 28 : 9;
    if (s.hold < pause) s.hold += 1;
    else {
      s.li = (s.li + 1) % L.length;
      s.w = 0;
      s.hold = 0;
    }
  }
  render();
}

// ======================================================================================================
// Window tilt on scroll (design: tilt())
// ======================================================================================================
function tilt(): void {
  const el = winEl;
  if (reduce) {
    el.style.transform = 'none';
    return;
  }
  const r = el.getBoundingClientRect();
  const vh = window.innerHeight || 800;
  const p = Math.max(0, Math.min(1, 1 - (r.top - vh * 0.12) / (vh * 0.7)));
  const e = 1 - Math.pow(1 - p, 3);
  el.style.transform =
    'perspective(1600px) rotateX(' + ((1 - e) * 24).toFixed(2) + 'deg) scale(' + (0.9 + 0.1 * e).toFixed(4) + ') translateY(' + ((1 - e) * 30).toFixed(1) + 'px)';
  el.style.opacity = (0.55 + 0.45 * e).toFixed(3);
}

// ======================================================================================================
// Scroll reveal + count-up (design: observe(), countUp())
// ======================================================================================================
let io: IntersectionObserver | null = null;
let cntGo = false;

function countUp(): void {
  if (cntGo) return;
  cntGo = true;
  const t0 = performance.now();
  const step = (t: number): void => {
    const p = Math.min(1, (t - t0) / 1500);
    state.cnt = 1 - Math.pow(1 - p, 3);
    renderStats();
    if (p < 1) requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
}

function observe(): void {
  root.classList.add('yl-js');
  const els = root.querySelectorAll<HTMLElement>('.yl-rv:not(.yl-on)');
  if (reduce || typeof IntersectionObserver === 'undefined') {
    els.forEach((e) => e.classList.add('yl-on'));
    countUp();
    return;
  }
  io = new IntersectionObserver(
    (ents) => {
      ents.forEach((en) => {
        if (!en.isIntersecting) return;
        const target = en.target as HTMLElement;
        target.classList.add('yl-on');
        io!.unobserve(target);
        if (target.hasAttribute('data-count')) countUp();
      });
    },
    { rootMargin: '0px 0px -6% 0px', threshold: 0.08 },
  );
  els.forEach((e) => io!.observe(e));
}

// ======================================================================================================
// Canvas frame (design: colors() + frame(t))
// ======================================================================================================
function colors(): Colors {
  if (!col || (colN = (colN || 0) + 1) % 90 === 0) {
    const cs = getComputedStyle(root || document.documentElement);
    col = {
      a: cs.getPropertyValue('--accent').trim() || '#7dd3a0',
      th: cs.getPropertyValue('--them').trim() || '#d4d4dc',
      tx: cs.getPropertyValue('--text').trim() || '#f3f3f5',
    };
  }
  return col;
}

function frame(t: number): void {
  const orb = orbEl;
  const bg = bgEl;
  const vh = window.innerHeight || 800;
  const ro = orb.getBoundingClientRect();
  if (ro.bottom < -200 || ro.top > vh + 200) return;
  const C = colors();
  const time = t / 1000;
  const s = state;
  const line = L[s.li];
  const words = line.text.split(' ');
  const speaking = s.w < words.length && !reduce;
  const dpr = Math.min(2, window.devicePixelRatio || 1);

  const ow = ro.width;
  const oh = ro.height;
  const size = oh;
  if (ow > 0 && oh > 0) {
    const W = Math.round(ow * dpr);
    const H = Math.round(oh * dpr);
    if (orb.width !== W || orb.height !== H) {
      orb.width = W;
      orb.height = H;
    }
    const ctx = orb.getContext('2d')!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, ow, oh);
    const cx = ow / 2;
    const cy = oh * 0.44;
    const R0 = size * 0.25;
    const Lm = size * 0.15;
    const N = 72;
    ctx.globalAlpha = 0.16;
    ctx.fillStyle = C.a;
    ctx.beginPath();
    ctx.arc(cx, cy, R0 - 2, 0, Math.PI * 2);
    ctx.fill();
    ctx.globalAlpha = 0.55;
    ctx.strokeStyle = C.a;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.arc(cx, cy, R0, 0, Math.PI * 2);
    ctx.stroke();
    const spin = reduce ? 0 : time * 0.12;
    ctx.globalAlpha = 0.28;
    ctx.lineWidth = 1;
    ctx.setLineDash([2, 9]);
    ctx.beginPath();
    ctx.arc(cx, cy, R0 + Lm + 26, spin, spin + Math.PI * 2);
    ctx.stroke();
    ctx.globalAlpha = 0.12;
    ctx.setLineDash([1, 5]);
    ctx.beginPath();
    ctx.arc(cx, cy, R0 + Lm + 46, -spin * 0.7, -spin * 0.7 + Math.PI * 2);
    ctx.stroke();
    ctx.setLineDash([]);
    ctx.lineCap = 'round';
    ctx.lineWidth = Math.max(2, size * 0.012);
    for (let i = 0; i < N; i++) {
      const a = (i / N) * Math.PI * 2 - Math.PI / 2;
      const left = Math.cos(a) < 0;
      const active = speaking && (line.me ? left : !left);
      const wob = Math.abs(Math.sin(i * 1.93 + time * 7.3) * Math.cos(i * 0.71 - time * 4.1));
      const target = active ? 0.3 + 0.7 * wob : 0.05 + 0.06 * Math.abs(Math.sin(i * 0.9 + time * 1.7));
      lv[i] += (target - lv[i]) * 0.2;
      const len = 3 + Lm * lv[i];
      const r1 = R0 + 10;
      const r2 = r1 + len;
      ctx.globalAlpha = left ? 0.95 : 0.8;
      ctx.strokeStyle = left ? C.a : C.th;
      ctx.beginPath();
      ctx.moveTo(cx + Math.cos(a) * r1, cy + Math.sin(a) * r1);
      ctx.lineTo(cx + Math.cos(a) * r2, cy + Math.sin(a) * r2);
      ctx.stroke();
    }
    if (speaking && Math.random() < 0.55 && parts.length < 90) {
      const chars = line.text.replace(/[\s—,.?!«»“”’]/g, '');
      const ch = chars.charAt(Math.floor(Math.random() * chars.length)) || '·';
      const a = (line.me ? Math.PI : 0) + (Math.random() - 0.5) * 1.3;
      const r = R0 + 12 + Lm * 0.6;
      const sp = size * (0.003 + Math.random() * 0.0045);
      parts.push({ x: cx + Math.cos(a) * r, y: cy + Math.sin(a) * r, vx: Math.cos(a) * sp, vy: Math.sin(a) * sp - size * 0.0004, life: 1, ch, me: line.me, rot: (Math.random() - 0.5) * 0.6 });
    }
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.font = '700 ' + Math.round(size * 0.054) + 'px "JetBrains Mono", ui-monospace, monospace';
    for (let k = parts.length - 1; k >= 0; k--) {
      const p = parts[k];
      p.x += p.vx;
      p.y += p.vy;
      p.vy += size * 0.00003;
      p.life -= 0.0095;
      if (p.life <= 0) {
        parts.splice(k, 1);
        continue;
      }
      ctx.save();
      ctx.translate(p.x, p.y);
      ctx.rotate(p.rot * (1 - p.life));
      ctx.globalAlpha = Math.min(1, p.life * 1.4) * 0.9;
      ctx.fillStyle = p.me ? C.a : C.th;
      ctx.fillText(p.ch, 0, 0);
      ctx.restore();
    }
    ctx.globalAlpha = 1;
  }

  if (bg) {
    const rb = bg.getBoundingClientRect();
    const bw = rb.width;
    const bh = rb.height;
    if (bw > 0 && bh > 0) {
      const BW = Math.round(bw * dpr);
      const BH = Math.round(bh * dpr);
      if (bg.width !== BW || bg.height !== BH) {
        bg.width = BW;
        bg.height = BH;
      }
      const g = bg.getContext('2d')!;
      g.setTransform(dpr, 0, 0, dpr, 0, 0);
      g.clearRect(0, 0, bw, bh);
      const ox = ro.left - rb.left + ro.width / 2;
      const oy = ro.top - rb.top + ro.height * 0.44;
      if (speaking && (!lastRip || t - lastRip > 900)) {
        lastRip = t;
        ripples.push({ r: ro.height * 0.42, me: line.me });
      }
      for (let k = ripples.length - 1; k >= 0; k--) {
        ripples[k].r += 2.4;
        if (ripples[k].r > Math.max(bw, bh)) ripples.splice(k, 1);
      }
      const G = 22;
      const maxR = Math.max(bw, bh);
      for (let y = G / 2; y < bh; y += G) {
        for (let x = G / 2; x < bw; x += G) {
          const dx = x - ox;
          const dy = y - oy;
          const d = Math.sqrt(dx * dx + dy * dy);
          let boost = 0;
          let me = true;
          for (let k = 0; k < ripples.length; k++) {
            const rr = ripples[k];
            const q = Math.abs(d - rr.r);
            if (q < 26) {
              const b = (1 - q / 26) * (1 - rr.r / maxR);
              if (b > boost) {
                boost = b;
                me = rr.me;
              }
            }
          }
          g.globalAlpha = 0.09 + boost * 0.55;
          g.fillStyle = boost > 0.02 ? (me ? C.a : C.th) : C.tx;
          g.fillRect(x - 0.9, y - 0.9, 1.8 + boost * 1.2, 1.8 + boost * 1.2);
        }
      }
      g.globalAlpha = 1;
    }
  }
}

// ======================================================================================================
// Run loop: rAF + intervals, paused while nothing that depends on them is on screen
// ======================================================================================================
let raf = 0;
let ivTick = 0;
let ivClock = 0;
let heroVis = true; // the design starts everything at mount
let demoVis = false;
let clockPausedAt = 0;

function loop(t: number): void {
  raf = requestAnimationFrame(loop);
  frame(t);
}

function sync(): void {
  const visible = !document.hidden;
  const wantRaf = visible && heroVis;
  const wantTick = visible && (heroVis || demoVis);
  const wantClock = visible && demoVis;
  if (wantRaf && !raf) raf = requestAnimationFrame(loop);
  if (!wantRaf && raf) {
    cancelAnimationFrame(raf);
    raf = 0;
  }
  if (wantTick && !ivTick) ivTick = window.setInterval(tick, 105);
  if (!wantTick && ivTick) {
    clearInterval(ivTick);
    ivTick = 0;
  }
  if (wantClock && !ivClock) {
    if (clockPausedAt) state.sec += Math.floor((performance.now() - clockPausedAt) / 1000);
    clockPausedAt = 0;
    renderClock();
    ivClock = window.setInterval(() => {
      state.sec += 1;
      renderClock();
    }, 1000);
  }
  if (!wantClock && ivClock) {
    clearInterval(ivClock);
    ivClock = 0;
    clockPausedAt = performance.now();
  }
  if (!wantClock && !clockPausedAt && !ivClock) clockPausedAt = performance.now();
}

// ======================================================================================================
// Boot (design: componentDidMount)
// ======================================================================================================
function boot(): void {
  adoptFirstBubble();
  render();
  renderClock();
  renderStats(); // cnt = 0, as in the design's initial state

  const ctx: Ctx = {
    os: 'mac',
    unit: data.mb,
    onMacMb: (n) => {
      macMb = n;
      if (state.cnt > 0) renderStats();
    },
  };
  state.os = detectOs();
  ctx.os = state.os;
  applyOs(document, rel, ctx, data.ctaFor);

  // spotlight on cards (design: spot())
  document.querySelectorAll<HTMLElement>('.yl-spot').forEach((el) => {
    el.addEventListener('mousemove', (e) => {
      const r = el.getBoundingClientRect();
      el.style.setProperty('--mx', e.clientX - r.left + 'px');
      el.style.setProperty('--my', e.clientY - r.top + 'px');
    });
  });

  const onScroll = (): void => tilt();
  window.addEventListener('scroll', onScroll, { passive: true });
  window.addEventListener('resize', onScroll);
  tilt();

  if (typeof IntersectionObserver !== 'undefined') {
    // same +/-200 px margin the design's frame() used for "orb on screen"
    new IntersectionObserver(
      (es) => {
        heroVis = es[es.length - 1].isIntersecting;
        sync();
      },
      { rootMargin: '200px 0px' },
    ).observe(orbEl);
    new IntersectionObserver((es) => {
      demoVis = es[es.length - 1].isIntersecting;
      sync();
    }).observe(demoEl);
  }
  document.addEventListener('visibilitychange', sync);
  sync();

  setTimeout(observe, 50);

  // fresh release data (cached for an hour in sessionStorage); failures keep the baked links
  void fetchRelease().then((fresh) => {
    if (!fresh) return;
    rel = fresh;
    applyRelease(document, fresh, ctx);
  });
}

boot();

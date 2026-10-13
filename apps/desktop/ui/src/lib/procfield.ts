/**
 * The processing screen's glyph field: three parallax layers of Cyrillic + Latin glyphs (letters of the meeting itself),
 * waves that travel out of the progress ring, and words from the conversation that the field "finds" — a wave arrives,
 * letters flicker through candidates and settle one by one (ease-out-back), bloom in the accent, then are blown away.
 * Everything is drawn from a glyph atlas (drawImage), at most 30 fps, and nothing here runs while the window is hidden.
 */

export interface WordSource {
  next(maxLen: number): string | null;
  fallback: () => string[];
  /** Letters of the meeting, most frequent first (the field is made of them). */
  alphabet?: () => string;
  /** Reduced motion: the word to show as plain text instead of in the field. */
  show?: (w: string | null) => void;
}

export interface FieldColors { acc: number[]; ink: number[]; dark: boolean }

const LETTERS = "абвгдежзиклмнопрстуфхцчшэюяabcdefghijklmnoprstuvwxyz";
const css = (c: number[], a = 1) => `rgba(${c[0] | 0},${c[1] | 0},${c[2] | 0},${a})`;
const mix = (a: number[], b: number[], k: number) => a.map((v, i) => v + (b[i] - v) * k);
const hash = (x: number, y: number, s: number) => (Math.imul(x, 73856093) ^ Math.imul(y, 19349663) ^ Math.imul(s, 83492791)) >>> 0;
const easeOutBack = (u: number) => { const c1 = 1.70158, c3 = c1 + 1, v = u - 1; return 1 + c3 * v * v * v + c1 * v * v; };
const smooth = (a: number, b: number, x: number) => { const u = Math.min(1, Math.max(0, (x - a) / (b - a))); return u * u * (3 - 2 * u); };

interface Layer { cell: number; size: number; alpha: number; drift: number; sway: number; rate: number; density: number; seed: number }
/** far dust, the main field, and a sparse near layer that drifts faster (parallax). */
const LAYERS: Layer[] = [
  { cell: 24, size: 8, alpha: 0.085, drift: 4, sway: 2, rate: 0.25, density: 1, seed: 11 },
  { cell: 16, size: 10, alpha: 0.13, drift: 8, sway: 3, rate: 0.6, density: 1, seed: 23 },
  { cell: 34, size: 14, alpha: 0.2, drift: 17, sway: 6, rate: 0.9, density: 0.3, seed: 37 },
];

interface Atlas { img: HTMLCanvasElement; slotW: number; slotH: number; dpr: number }
interface Wave { t0: number; amp: number; speed: number; /** A ping aimed at a word: only this sector of the ring lights up. */ ang?: number }
interface Spark { x: number; y: number; vx: number; vy: number; t0: number; life: number; size: number }
interface Word {
  chars: string[]; x: number[]; y: number; d: number[]; rank: number[];
  t0: number; lock: number[]; leave: number[]; holdEnd: number; end: number; wind: number; cxw: number; seed: number;
}

const ADV = 24;            // letter advance of a word, px
const WORD_PX = 28;        // word glyph size
const WAVE_SPEED = 0.43;   // px per ms
const RING_R = 84;

export class ProcField {
  private atlas = new Map<string, Atlas>();
  private waves: Wave[] = [];
  private sparks: Spark[] = [];
  private word: Word | null = null;
  private nextAt = 0;
  private side = 1;
  private lastRow = 99;
  private fb = 0;
  private plainAt = 0;
  private bias: number[] = [];
  private biasFor = "";
  /** 0..1: the ring should flash (a wave just left it); decays on its own. */
  kick = 0;
  reduced = false;

  constructor(private src?: WordSource) {}

  /** A stage changed: a stronger wave leaves the ring together with a burst of sparks. */
  stageBeat(now: number, cx: number, cy: number) {
    this.waves.push({ t0: now, amp: 1, speed: WAVE_SPEED * 1.25 });
    this.kick = 1;
    if (this.reduced) return;
    for (let i = 0; i < 22; i++) {
      const a = (i / 22) * Math.PI * 2 + Math.random() * 0.3, v = 0.1 + Math.random() * 0.2;
      this.sparks.push({ x: cx + Math.cos(a) * RING_R, y: cy + Math.sin(a) * RING_R, vx: Math.cos(a) * v, vy: Math.sin(a) * v, t0: now, life: 700 + Math.random() * 500, size: 1.2 + Math.random() * 2 });
    }
  }

  /** Leaves the processing look: forget the running word and waves. */
  stop() {
    if (this.word || this.waves.length) { this.word = null; this.waves = []; this.sparks = []; }
    if (this.plainAt) { this.plainAt = 0; this.src?.show?.(null); }
    this.nextAt = 0;
  }

  private atlasFor(size: number, dark: boolean, dpr: number, ink: number[], acc: number[]): Atlas {
    const fontsReady = typeof document !== "undefined" && document.fonts?.status === "loaded";
    const key = `${size}|${dark}|${dpr}|${fontsReady}|${ink.join()}|${acc.join()}`;
    let a = this.atlas.get(key);
    if (a) return a;
    if (this.atlas.size > 6) this.atlas.clear();
    const slotW = Math.ceil(size * 0.8) + 4, slotH = size + 6;
    const img = document.createElement("canvas");
    img.width = slotW * LETTERS.length * dpr; img.height = slotH * 2 * dpr;
    const g = img.getContext("2d")!;
    g.scale(dpr, dpr);
    g.font = `600 ${size}px 'JetBrains Mono', monospace`; g.textAlign = "center"; g.textBaseline = "middle";
    [css(ink), css(acc)].forEach((col, row) => {
      g.fillStyle = col;
      [...LETTERS].forEach((ch, i) => g.fillText(ch, i * slotW + slotW / 2, row * slotH + slotH / 2 + 0.5));
    });
    a = { img, slotW, slotH, dpr };
    this.atlas.set(key, a);
    return a;
  }

  private rebias(alphabet: string) {
    if (alphabet === this.biasFor) return;
    this.biasFor = alphabet;
    this.bias = [...alphabet.toLowerCase()].map((ch) => LETTERS.indexOf(ch)).filter((i) => i >= 0);
  }
  private pick(h: number): number {
    if (this.bias.length >= 8 && ((h >>> 20) & 3) !== 0) return this.bias[(h >>> 4) % this.bias.length];
    return h % LETTERS.length;
  }

  // ---- words ------------------------------------------------------------------------------------------------

  private plan(now: number, W: number, cx: number, cy: number, active: boolean) {
    const src = this.src;
    if (!src) return;
    if (!active) { this.stop(); return; }
    if (this.reduced) {
      if (now >= this.plainAt) { this.plainAt = now + 3200; src.show?.(src.next(14) ?? this.fallbackWord(14)); }
      return;
    }
    if (this.word && now > this.word.end) this.word = null;
    if (this.word || now < this.nextAt) return;
    const zone = (s: number) => (s === 1 ? W - 56 - (cx + 124) : cx - 124 - 56);
    const room = Math.min(13, Math.floor(Math.max(zone(1), zone(-1)) / ADV));
    if (room < 3) { this.nextAt = now + 1000; return; }
    const text = src.next(room) ?? this.fallbackWord(room);
    const chars = [...text];
    this.side = -this.side;
    if (zone(this.side) < chars.length * ADV) this.side = -this.side;
    if (zone(this.side) < chars.length * ADV) { this.nextAt = now + 400; return; }
    const rows = [-56, -28, 0, 28, 56].filter((r) => r !== this.lastRow);
    const row = rows[Math.floor(Math.random() * rows.length)];
    this.lastRow = row;
    const y = cy + row;
    const z0 = this.side === 1 ? cx + 124 : 56, zw = zone(this.side);
    const startX = z0 + (zw - chars.length * ADV) / 2 + ADV / 2;
    const x = chars.map((_, i) => startX + i * ADV);
    const d = x.map((xx) => Math.hypot(xx - cx, y - cy));
    const order = d.map((v, i) => [v, i]).sort((a, b) => a[0] - b[0]);
    const rank = new Array<number>(chars.length);
    order.forEach(([, i], r) => { rank[i] = r; });
    const lock = chars.map((_, i) => Math.max(0, (d[i] - RING_R) / WAVE_SPEED) + 90 + rank[i] * 48);
    const holdEnd = Math.max(...lock) + 1150;
    const leave = chars.map((_, i) => holdEnd + rank[i] * 42);
    const end = Math.max(...leave) + 800;
    this.word = { chars, x, y, d, rank, t0: now, lock, leave, holdEnd, end: now + end, wind: this.side, cxw: startX + ((chars.length - 1) * ADV) / 2, seed: (Math.random() * 1e6) | 0 };
    this.waves.push({ t0: now, amp: 0.9, speed: WAVE_SPEED, ang: Math.atan2(y - cy, this.word.cxw - cx) });
    this.kick = Math.max(this.kick, 0.8);
    this.nextAt = now + Math.max(2600, end + 150);
  }
  private fallbackWord(max: number): string {
    const list = (this.src?.fallback() ?? []).filter((w) => w.length <= max);
    return list.length ? list[this.fb++ % list.length] : "…";
  }

  // ---- drawing ----------------------------------------------------------------------------------------------

  draw(ctx: CanvasRenderingContext2D, W: number, H: number, cx: number, cy: number, now: number, dpr: number, colors: FieldColors, alpha: number, active: boolean) {
    if (alpha < 0.01) return;
    const { acc, ink, dark } = colors;
    this.kick *= 0.94;
    this.plan(now, W, cx, cy, active);
    this.waves = this.waves.filter((w) => (now - w.t0) * w.speed < 620);
    this.sparks = this.sparks.filter((s) => now - s.t0 < s.life);
    if (this.src?.alphabet) this.rebias(this.src.alphabet());
    const t = this.reduced ? 0 : now / 1000;
    const wd = this.word;
    const el = wd ? now - wd.t0 : 0;
    // How strongly the word glows right now: rises with the first locks, breathes while held, fades as it is blown away.
    let bloom = 0;
    if (wd) {
      const firstLock = Math.min(...wd.lock), lastLock = Math.max(...wd.lock);
      bloom = smooth(firstLock, lastLock + 200, el) * (1 - smooth(wd.holdEnd, wd.holdEnd + 650, el)) * (0.9 + 0.1 * Math.sin(el / 260));
    }
    const presence = wd ? smooth(0, 350, el) * (1 - smooth(wd.end - wd.t0 - 600, wd.end - wd.t0, el)) : 0;
    const wideAcc = dark ? mix(acc, [236, 236, 242], 0.22) : mix(acc, [0, 0, 0], 0.45);
    ctx.save();
    ctx.globalAlpha = alpha;

    // layers of glyphs
    for (const L of LAYERS) {
      const at = this.atlasFor(L.size, dark, dpr, ink, acc);
      const off = t * L.drift, rows0 = Math.floor(off / L.cell), fy = off - rows0 * L.cell;
      const sx = Math.sin(t * 0.3 + L.seed) * L.sway;
      for (let iy = -1; iy * L.cell - fy < H + L.cell; iy++) {
        const y = iy * L.cell + L.cell / 2 - fy;
        for (let ix = -1; ix * L.cell < W + L.cell; ix++) {
          const x = ix * L.cell + L.cell / 2 + sx;
          const wy = iy + rows0;
          const h0 = hash(ix, wy, L.seed);
          if (L.density < 1 && (h0 & 255) / 255 > L.density) continue;
          const dx = x - cx, dy = y - cy, d2 = dx * dx + dy * dy;
          if (d2 < 90 * 90) continue;
          const phase = ((h0 >>> 8) & 1023) / 1024;
          const tick = Math.floor(t * L.rate + phase * 8);
          const ci = this.pick(hash(ix, wy, tick + L.seed * 7));
          const noise = 0.5 + 0.5 * Math.sin(ix * 0.37 * (24 / L.cell) + t * 0.6 + L.seed) * Math.sin(wy * 0.31 - t * 0.45);
          let a = L.alpha * (0.5 + 0.5 * noise);
          if (wd && presence > 0.01) {   // the word sits on a calm patch of the field (soft edges)
            const ddy = y - wd.y, ddx = Math.abs(x - wd.cxw) - (wd.chars.length * ADV) / 2;
            a *= 1 - 0.8 * presence * Math.exp(-(ddy * ddy) / (2 * 15 * 15)) * (1 - smooth(0, 34, ddx));
          }
          let lit = 0;
          if (!this.reduced) {
            const d = Math.sqrt(d2);
            for (const w of this.waves) {
              const r = RING_R + (now - w.t0) * w.speed;
              let k = Math.exp(-((d - r) * (d - r)) / (2 * 26 * 26)) * w.amp * (1 - (r - RING_R) / 540);
              if (w.ang !== undefined && k > 0.01) {
                let da = Math.atan2(dy, dx) - w.ang; da = Math.atan2(Math.sin(da), Math.cos(da));
                k *= Math.exp(-(da * da) / (2 * 0.42 * 0.42));
              }
              if (k > lit) lit = k;
            }
            if (wd && bloom > 0.02) {
              const ddx = x - wd.cxw, ddy = y - wd.y;
              lit = Math.max(lit, bloom * 0.7 * Math.exp(-(ddx * ddx * 0.5 + ddy * ddy * 2.2) / (2 * 80 * 80)));
            }
            a += lit * 0.5;
          }
          const row = lit > 0.35 ? 1 : 0;
          ctx.globalAlpha = Math.min(0.85, a) * alpha;
          ctx.drawImage(at.img, ci * at.slotW * at.dpr, row * at.slotH * at.dpr, at.slotW * at.dpr, at.slotH * at.dpr, x - at.slotW / 2, y - at.slotH / 2, at.slotW, at.slotH);
        }
      }
    }
    ctx.globalAlpha = alpha;

    // sparks of a stage beat
    for (const s of this.sparks) {
      const u = (now - s.t0) / s.life, k = 1 - u;
      ctx.fillStyle = css(acc, 0.8 * k * k * alpha);
      ctx.beginPath(); ctx.arc(s.x + s.vx * (now - s.t0), s.y + s.vy * (now - s.t0), s.size * (0.5 + k * 0.5), 0, Math.PI * 2); ctx.fill();
    }

    // vignette: the field is brightest around the orb and fades towards the edges
    ctx.globalAlpha = 1;
    ctx.globalCompositeOperation = "destination-in";
    const v = ctx.createRadialGradient(cx, cy + 30, 150, cx, cy + 30, Math.max(W * 0.62, 420));
    v.addColorStop(0, "rgba(0,0,0,1)"); v.addColorStop(1, "rgba(0,0,0,0.22)");
    ctx.fillStyle = v; ctx.fillRect(0, 0, W, H);
    ctx.globalCompositeOperation = "source-over";
    // the word is drawn after the vignette so it keeps its full brightness
    if (wd && !this.reduced) this.drawWord(ctx, wd, el, bloom, acc, wideAcc, dark, alpha);
    ctx.restore();
  }

  private drawWord(ctx: CanvasRenderingContext2D, w: Word, el: number, bloom: number, acc: number[], wordCol: number[], dark: boolean, alpha: number) {
    // Soft bloom behind the word: a radial gradient squeezed into an ellipse (no shadowBlur).
    if (bloom > 0.02) {
      const n = w.chars.length * ADV;
      for (const [rx, ry, k] of [[n * 0.8 + 90, 84, 0.18], [n * 0.5 + 30, 38, 0.42]] as const) {
        ctx.save();
        ctx.translate(w.cxw, w.y); ctx.scale(rx / ry, 1);
        const g = ctx.createRadialGradient(0, 0, 0, 0, 0, ry);
        g.addColorStop(0, css(acc, (dark ? 1 : 0.8) * k * bloom * alpha)); g.addColorStop(0.55, css(acc, (dark ? 1 : 0.8) * k * 0.35 * bloom * alpha)); g.addColorStop(1, css(acc, 0));
        ctx.fillStyle = g; ctx.fillRect(-ry, -ry, ry * 2, ry * 2);
        ctx.restore();
      }
    }
    ctx.font = `700 ${WORD_PX}px 'JetBrains Mono', monospace`; ctx.textAlign = "center"; ctx.textBaseline = "middle";
    const glyphs = LETTERS;
    w.chars.forEach((ch, i) => {
      const arrive = Math.max(0, (w.d[i] - RING_R) / WAVE_SPEED);
      const lock = w.lock[i], leave = w.leave[i];
      if (el < arrive) return;
      const hh = hash(i, w.seed, 5), r1 = (hh & 255) / 255, r2 = ((hh >>> 8) & 255) / 255;
      let x = w.x[i], y = w.y, rot = 0, sc = 1, a = 1, flash = 0, show = ch;
      if (el < lock) {
        // still searching: candidates flicker (new glyph every ~45 ms), fading in as the wave passes
        const f = (el - arrive) / Math.max(1, lock - arrive);
        show = glyphs[hash(i, Math.floor(el / 45), w.seed) % glyphs.length];
        a = 0.25 + 0.55 * f; sc = 0.8 + 0.2 * f;
      } else if (el < leave) {
        const u = Math.min(1, (el - lock) / 300);
        sc = 0.6 + 0.4 * easeOutBack(u); flash = (1 - u) * 0.55; a = Math.min(1, 0.4 + u * 0.6 + 0.2);
        // a soft highlight travels along the settled word once
        const sweepX = w.x[0] - 30 + ((el - Math.max(...w.lock)) / 800) * (w.chars.length * ADV + 60);
        flash = Math.max(flash, 0.55 * Math.exp(-((x - sweepX) ** 2) / (2 * 22 * 22)));
        y += Math.sin(el / 380 + i * 0.7) * 0.8; // breathing
      } else {
        // blown away: drifts with the wind, tumbles, turns back into noise and fades
        const u = Math.min(1, (el - leave) / 780), e = u * u * (3 - 2 * u);
        x += w.wind * e * (46 + r1 * 70); y += -e * (8 + r2 * 34) + u * u * 14 * r1; rot = (r1 - 0.5) * 1.2 * e;
        sc = 1 - 0.25 * e; a = Math.pow(1 - u, 1.4);
        if (u > 0.3) show = glyphs[hash(i, Math.floor(el / 60), w.seed) % glyphs.length];
      }
      ctx.save();
      ctx.translate(x, y); if (rot) ctx.rotate(rot); ctx.scale(sc, sc);
      ctx.globalAlpha = a * alpha;
      ctx.fillStyle = css(mix(wordCol, dark ? [250, 250, 252] : [255, 255, 255], flash));
      ctx.fillText(show, 0, 0);
      ctx.restore();
    });
  }
}

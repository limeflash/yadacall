/**
 * Canvas visualiser of the recording screen, ported from the design: the radial "orb" (76 bars split into me = left half,
 * them = right half, plus the idle comet, the mascot particles and the processing ring) and the dotted background that
 * ripples with speech. Input levels come from real `Levels` events (the mock emits the same shape).
 */
import { levels } from "./state.svelte";
import { isVisible } from "./visibility";

export interface VisState { phase: "idle" | "rec" | "proc" | "done"; paused: boolean; job: number; hover: boolean }
export interface VisColors { acc: string; dark: boolean }

const rgb = (h: string) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
const mixc = (a: number[], b: number[], k: number) => a.map((v, i) => Math.round(v + (b[i] - v) * k));
const css = (c: number[], a = 1) => `rgba(${c[0]},${c[1]},${c[2]},${a})`;

export class Visualizer {
  private a = { rec: 0, proc: 0, hov: 0, lm: 0, lt: 0, burst: 0, cat: 0 };
  private ripples: { t0: number; me: boolean; amp: number }[] = [];
  private lastRip = 0;
  private raf = 0;
  private parts = Array.from({ length: 26 }, (_, i) => ({ ph: Math.random(), sp: 0.12 + Math.random() * 0.16, side: i % 2 ? 1 : -1, off: (Math.random() - 0.5) * 1.4, sz: 1.2 + Math.random() * 1.8 }));
  private motion = typeof matchMedia !== "undefined" ? matchMedia("(prefers-reduced-motion: reduce)") : null;
  private reduced = this.motion?.matches ?? false;
  private last = 0;
  private frame = 0;
  /** Milliseconds since the previous frame; the easing below is tuned for 60 fps and scaled by this. */
  private dt = 16.7;

  constructor(private orb: HTMLCanvasElement, private bg: HTMLCanvasElement, private state: () => VisState, private colors: () => VisColors) {}

  /**
   * Draws at most 30 fps (10 with reduced motion), and not at all while the window is hidden, minimised or
   * covered (`visibilitychange`).
   */
  start() {
    this.motion?.addEventListener("change", this.onMotion);
    document.addEventListener("visibilitychange", this.onVis);
    this.onVis();
  }
  stop() {
    this.motion?.removeEventListener("change", this.onMotion);
    document.removeEventListener("visibilitychange", this.onVis);
    cancelAnimationFrame(this.raf); this.raf = 0;
  }
  private onMotion = () => { this.reduced = this.motion?.matches ?? false; };
  private onVis = () => {
    if (!isVisible()) { cancelAnimationFrame(this.raf); this.raf = 0; return; }
    if (this.raf) return;
    this.last = 0;
    this.raf = requestAnimationFrame(this.loop);
  };
  private loop = (ts: number) => {
    this.raf = 0;
    if (!isVisible()) return;
    const every = this.reduced ? 100 : 1000 / 30;
    if (this.last === 0 || ts - this.last >= every - 3) {
      this.dt = this.last === 0 ? 16.7 : Math.min(100, ts - this.last);
      this.last = ts;
      this.draw(ts);
      // The dotted background changes slowly and costs the most: every other frame is enough.
      if ((this.frame++ & 1) === 0) this.drawBg(ts);
    }
    this.raf = requestAnimationFrame(this.loop);
  };
  /** Per-frame easing factor `f` (defined at 60 fps) for the current frame time. */
  private k = (f: number) => 1 - Math.pow(1 - f, this.dt / 16.7);
  burst() { this.a.burst = 1; }

  private drawBg(tsRaw: number) {
    const c = this.bg;
    const ts = this.reduced ? 0 : tsRaw;
    const W = c.clientWidth || 792, H = 520, dpr = 1; // faint dots: no need for retina pixels
    if (c.width !== Math.round(W * dpr)) { c.width = Math.round(W * dpr); c.height = Math.round(H * dpr); }
    const ctx = c.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0); ctx.clearRect(0, 0, W, H);
    const s = this.state(), a = this.a, t = ts / 1000, { dark } = this.colors();
    const acc = rgb(this.colors().acc);
    const them = dark ? [215, 215, 225] : [70, 70, 80];
    const ink = dark ? [255, 255, 255] : [20, 20, 26];
    const cx = W / 2, cy = 112, G = 16;
    const nowT = tsRaw / 1000;
    if (s.phase === "rec" && !s.paused && tsRaw - this.lastRip > 260 && Math.max(a.lm, a.lt) > 0.3) {
      this.ripples.push({ t0: nowT, me: a.lm >= a.lt, amp: Math.max(a.lm, a.lt) }); this.lastRip = tsRaw;
    }
    this.ripples = this.ripples.filter((r) => nowT - r.t0 < 3.2);
    const idle = 1 - Math.max(a.rec, a.proc);
    const p = s.job / 100;
    const sweep = -Math.PI / 2 + p * Math.PI * 2;
    const CH = "абвгдеёжзиклмнопрстуфхцчшэюя";
    ctx.font = "600 9px 'JetBrains Mono', monospace"; ctx.textAlign = "center"; ctx.textBaseline = "middle";
    for (let gy = G / 2; gy < H; gy += G) for (let gx = G / 2; gx < W; gx += G) {
      const dx = gx - cx, dy = gy - cy, d = Math.hypot(dx, dy);
      if (d < 92) continue;
      const fall = Math.max(0, 1 - d / 360);
      const v = (Math.sin(gx * 0.045 + t * 0.7) + Math.sin(gy * 0.05 - t * 0.5) + Math.sin((gx + gy) * 0.03 + t * 0.35)) / 3;
      const al = 0.05 + fall * 0.06;
      let glow = Math.pow(Math.max(0, v), 3) * 0.55 * idle * fall;
      let rip = 0, ripMe = true;
      for (const r of this.ripples) {
        const rr = 92 + (nowT - r.t0) * 110, k = Math.exp(-((d - rr) ** 2) / 90) * r.amp * (1 - (nowT - r.t0) / 3.2);
        if (k > rip) { rip = k; ripMe = r.me; }
      }
      const side = dx < 0;
      if (rip > 0.02) glow = Math.max(glow, rip * (ripMe === side ? 1 : 0.35));
      const size = 1.1 + glow * 1.4;
      if (a.proc > 0.02) {
        let ang = Math.atan2(dy, dx); if (ang < -Math.PI / 2) ang += Math.PI * 2;
        const behind = sweep - ang;
        const h = ((gx * 73856093) ^ (gy * 19349663)) >>> 0;
        if (behind > 0 && fall > 0.05) {
          const fresh = Math.exp(-behind * 2.2);
          const flick = fresh > 0.3 ? Math.floor(nowT * 14 + h) : h;
          const chA = (0.1 + fresh * 0.8) * fall * 1.6 * a.proc;
          if (chA > 0.03) { ctx.fillStyle = css(fresh > 0.3 ? acc : ink, Math.min(1, chA)); ctx.fillText(CH[flick % CH.length], gx, gy); continue; }
        }
      }
      const cc = glow > 0.05 ? (rip > 0.02 && !side ? them : acc) : ink;
      ctx.fillStyle = css(cc, Math.min(1, al + glow));
      ctx.fillRect(gx - size / 2, gy - size / 2, size, size);
    }
    // Fade the grid out towards the bottom here instead of with a CSS mask, which forces the GPU to
    // re-composite this animating canvas through an offscreen layer every frame.
    const fade = ctx.createLinearGradient(0, 0, 0, H);
    fade.addColorStop(0.45, "#000"); fade.addColorStop(0.8, "rgba(0,0,0,0)");
    ctx.globalCompositeOperation = "destination-in"; ctx.fillStyle = fade; ctx.fillRect(0, 0, W, H);
    ctx.globalCompositeOperation = "source-over";
  }

  private draw(tsRaw: number) {
    const c = this.orb;
    const S = 250, dpr = window.devicePixelRatio || 1;
    if (c.width !== S * dpr) { c.width = S * dpr; c.height = S * dpr; }
    const ctx = c.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0); ctx.clearRect(0, 0, S, S);
    const s = this.state(), a = this.a, t = (this.reduced ? 0 : tsRaw) / 1000, cx = S / 2, cy = S / 2;
    const rec = s.phase === "rec", live = rec && !s.paused, proc = s.phase === "proc";
    a.rec += ((rec ? 1 : 0) - a.rec) * this.k(0.09);
    a.proc += ((proc ? 1 : 0) - a.proc) * this.k(0.08);
    a.hov += ((s.hover ? 1 : 0) - a.hov) * this.k(0.08);
    const tm = live ? 0.04 + Math.min(1, levels.mic.peak) * 0.96 : 0.04;
    const tt = live ? 0.04 + Math.min(1, levels.system.peak) * 0.96 : 0.04;
    a.lm += (tm - a.lm) * this.k(0.18); a.lt += (tt - a.lt) * this.k(0.18);
    a.burst *= Math.pow(0.955, this.dt / 16.7);
    const { acc: accHex, dark } = this.colors();
    const acc = rgb(accHex);
    const them = dark ? [215, 215, 225] : [80, 80, 92];
    const idle = 1 - Math.max(a.rec, a.proc);
    a.cat += ((s.phase === "idle" ? 1 : 0) - a.cat) * this.k(0.075);
    const kc = a.cat;
    if (kc > 0.01) {
      const tx = [cx - 38, cy + 4], tx2 = [cx + 40, cy + 6];
      for (const q of this.parts) {
        const ph = (q.ph + t * q.sp * (1 + a.hov * 1.5)) % 1;
        const ang = (q.side < 0 ? Math.PI : 0) + q.off;
        const r = 118 - ph * 80;
        const tgt = q.side < 0 ? tx : tx2;
        const x = tgt[0] + Math.cos(ang) * r + Math.sin(ph * 9 + q.off * 5) * 5;
        const y = tgt[1] + Math.sin(ang) * r * 0.9;
        const al = Math.sin(ph * Math.PI) * kc * (0.55 + a.hov * 0.45);
        ctx.beginPath(); ctx.arc(x, y, q.sz * (1 - ph * 0.5), 0, Math.PI * 2);
        ctx.fillStyle = css(acc, al); ctx.fill();
      }
    }
    if (a.burst > 0.02) {
      const r = 70 + (1 - a.burst) * 60;
      ctx.beginPath(); ctx.arc(cx, cy, r, 0, Math.PI * 2);
      ctx.strokeStyle = css(acc, a.burst * 0.8); ctx.lineWidth = 3 * a.burst + 0.5; ctx.stroke();
      for (let i = 0; i < 14; i++) {
        const th = (i / 14) * Math.PI * 2 + 0.3, rr = 80 + (1 - a.burst) * 50;
        ctx.beginPath(); ctx.arc(cx + Math.cos(th) * rr, cy + Math.sin(th) * rr, 3 * a.burst + 0.5, 0, Math.PI * 2);
        ctx.fillStyle = css(acc, a.burst); ctx.fill();
      }
    }
    {
      const N = 76, head = t * 1.1;
      ctx.lineCap = "round";
      for (let i = 0; i < N; i++) {
        const th = (i / N) * Math.PI * 2 - Math.PI / 2;
        const left = Math.cos(th) < 0;
        const lvl = left ? a.lm : a.lt;
        const n = (Math.sin(i * 1.9 + t * 11) * 0.5 + 0.5) * (Math.sin(i * 0.7 - t * 6.4) * 0.5 + 0.5);
        const d = (((th - head) % (Math.PI * 2)) + Math.PI * 2) % (Math.PI * 2);
        const comet = Math.pow(1 - d / (Math.PI * 2), 5);
        const len = (3 + lvl * (6 + n * 40)) * a.rec;
        const r0 = 72 + 4 * a.hov * idle;
        const idleA = (0.16 + 0.7 * comet + 0.25 * a.hov) * idle * (1 - a.proc) * (1 - kc);
        const recA = (0.35 + lvl * 0.65) * a.rec * (s.paused ? 0.45 : 1);
        ctx.lineWidth = 3 + comet * 1.2 * idle;
        ctx.beginPath();
        ctx.moveTo(cx + Math.cos(th) * r0, cy + Math.sin(th) * r0);
        ctx.lineTo(cx + Math.cos(th) * (r0 + len + 0.01), cy + Math.sin(th) * (r0 + len + 0.01));
        ctx.strokeStyle = css(left || a.rec < 0.5 ? acc : them, Math.min(1, idleA + recA)); ctx.stroke();
      }
    }
    if (a.proc > 0.01) {
      const p = s.job / 100;
      ctx.lineCap = "round"; ctx.lineWidth = 5;
      ctx.beginPath(); ctx.arc(cx, cy, 84, 0, Math.PI * 2); ctx.strokeStyle = css(acc, 0.12 * a.proc); ctx.stroke();
      ctx.beginPath(); ctx.arc(cx, cy, 84, -Math.PI / 2, -Math.PI / 2 + Math.PI * 2 * p); ctx.strokeStyle = css(acc, a.proc); ctx.stroke();
      for (let k = 0; k < 3; k++) {
        const th = t * (1.6 + k * 0.5) + k * 2.1, rr = 100 + k * 6;
        ctx.beginPath(); ctx.arc(cx + Math.cos(th) * rr, cy + Math.sin(th) * rr, 2.5 - k * 0.5, 0, Math.PI * 2);
        ctx.fillStyle = css(acc, 0.7 * a.proc); ctx.fill();
      }
    }
    if (kc > 0.99) return;
    ctx.globalAlpha = Math.min(1, (1 - kc) * 1.4);
    const ek = kc * kc * (3 - 2 * kc);
    const R = (1 - ek) * 60 - 6 * a.rec - 6 * a.proc + 3 * a.hov * idle + (a.lm + a.lt) * 3 * a.rec + Math.sin(t * 1.6) * 0.8 * idle;
    const deep = dark ? [29, 29, 34] : [22, 22, 26];
    const col = mixc(acc, deep, a.rec);
    if (R <= 0.5) { ctx.globalAlpha = 1; return; }
    const g = ctx.createRadialGradient(cx - R * 0.35, cy - R * 0.45, R * 0.1, cx, cy, R * 1.1);
    g.addColorStop(0, css(mixc(col, [255, 255, 255], 0.28 * (1 - a.rec) + 0.06 * a.rec))); g.addColorStop(1, css(col));
    // Glow: a radial gradient instead of `shadowBlur`, which re-blurs the whole disc every frame on the GPU.
    const glow = ctx.createRadialGradient(cx, cy + 8, R * 0.7, cx, cy + 8, R * 1.55);
    glow.addColorStop(0, css(acc, 0.4 * (1 - a.rec * 0.5))); glow.addColorStop(1, css(acc, 0));
    ctx.beginPath(); ctx.arc(cx, cy + 8, R * 1.55, 0, Math.PI * 2); ctx.fillStyle = glow; ctx.fill();
    ctx.beginPath(); ctx.arc(cx, cy, R, 0, Math.PI * 2);
    ctx.fillStyle = g; ctx.fill();
    if (a.rec > 0.01) {
      ctx.beginPath(); ctx.arc(cx, cy, R - 1, 0, Math.PI * 2);
      ctx.strokeStyle = css(acc, a.rec); ctx.lineWidth = 2; ctx.stroke();
    }
    ctx.globalAlpha = 1;
  }
}

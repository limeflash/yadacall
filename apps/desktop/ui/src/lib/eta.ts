/** Remaining-time estimate from the progress rate; stays hidden until consecutive estimates agree. */
export class Eta {
  private first: { t: number; p: number } | null = null;
  private est: number[] = [];
  private smooth: number | null = null;
  private at = 0;

  reset() { this.first = null; this.est = []; this.smooth = null; this.at = 0; }

  /** `p` is 0..100. */
  push(p: number, now = Date.now()) {
    if (!this.first) { this.first = { t: now, p }; return; }
    const dp = p - this.first.p, dt = (now - this.first.t) / 1000;
    if (dt < 4 || dp < 2) return;
    const left = ((100 - p) / dp) * dt;
    this.smooth = this.smooth == null ? left : this.smooth * 0.7 + left * 0.3;
    this.at = now;
    this.est.push(this.smooth);
    if (this.est.length > 4) this.est.shift();
  }

  /** Seconds left, or null while the estimate is still moving around. */
  seconds(now = Date.now()): number | null {
    if (this.est.length < 3 || this.smooth == null) return null;
    const lo = Math.min(...this.est), hi = Math.max(...this.est);
    return hi - lo <= Math.max(3, 0.35 * hi) ? Math.max(1, Math.round(this.smooth - (now - this.at) / 1000)) : null;
  }
}

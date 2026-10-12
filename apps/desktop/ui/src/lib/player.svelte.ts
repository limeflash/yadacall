/**
 * Audio playback for a meeting. With a real `audioUrl` it drives an <audio>; without one (demo data, audio deleted
 * by the retention policy) it runs a virtual clock so the transcript still follows along.
 */
export class Player {
  pos = $state(0);        // ms
  playing = $state(false);
  speed = $state(1);
  touched = $state(false);
  private audio?: HTMLAudioElement;
  private timer?: ReturnType<typeof setInterval>;
  private last = 0;

  constructor(public duration: number, url?: string) {
    if (url) {
      this.audio = new Audio(url);
      this.audio.preload = "metadata";
      this.audio.addEventListener("timeupdate", () => { this.pos = this.audio!.currentTime * 1000; });
      this.audio.addEventListener("ended", () => { this.playing = false; });
    }
  }
  get real() { return !!this.audio; }

  private tick = () => {
    const now = performance.now();
    this.pos = Math.min(this.duration, this.pos + (now - this.last) * this.speed);
    this.last = now;
    if (this.pos >= this.duration) this.pause();
  };

  play() {
    this.touched = true;
    this.playing = true;
    if (this.audio) { this.audio.playbackRate = this.speed; void this.audio.play().catch(() => { this.playing = false; }); return; }
    this.last = performance.now();
    clearInterval(this.timer);
    this.timer = setInterval(this.tick, 100);
  }
  pause() {
    this.playing = false;
    this.audio?.pause();
    clearInterval(this.timer);
  }
  toggle() { this.playing ? this.pause() : this.play(); }
  seek(ms: number, play = false) {
    this.touched = true;
    this.pos = Math.max(0, Math.min(this.duration, ms));
    if (this.audio) this.audio.currentTime = this.pos / 1000;
    if (play) this.play();
  }
  cycleSpeed() {
    const order = [1, 1.5, 2];
    this.speed = order[(order.indexOf(this.speed) + 1) % order.length];
    if (this.audio) this.audio.playbackRate = this.speed;
  }
  get speedLabel() { return this.speed === 1 ? "1×" : this.speed === 1.5 ? "1,5×" : "2×"; }
  destroy() { this.pause(); this.audio?.removeAttribute("src"); }
}

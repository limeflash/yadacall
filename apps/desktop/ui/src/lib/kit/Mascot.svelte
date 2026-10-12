<script lang="ts">
  /** The headphone-cat mark: layered WebP parts with blink / look-around / earcup-bob animation (design `ycm*` keyframes). */
  import band from "../../assets/mascot/band.webp";
  import cupR from "../../assets/mascot/cup-r.webp";
  import face from "../../assets/mascot/face.webp";
  import head from "../../assets/mascot/head.webp";
  import eyeL from "../../assets/mascot/eye-l.webp";
  import eyeR from "../../assets/mascot/eye-r.webp";
  import cupL from "../../assets/mascot/cup-l.webp";
  import monoBlack from "../../assets/mascot/mono-black.webp";
  import monoWhite from "../../assets/mascot/mono-white.webp";

  let { size = 30, accent = "#7dd3a0", variant = "color", animated = true, alt = "" }:
    { size?: number | string; accent?: string; variant?: "color" | "white" | "black"; animated?: boolean; alt?: string } = $props();

  // The layers are authored in mint; other accents are produced by hue-rotating the mint set (as the design's export does).
  function tint(acc: string): string {
    if (!/^#[0-9a-f]{6}$/i.test(acc) || acc.toLowerCase() === "#7dd3a0") return "none";
    const r = parseInt(acc.slice(1, 3), 16) / 255, g = parseInt(acc.slice(3, 5), 16) / 255, b = parseInt(acc.slice(5, 7), 16) / 255;
    const mx = Math.max(r, g, b), mn = Math.min(r, g, b), d = mx - mn;
    let h = 0;
    if (d) h = mx === r ? ((g - b) / d) % 6 : mx === g ? (b - r) / d + 2 : (r - g) / d + 4;
    h = (h * 60 + 360) % 360;
    const l = (mx + mn) / 2, sat = d ? d / (1 - Math.abs(2 * l - 1)) : 0;
    const rot = Math.round(((h - 144 + 540) % 360) - 180);
    return `hue-rotate(${rot}deg) saturate(${(0.7 + sat * 0.4).toFixed(2)})`;
  }
  const filter = $derived(tint(accent));
  const px = $derived(typeof size === "number" ? `${size}px` : size);
  const an = $derived(animated
    ? { blink: "ycmBlink 4.8s ease-in-out infinite", look: "ycmLook 7s ease-in-out infinite", cupL: "ycmCup 3.2s ease-in-out infinite", cupR: "ycmCup 3.2s ease-in-out .4s infinite", band: "ycmBand 3.2s ease-in-out infinite" }
    : { blink: "none", look: "none", cupL: "none", cupR: "none", band: "none" });
</script>

{#if variant === "color"}
  <div class="ycm-anim m" style="width: {px}" role={alt ? "img" : undefined} aria-label={alt || undefined} aria-hidden={alt ? undefined : true}>
    <div class="in" style="filter: {filter}">
      <img src={band} alt="" style="left: 22.4%; top: 18.47%; width: 36.4%; height: 28.72%; transform-origin: 20% 90%; animation: {an.band}" />
      <img src={cupR} alt="" style="left: 67.59%; top: 45.86%; width: 17.8%; height: 26.91%; animation: {an.cupR}" />
      <img src={face} alt="" style="left: 41.63%; top: 38.36%; width: 35.15%; height: 32.52%" />
      <img src={head} alt="" style="left: 24.53%; top: 20.26%; width: 53.01%; height: 61.86%" />
      <div class="look" style="animation: {an.look}">
        <img src={eyeL} alt="" style="left: 51.77%; top: 46.85%; width: 7.95%; height: 14.36%; animation: {an.blink}" />
        <img src={eyeR} alt="" style="left: 64.59%; top: 49.53%; width: 7.6%; height: 14.51%; animation: {an.blink}" />
      </div>
      <img src={cupL} alt="" style="left: 14.37%; top: 40.83%; width: 25.67%; height: 32.06%; animation: {an.cupL}" />
    </div>
  </div>
{:else}
  <div class="ycm-anim m" style="width: {px}" role={alt ? "img" : undefined} aria-label={alt || undefined} aria-hidden={alt ? undefined : true}>
    <div class="mono">
      <img src={variant === "white" ? monoWhite : monoBlack} alt="" style="inset: 0; width: 100%; height: 100%" />
      <div class="look" style="animation: {an.look}">
        <div class="eye" style="left: 53.89%; top: 46.45%; width: 8.67%; height: 17.33%; background: {variant === 'white' ? '#fff' : '#0c0c0e'}; animation: {an.blink}"></div>
        <div class="eye" style="left: 72.11%; top: 50.89%; width: 7.33%; height: 16.22%; background: {variant === 'white' ? '#fff' : '#0c0c0e'}; animation: {an.blink}"></div>
      </div>
    </div>
  </div>
{/if}

<style>
  .m { position: relative; aspect-ratio: 1 / 1; overflow: visible; flex: none; }
  .in { position: absolute; left: -19.44%; top: -20.22%; width: 139.33%; aspect-ratio: 1 / 1; transition: filter 0.6s; }
  .mono { position: absolute; inset: 0; }
  .look { position: absolute; inset: 0; }
  img { position: absolute; pointer-events: none; }
  .eye { position: absolute; border-radius: 999px; transform: rotate(1.7deg); }
</style>

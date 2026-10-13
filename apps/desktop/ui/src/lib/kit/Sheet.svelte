<script lang="ts">
  /** A side sheet over the current screen: veil, title bar with close, scrolling body, optional footer. Esc and the veil close it. */
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";
  import { t } from "../i18n/index.svelte";
  let { open = $bindable(false), title, children, footer, onclose }:
    { open?: boolean; title: string; children: Snippet; footer?: Snippet; onclose?: () => void } = $props();
  let panel = $state<HTMLElement | null>(null);
  let opener: HTMLElement | null = null;
  /** Moves the node to <body>: ancestors with a transform (screen-enter animations) would otherwise become the containing block of `position: fixed`. */
  function portal(node: HTMLElement) { document.body.appendChild(node); return { destroy() { node.remove(); } }; }
  const titleId = "sheet" + Math.random().toString(36).slice(2, 7);
  function close() { open = false; onclose?.(); }
  $effect(() => {
    if (!open) return;
    opener = document.activeElement as HTMLElement | null;
    queueMicrotask(() => panel?.focus());
    return () => { opener?.focus?.(); };
  });
  function key(e: KeyboardEvent) {
    if (e.key === "Escape" && !e.defaultPrevented) { e.preventDefault(); close(); }
    // keep Tab inside the panel
    if (e.key === "Tab" && panel) {
      const f = [...panel.querySelectorAll<HTMLElement>('button:not([disabled]), a[href], input:not([disabled]), [tabindex]:not([tabindex="-1"])')].filter((x) => x.offsetParent !== null);
      if (!f.length) return;
      const first = f[0], last = f[f.length - 1];
      if (e.shiftKey && (document.activeElement === first || document.activeElement === panel)) { e.preventDefault(); last.focus(); }
      else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
    }
  }
</script>

{#if open}
  <div use:portal class="portal">
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="veil" onclick={close}></div>
  <div bind:this={panel} class="sheet" role="dialog" aria-modal="true" aria-labelledby={titleId} tabindex="-1" onkeydown={key}>
    <header>
      <h2 id={titleId}>{title}</h2>
      <button type="button" class="x yc-press" aria-label={t("Закрыть")} onclick={close}><Icon name="x" size={16} /></button>
    </header>
    <div class="body yc-scroll">{@render children()}</div>
    {#if footer}<div class="foot">{@render footer()}</div>{/if}
  </div>
  </div>
{/if}

<style>
  .veil { position: fixed; inset: 0; z-index: 30; background: var(--backdrop); animation: sheetFade 0.2s ease-out both; }
  .sheet {
    position: fixed; z-index: 31; top: 0; right: 0; bottom: 0; width: min(540px, 100%); box-sizing: border-box; display: flex; flex-direction: column;
    background: var(--bg); border-left: 1px solid var(--border); box-shadow: -24px 0 60px rgba(0, 0, 0, 0.25); outline: none;
    --cat-bg: var(--bg); --cat-pad: 20px; animation: sheetIn 0.32s var(--ease-out-soft, ease-out) both;
  }
  header { flex: none; display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 16px 20px 10px; }
  h2 { margin: 0; font-size: 18px; font-weight: 800; letter-spacing: -0.03em; }
  .x { width: 30px; height: 30px; border: 0; border-radius: 50%; background: var(--surface-2); color: var(--muted); display: grid; place-items: center; cursor: pointer; }
  .x:hover { color: var(--text); }
  .x:focus-visible { outline: 2px solid var(--accent-line); }
  .body { flex: 1; min-height: 0; padding: 0 20px 20px; }
  .foot { flex: none; padding: 12px 20px 16px; border-top: 1px solid var(--border); background: var(--surface); }
  @keyframes sheetIn { from { transform: translateX(28px); opacity: 0; } to { transform: none; opacity: 1; } }
  @keyframes sheetFade { from { opacity: 0; } to { opacity: 1; } }
  @media (prefers-reduced-motion: reduce) { .sheet, .veil { animation: none; } }
</style>

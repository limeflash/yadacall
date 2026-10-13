<script lang="ts">
  import Icon from "./Icon.svelte";
  import { t } from "../i18n/index.svelte";
  /** Search box with a clear button; Esc clears (or leaves the field when it is already empty). Expose `el` to focus it from outside. */
  let { value = $bindable(""), placeholder = "", label = "", el = $bindable<HTMLInputElement | null>(null), hint = "" }:
    { value?: string; placeholder?: string; label?: string; el?: HTMLInputElement | null; hint?: string } = $props();
  function key(e: KeyboardEvent) {
    if (e.key !== "Escape") return;
    e.preventDefault();
    if (value) value = ""; else el?.blur();
  }
</script>

<label class="field">
  <span class="mg-sr">{label}</span>
  <span class="ic" aria-hidden="true"><Icon name="search" size={15} /></span>
  <input bind:this={el} bind:value type="text" {placeholder} autocomplete="off" spellcheck="false" onkeydown={key} />
  {#if value}
    <button type="button" class="clear yc-press" aria-label={t("Очистить поиск")} onclick={() => { value = ""; el?.focus(); }}><Icon name="x" size={13} /></button>
  {:else if hint}
    <kbd class="hint mg-mono" aria-hidden="true">{hint}</kbd>
  {/if}
</label>

<style>
  .field { display: flex; align-items: center; gap: 9px; height: 40px; box-sizing: border-box; padding: 0 8px 0 13px; border: 1px solid var(--border); border-radius: var(--radius-field); background: var(--surface); transition: border-color 0.2s; }
  .field:focus-within { border-color: var(--accent-line); }
  .ic { flex: none; display: grid; color: var(--muted); }
  input { flex: 1; min-width: 0; height: 100%; margin: 0; padding: 0; border: 0; outline: 0; background: transparent; color: var(--text); font: inherit; font-size: 14px; }
  input::placeholder { color: var(--muted); }
  .clear { flex: none; width: 26px; height: 26px; border: 0; border-radius: 50%; background: var(--surface-2); color: var(--muted); display: grid; place-items: center; cursor: pointer; }
  .clear:hover { color: var(--text); }
  .clear:focus-visible { outline: 2px solid var(--accent-line); }
  .hint { flex: none; min-width: 20px; height: 20px; padding: 0 5px; border: 1px solid var(--border); border-bottom-width: 2px; border-radius: 6px; background: var(--surface-2); color: var(--muted); font-size: 11px; font-weight: 700; display: grid; place-items: center; }
</style>

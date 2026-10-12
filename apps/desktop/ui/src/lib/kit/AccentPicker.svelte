<script lang="ts">
  import { t } from "../i18n/index.svelte";
  const presets = [
    { id: "mint", color: "#7dd3a0", label: "Мятный" }, { id: "lavender", color: "#b8acf2", label: "Лаванда" }, { id: "sky", color: "#8ec8ea", label: "Небесный" },
    { id: "sand", color: "#dcc68c", label: "Песочный" }, { id: "rose", color: "#e6a6b0", label: "Роза" }, { id: "sage", color: "#b0cc90", label: "Шалфей" },
  ];
  let { value, onchange }: { value: string; onchange: (hex: string) => void } = $props();
  let draft = $state<string | null>(null);
  let root: HTMLElement;
  const accent = $derived(value.toLowerCase());
  const shown = $derived(draft ?? accent.slice(1));
  const valid = $derived(/^[0-9a-f]{6}$/i.test(shown));
  function keys(e: KeyboardEvent) {
    const d: Record<string, number> = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 };
    if (!(e.key in d)) return;
    e.preventDefault();
    const i = Math.max(0, presets.findIndex((p) => p.color === accent));
    const n = (i + d[e.key] + presets.length) % presets.length;
    onchange(presets[n].color);
    root.querySelectorAll<HTMLElement>('[role="radio"]')[n]?.focus();
  }
  function input(e: Event) {
    const v = (e.target as HTMLInputElement).value.replace(/[^0-9a-f]/gi, "").slice(0, 6);
    draft = v;
    if (/^[0-9a-f]{6}$/i.test(v)) onchange("#" + v.toLowerCase());
  }
</script>

<div class="mg-accent">
  <div bind:this={root} role="radiogroup" aria-label={t("Акцент")} class="mg-accent__swatches" onkeydown={keys}>
    {#each presets as p (p.id)}
      {@const on = p.color === accent}
      <button type="button" role="radio" aria-checked={on} aria-label={t(p.label)} tabindex={on || (!presets.some((x) => x.color === accent) && p.id === "mint") ? 0 : -1} class="mg-swatch" onclick={() => onchange(p.color)}>
        <span class="mg-swatch__dot" style="background: {p.color}; --swatch: {p.color}"><span class="mg-swatch__check"></span></span>
        <span aria-hidden="true" class="mg-swatch__label">{t(p.label)}</span>
      </button>
    {/each}
  </div>
  <label class="mg-hex">
    <span class="mg-hex__label">{t("Свой")}</span>
    <span class="mg-hex__box {valid ? '' : 'mg-hex__box--bad'}">
      <span aria-hidden="true" class="mg-hex__hash">#</span>
      <input value={shown} maxlength="6" spellcheck="false" autocapitalize="off" autocomplete="off" aria-invalid={!valid} class="mg-hex__input" oninput={input} onblur={() => (draft = null)} />
      <span aria-hidden="true" class="mg-hex__preview" style="background: {valid ? '#' + shown : 'var(--surface-2)'}"></span>
    </span>
  </label>
</div>

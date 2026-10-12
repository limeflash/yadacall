<script lang="ts">
  import { t } from "../i18n/index.svelte";
  let { dark, onchange }: { dark: boolean; onchange: (next: "light" | "dark") => void } = $props();
</script>
<button type="button" onclick={() => onchange(dark ? "light" : "dark")} aria-label={dark ? t("Светлая тема") : t("Тёмная тема")} class="mg-theme-toggle {dark ? 'mg-theme-toggle--dark' : ''}">
  <span aria-hidden="true" class="mg-theme-toggle__knob"><span class="mg-theme-toggle__dot"></span></span>
</button>

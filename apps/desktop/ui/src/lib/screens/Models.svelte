<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import ModelCatalog from "../components/ModelCatalog.svelte";
  import StorageCleanup from "../components/StorageCleanup.svelte";
  import { t } from "../i18n/index.svelte";
  import { back } from "../router.svelte";
  import { bytes } from "../format";
  import { attachModels, diskBytes, mstore } from "../modelsStore.svelte";

  onMount(() => attachModels());

  const hardware = $derived(mstore.hardware);
  const used = $derived(mstore.models.reduce((s, m) => s + diskBytes(m), 0));
  const usedW = $derived(Math.min(100, (used / Math.max(1, hardware?.diskFreeBytes ?? 1)) * 100 * 8));
  const hwLine = $derived(hardware ? `${hardware.label} · ${hardware.chip} · ${hardware.ramGb} ${t("ГБ")}` : "");
</script>

<main>
  <div class="yc-scroll" style="flex: 1; min-height: 0">
    <div class="yc-in col">
      <button type="button" class="backlink" onclick={() => back({ name: "settings" })}><Icon name="back" size={14} />{t("Настройки")}</button>

      <header>
        <h1>{t("Модели")}</h1>
        <span class="yc-pretty lead">{t("Работают без интернета. Качаются один раз с Hugging Face — если связь пропадёт, докачаются с того же места.")}</span>
      </header>

      {#if hardware}
        <div class="hw">
          <span class="hw-ic"><Icon name="cpu" size={18} /></span>
          <span style="flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px">
            <span style="font-size: 13px; font-weight: 700">{hwLine}</span>
            <span style="font-size: 12px; color: var(--muted)">{t("Ускорение")}: <span class="mg-mono">{hardware.backend.toUpperCase()}</span> · {t("оценки ниже посчитаны под эту машину")}</span>
          </span>
        </div>

        <div style="display: flex; flex-direction: column; gap: 8px; padding: 2px 2px 4px">
          <div style="display: flex; justify-content: space-between; gap: 10px; font-size: 12px; color: var(--muted)">
            <span>{t("Модели занимают")} <span class="mg-mono" style="color: var(--text)">{bytes(used)}</span></span>
            <span>{t("свободно на диске")} <span class="mg-mono">{bytes(hardware.diskFreeBytes)}</span></span>
          </div>
          <span aria-hidden="true" class="usedbar"><span style="width: {usedW}%"></span></span>
          <StorageCleanup />
        </div>
      {/if}

      <ModelCatalog mode="manage" />

      <span class="note"><Icon name="info" size={12} />{t("Запись не ждёт загрузку: пока качается новая, расшифровывает основная.")}</span>
    </div>
  </div>
</main>

<style>
  main { position: relative; flex: 1 1 520px; min-width: 0; display: flex; flex-direction: column; min-height: 0; }
  .col { max-width: 600px; box-sizing: border-box; margin: 0 auto; padding: 24px 28px 40px; display: flex; flex-direction: column; gap: 14px; }
  .backlink { align-self: flex-start; display: inline-flex; align-items: center; gap: 6px; padding: 0; border: 0; background: none; font-size: 13px; color: var(--muted); }
  header { display: flex; flex-direction: column; gap: 4px; }
  h1 { margin: 0; font-size: 28px; line-height: 1.15; font-weight: 800; letter-spacing: -0.04em; }
  .lead { font-size: 13px; color: var(--muted); }
  .hw { display: flex; align-items: center; gap: 12px; padding: 11px 14px; border: 1px solid var(--border); border-radius: 14px; background: var(--surface); }
  .hw-ic { width: 36px; height: 36px; flex: none; border-radius: 11px; background: var(--accent-soft); color: var(--accent-text); display: grid; place-items: center; }
  .usedbar { display: block; height: 4px; border-radius: 2px; overflow: hidden; background: var(--surface-2); }
  .usedbar span { display: block; height: 100%; border-radius: 2px; background: var(--accent); transition: width 0.4s var(--ease-out-soft); }
  .note { display: flex; align-items: center; gap: 7px; padding: 2px 2px 0; font-size: 12px; color: var(--muted); }
</style>

<script lang="ts">
  import Icon from "./Icon.svelte";
  import { toasts, dismissToast } from "../toast.svelte";
  import { t } from "../i18n/index.svelte";
  let showDetail = $state(false);
  $effect(() => { toasts.error?.key; showDetail = false; });
</script>

<div class="mg-toasts">
  {#if toasts.error}
    {#key toasts.error.key}
      <div role="alert" aria-live="assertive" class="mg-toast mg-toast--error">
        <span aria-hidden="true" class="mg-toast__icon"><span class="mg-toast__bang">!</span></span>
        <span class="mg-toast__msg">
          {toasts.error.message}
          {#if toasts.error.detail}
            <button type="button" class="mg-toast__action" aria-expanded={showDetail} onclick={() => (showDetail = !showDetail)}>{t("Подробнее")}</button>
            {#if showDetail}<span class="mg-toast__detail">{toasts.error.detail}</span>{/if}
          {/if}
        </span>
        <button type="button" aria-label={t("Закрыть")} class="mg-toast__close" onclick={() => dismissToast("error")}><Icon name="x" size={12} /></button>
      </div>
    {/key}
  {/if}
  {#if toasts.note}
    {#key toasts.note.key}
      <div role="status" aria-live="polite" class="mg-toast">
        <span aria-hidden="true" class="mg-toast__icon"><span class="mg-toast__check"></span></span>
        <span class="mg-toast__msg">{toasts.note.message}</span>
        {#if toasts.note.actionLabel}
          <button type="button" class="mg-toast__action" onclick={() => { const f = toasts.note?.onAction; dismissToast("note"); f?.(); }}>{toasts.note.actionLabel}</button>
        {/if}
      </div>
    {/key}
  {/if}
</div>

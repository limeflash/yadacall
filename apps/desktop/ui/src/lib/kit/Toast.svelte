<script lang="ts">
  import Icon from "./Icon.svelte";
  let { message, error = false, actionLabel, onaction, onclose }: { message: string; error?: boolean; actionLabel?: string; onaction?: () => void; onclose?: () => void } = $props();
</script>
<div role={error ? "alert" : "status"} aria-live={error ? "assertive" : "polite"} class="mg-toast {error ? 'mg-toast--error' : ''}">
  <span aria-hidden="true" class="mg-toast__icon">{#if error}<span class="mg-toast__bang">!</span>{:else}<span class="mg-toast__check"></span>{/if}</span>
  <span class="mg-toast__msg">{message}</span>
  {#if actionLabel}<button type="button" class="mg-toast__action" onclick={onaction}>{actionLabel}</button>{/if}
  {#if error && onclose}<button type="button" aria-label="Закрыть" class="mg-toast__close" onclick={onclose}><Icon name="x" size={12} /></button>{/if}
</div>

<script lang="ts">
  const kinds = { ok: "!", warn: "!", bad: "×", off: "–", busy: "", blip: "~" } as const;
  let { kind, label, sm = false }: { kind: keyof typeof kinds; label: string; sm?: boolean } = $props();
</script>
<span class="mg-tone-{kind} mg-pill mg-status {sm ? 'mg-status--sm' : 'mg-status--md'}">
  <span aria-hidden="true" class="mg-pill__glyph {kind === 'busy' ? 'mg-pill__glyph--busy' : ''}">{#if kind === "ok"}<span class="mg-pill__check"></span>{:else if kind !== "busy"}{kinds[kind]}{/if}</span>{label}
</span>

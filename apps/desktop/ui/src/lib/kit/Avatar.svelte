<script lang="ts">
  const colors = ["var(--accent-lavender)", "var(--accent-sky)", "var(--accent-sand)", "var(--accent-rose)", "var(--accent-mint)", "var(--accent-sage)", "var(--avatar-7)", "var(--avatar-8)", "var(--avatar-9)"];
  let { name, index = 0, size = 28 }: { name: string; index?: number; size?: number } = $props();
  const initial = $derived((Array.from((name ?? "").trim())[0] ?? "?").toLocaleUpperCase());
</script>
<span aria-hidden="true" class="mg-avatar" style="width: {size}px; height: {size}px; font-size: {Math.round(size * 0.44)}px; background: {colors[index % colors.length]}"><span>{initial}</span></span>

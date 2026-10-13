<script lang="ts">
  /** Small rounded square with the vendor's initials, tinted by a deterministic pastel. Decorative: the vendor is also written out. */
  import { monogram, vendorTone } from "../models";
  let { vendor, size = 34 }: { vendor: string; size?: number } = $props();
  const text = $derived(monogram(vendor));
</script>

<span aria-hidden="true" class="mg-id-chip mark" data-tone={vendorTone(vendor)} style="width: {size}px; height: {size}px; font-size: {text.length > 1 ? Math.round(size * 0.36) : Math.round(size * 0.44)}px">{text}</span>

<style>
  .mark { border-radius: 10px; font-weight: 800; letter-spacing: -0.02em; user-select: none; }
</style>

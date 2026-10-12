<script lang="ts">
  import Icon from "./Icon.svelte";
  import Button from "./Button.svelte";
  import { toast } from "../toast.svelte";
  import { t } from "../i18n/index.svelte";
  let { value, label = t("Копировать"), size = "sm", variant = "secondary" }: { value: string | (() => string | Promise<string>); label?: string; size?: "sm" | "md"; variant?: "secondary" | "outline" | "ghost" } = $props();
  let done = $state(false);
  async function copy() {
    try {
      const v = typeof value === "function" ? await value() : value;
      await navigator.clipboard.writeText(v);
      done = true;
      setTimeout(() => (done = false), 2000);
    } catch {
      toast(t("Не удалось скопировать — скопируй вручную"));
    }
  }
</script>

<Button {variant} {size} onclick={copy}><Icon name={done ? "check" : "copy"} size={size === "sm" ? 12 : 14} />{done ? t("Скопировано") : label}</Button>

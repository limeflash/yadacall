import type { KeysSet, SummaryMode, SummarySettings } from "./types";

export const noKeys: KeysSet = { anthropic: false, openAi: false, openRouter: false, ollamaCloud: false };

/** Applies a `SettingsPatch.summaries` to the flattened summary settings (optimistic UI, browser mock). */
export function mergeSummaries(cur: SummarySettings, patch: { mode?: SummaryMode; key?: { account: keyof KeysSet; value: string } } | undefined): SummarySettings {
  if (!patch) return cur;
  const keysSet = { ...cur.keysSet };
  if (patch.key) keysSet[patch.key.account] = patch.key.value.length > 0;
  return { ...(patch.mode ?? cur), keysSet } as SummarySettings;
}

/** Model names that run on Ollama's servers. */
export const isCloudName = (name: string) => name.endsWith(":cloud") || name.endsWith("-cloud");

import type { Meeting } from "./api";
import { t } from "./i18n/index.svelte";

/** Display name for a speaker key within a meeting. Unnamed diarised voices read as "Гость". */
export function speakerName(m: Pick<Meeting, "participants">, key: string): string {
  if (key === "me") return t("Вы");
  const p = m.participants.find((x) => x.key === key);
  if (p?.name) return p.name;
  return key === "them" ? t("Собеседник") : t("Гость");
}
export function isNamed(m: Pick<Meeting, "participants">, key: string): boolean {
  return key === "me" || key === "them" || !!m.participants.find((x) => x.key === key)?.name;
}
/** Avatar / waveform colour slot: me=0, s1=1, s2=2, s3=3, anything further = 4 (muted). */
export function speakerIndex(key: string): number {
  if (key === "me") return 0;
  if (key === "them") return 1;
  const n = parseInt(key.replace(/\D/g, ""), 10);
  return Number.isFinite(n) ? Math.min(n, 4) : 4;
}
export const speakerColors = ["var(--accent-lavender)", "var(--accent-sky)", "var(--accent-sand)", "var(--accent-rose)", "var(--muted)"];

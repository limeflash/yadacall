/** Pure view logic for the Models screen: search, filters, sorting and grouping. No UI, no I/O. */
import type { ModelEntry, ModelKind, ModelLang, Verdict } from "./api";
import { plural, t } from "./i18n/index.svelte";

export type Tab = "all" | ModelKind;
export type FilterKey = "ru" | "multi" | "en" | "fits" | "installed";

export const TAB_ORDER: Tab[] = ["all", "asr", "llm", "voice", "embed"];
/** Section order inside the "all" tab. */
export const KIND_ORDER: ModelKind[] = ["asr", "llm", "voice", "embed"];
export const FILTER_ORDER: FilterKey[] = ["ru", "multi", "en", "fits", "installed"];

export interface Group {
  id: string;
  /** Russian source string, run through `t()` by the view. */
  title: string;
  sub?: string;
  items: ModelEntry[];
  /** Folded away until opened (the "won't run" group). */
  foldable?: boolean;
  tone?: "ok" | "warn" | "bad";
}

/** Lower-case and fold ё into е, one UTF-16 unit for one unit so match offsets stay valid for the original text. */
export function norm(s: string): string {
  let out = "";
  for (const ch of s) {
    const l = ch.toLowerCase();
    out += l.length === ch.length ? (l === "ё" ? "е" : l) : ch;
  }
  return out;
}

const LANG_WORDS: Record<ModelLang, string> = {
  ru: "русский русская русское русскоязычные русском russian ru",
  en: "английский английская english en",
  multilingual: "мультиязычные многоязычные мульти multilingual",
};
const TAG_WORDS: Record<string, string> = {
  "ru-tuned": "русское дообучение ru-tuned russian fine-tune",
  moe: "moe mixture experts быстрая для размера",
  "english-only": "только английский english-only",
  "reasoning-off": "без рассуждений размышления reasoning",
  streaming: "потоковая streaming",
  tiny: "крошечная лёгкая tiny",
};
/** Latin / alternative spellings people type for vendors whose display name is Cyrillic or compound. */
const VENDOR_WORDS: Record<string, string> = {
  "Сбер": "sber sberbank gigachat",
  "Яндекс": "yandex",
  "Т-Банк": "tbank t-bank tinkoff тинькофф т-банк",
  "Qwen · Alibaba": "qwen alibaba алибаба",
};
const KIND_WORDS: Record<ModelKind, string> = {
  asr: "расшифровка транскрипция speech whisper asr",
  llm: "итоги llm summary",
  voice: "голоса speaker diarization",
  embed: "поиск embeddings",
};

/** Everything a query can hit, already normalised. `blurb` is the shown (translated) one. */
export function haystack(m: ModelEntry, blurb: string): string {
  return norm([
    m.name, m.id, m.vendor, VENDOR_WORDS[m.vendor] ?? "", m.family, m.license, KIND_WORDS[m.kind],
    ...m.languages.map((l) => LANG_WORDS[l]), ...m.tags.map((x) => TAG_WORDS[x] ?? x), blurb,
  ].join(" "));
}

/** Query → normalised terms; every term has to be found (AND). */
export const terms = (q: string): string[] => norm(q).split(/\s+/).filter(Boolean);

export function matchesTerms(hay: string, ts: string[]): boolean {
  return ts.every((x) => hay.includes(x));
}

export const isFit = (v: Verdict) => v === "great" || v === "ok";

/** Language chips are a union (any selected language); the other chips narrow further. */
export function langMatches(m: ModelEntry, f: ReadonlySet<FilterKey>): boolean {
  if (!f.has("ru") && !f.has("multi") && !f.has("en")) return true;
  const multi = m.languages.includes("multilingual");
  const engOnly = m.tags.includes("english-only");
  return (f.has("ru") && (m.languages.includes("ru") || (multi && !engOnly)))
    || (f.has("multi") && multi)
    || (f.has("en") && (m.languages.includes("en") || multi));
}

/** One chip on its own, regardless of which others are on (used for per-chip counts). */
export function chipMatches(key: FilterKey, m: ModelEntry): boolean {
  return key === "fits" ? isFit(m.verdict) : key === "installed" ? m.installed : langMatches(m, new Set([key]));
}

/** Applies the chips except those in `skip`. */
export function applyFilters(list: ModelEntry[], f: ReadonlySet<FilterKey>, skip: ReadonlySet<FilterKey> = new Set()): ModelEntry[] {
  const langOn = !skip.has("ru") && !skip.has("multi") && !skip.has("en");
  return list.filter((m) =>
    (!langOn || langMatches(m, f))
    && (skip.has("fits") || !f.has("fits") || isFit(m.verdict))
    && (skip.has("installed") || !f.has("installed") || m.installed));
}

const VERDICT_RANK: Record<Verdict, number> = { great: 0, ok: 1, slow: 2, wontRun: 3 };

/** Installed + primary, recommended, installed, then verdict, quality (desc), size (asc). */
export function compareModels(a: ModelEntry, b: ModelEntry): number {
  const tier = (m: ModelEntry) => (m.installed && m.active ? 0 : m.recommended ? 1 : m.installed ? 2 : 3);
  return tier(a) - tier(b)
    || VERDICT_RANK[a.verdict] - VERDICT_RANK[b.verdict]
    || (b.qualityRank ?? 0) - (a.qualityRank ?? 0)
    || a.sizeBytes - b.sizeBytes;
}

export const sorted = (list: ModelEntry[]) => [...list].sort(compareModels);

/** Speech models are grouped by the language they are for. */
export function asrLang(m: ModelEntry): "multi" | "ru" | "en" {
  return m.languages.includes("multilingual") ? "multi" : m.languages.includes("ru") ? "ru" : "en";
}

/** Splits one kind's (already sorted) models into the groups its tab shows. Empty groups are dropped. */
export function groupsFor(kind: ModelKind, list: ModelEntry[]): Group[] {
  let gs: Group[];
  if (kind === "llm") {
    gs = [
      { id: "fit", title: "Подходят этому компьютеру", tone: "ok", items: list.filter((m) => isFit(m.verdict)) },
      { id: "slow", title: "Пойдут медленно", tone: "warn", items: list.filter((m) => m.verdict === "slow") },
      { id: "wont", title: "Не потянет", tone: "bad", foldable: true, items: list.filter((m) => m.verdict === "wontRun") },
    ];
  } else if (kind === "asr") {
    gs = [
      { id: "ru", title: "Русский", items: list.filter((m) => asrLang(m) === "ru") },
      { id: "multi", title: "Мультиязычные", items: list.filter((m) => asrLang(m) === "multi") },
      { id: "en", title: "Английский", items: list.filter((m) => asrLang(m) === "en") },
    ];
  } else {
    gs = [{ id: "one", title: "", items: list }];
  }
  return gs.filter((g) => g.items.length);
}

/** Runs of `[start, end)` ranges in `text` covered by any term (merged, in order). */
export function highlightRanges(text: string, ts: string[]): [number, number][] {
  const n = norm(text);
  if (!ts.length || n.length !== text.length) return [];
  const raw: [number, number][] = [];
  for (const t of ts) {
    for (let i = n.indexOf(t); i !== -1 && t; i = n.indexOf(t, i + t.length)) raw.push([i, i + t.length]);
  }
  raw.sort((a, b) => a[0] - b[0]);
  const out: [number, number][] = [];
  for (const r of raw) {
    const last = out[out.length - 1];
    if (last && r[0] <= last[1]) last[1] = Math.max(last[1], r[1]);
    else out.push([r[0], r[1]]);
  }
  return out;
}

/** Splits a name into plain / matched pieces for rendering. */
export function highlight(text: string, ts: string[]): { s: string; hit: boolean }[] {
  const rs = highlightRanges(text, ts);
  if (!rs.length) return [{ s: text, hit: false }];
  const out: { s: string; hit: boolean }[] = [];
  let at = 0;
  for (const [a, b] of rs) {
    if (a > at) out.push({ s: text.slice(at, a), hit: false });
    out.push({ s: text.slice(a, b), hit: true });
    at = b;
  }
  if (at < text.length) out.push({ s: text.slice(at), hit: false });
  return out;
}

const TONES = ["lavender", "sky", "sand", "sage", "mint", "rose"] as const;
/** Deterministic pastel per vendor. */
export function vendorTone(vendor: string): (typeof TONES)[number] {
  let h = 7;
  for (const ch of vendor) h = (h * 31 + ch.codePointAt(0)!) >>> 0;
  return TONES[h % TONES.length];
}

/** 1–2 letters for the vendor monogram: "Сбер" → "С", "Т-Банк" → "ТБ", "Qwen · Alibaba" → "QA". */
export function monogram(vendor: string): string {
  const parts = vendor.split(/[\s·\-]+/).filter(Boolean);
  if (!parts.length) return "?";
  if (/^\d/.test(parts[0])) return parts[0].slice(0, 2).toUpperCase();
  return parts.slice(0, 2).map((p) => [...p][0].toUpperCase()).join("");
}

/** Trailing precision / quantisation token of a catalog name: "int8", "fp16", "Q4_0", "Q4_K_M", "q4f16". */
const QUANT_TAIL = /\s+(int[248]|fp(?:16|32)|bf16|q\d+f\d+|q\d+(?:_[a-z0-9]+)*)$/i;

/** "Gemma 4 E2B Q4_0" → { title: "Gemma 4 E2B", quant: "Q4_0" }. Names without such a tail come back unchanged. */
export function splitName(name: string): { title: string; quant: string } {
  const m = QUANT_TAIL.exec(name);
  return m ? { title: name.slice(0, m.index), quant: m[1] } : { title: name, quant: "" };
}
/** The name as shown on a card: no quantisation noise. */
export const displayName = (m: Pick<ModelEntry, "name">) => splitName(m.name).title;
export const quantOf = (m: Pick<ModelEntry, "name">) => splitName(m.name).quant;
/** True when another model of the same kind would show the same clean name (two quantisations of one model): then the quantisation is worth showing. */
export const sharesName = (m: ModelEntry, all: ModelEntry[]) => all.some((x) => x.id !== m.id && x.kind === m.kind && displayName(x) === displayName(m));

/** Which language group a recommendation belongs to: speech models by language, summaries by machine, the rest on their own. */
export type RecScope = "ru" | "multi" | "en" | "machine" | "kind";
export const recScope = (m: ModelEntry): RecScope => (m.kind === "asr" ? asrLang(m) : m.kind === "llm" ? "machine" : "kind");
/** Models competing for the same recommendation badge. A badge with no rival says nothing, so the UI hides it. */
export const recRivals = (m: ModelEntry, all: ModelEntry[]) => all.filter((x) => x.kind === m.kind && recScope(x) === recScope(m)).length;

/** Languages as plain text: "русский", "русский и английский", "25 языков". */
export function langText(m: ModelEntry): string {
  if (m.tags.includes("english-only")) return t("английский");
  if (m.languages.includes("multilingual")) return m.languageCount ? `${m.languageCount} ${plural(m.languageCount, ["язык", "языка", "языков"], ["language", "languages"])}` : t("много языков");
  const ru = m.languages.includes("ru"), en = m.languages.includes("en");
  return ru && en ? t("русский и английский") : ru ? t("русский") : en ? t("английский") : "";
}

/** "4B", "0,6B", "118M"; the decimal separator follows the UI locale. */
export function paramsLabel(n: number, fmt: (x: number) => string): string {
  return n >= 0.5e9 ? fmt(n / 1e9) + "B" : Math.round(n / 1e6) + "M";
}

export type SpokenLang = "ru" | "multi" | "en";

/**
 * The model onboarding preselects: the best-quality one that runs well here and speaks the chosen language
 * (the backend's `recommended` pick wins when it qualifies). Falls back to any model that fits.
 */
export function recommendFor(models: ModelEntry[], kind: ModelKind, lang: SpokenLang): ModelEntry | undefined {
  const fits = models.filter((m) => m.kind === kind && isFit(m.verdict));
  const speaks = (m: ModelEntry) => {
    const multi = m.languages.includes("multilingual");
    return lang === "ru" ? m.languages.includes("ru") || (multi && !m.tags.includes("english-only")) : lang === "en" ? m.languages.includes("en") || multi : multi;
  };
  const pool = fits.filter(speaks);
  const use = pool.length ? pool : fits;
  const best = (a: ModelEntry, b: ModelEntry) => (b.qualityRank ?? 0) - (a.qualityRank ?? 0) || a.sizeBytes - b.sizeBytes;
  return use.filter((m) => m.recommended).sort(best)[0] ?? [...use].sort(best)[0];
}

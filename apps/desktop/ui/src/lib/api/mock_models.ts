/**
 * Mock model catalog for the browser preview: the seed models from `demo_data.json` get the richer metadata the
 * Models screen groups and searches by, and the upcoming catalog entries are appended so the screen can be judged
 * against a realistic ~30-model list. Verdicts model a 16 GB M1 Pro (the demo hardware).
 */
import type { ModelEntry, ModelKind, ModelLang, Verdict } from "./types";

type Meta = Pick<ModelEntry, "vendor" | "family" | "languages" | "tags" | "license"> & Partial<Pick<ModelEntry, "params" | "activeParams" | "licenseUrl" | "licenseCustom" | "homepage" | "recommended" | "qualityRank">>;

const APACHE = "Apache-2.0";

/** Metadata for the models that already live in `demo_data.json`. */
const SEED_META: Record<string, Meta> = {
  "gigaam-v3-rnnt": { vendor: "Сбер", family: "GigaAM", languages: ["ru"], tags: [], params: 240e6, license: "MIT", recommended: true, qualityRank: 90, homepage: "https://github.com/salute-developers/GigaAM" },
  "parakeet-tdt-v3": { vendor: "NVIDIA", family: "Parakeet", languages: ["multilingual"], tags: [], params: 600e6, license: "CC-BY-4.0", recommended: true, qualityRank: 80 },
  "voice-embed": { vendor: "3D-Speaker", family: "CAM++", languages: ["multilingual"], tags: ["tiny"], license: APACHE, qualityRank: 50 },
  "text-embed-small": { vendor: "intfloat", family: "E5", languages: ["multilingual"], tags: ["tiny"], params: 118e6, license: "MIT", recommended: true, qualityRank: 50 },
  "gemma-4-e4b": { vendor: "Google", family: "Gemma 4", languages: ["multilingual"], tags: [], params: 4e9, license: APACHE, qualityRank: 60 },
  "gemma-4-e2b": { vendor: "Google", family: "Gemma 4", languages: ["multilingual"], tags: [], params: 2e9, license: APACHE, qualityRank: 40 },
};

interface Extra {
  id: string; kind: ModelKind; name: string; sizeBytes: number; blurb: string; verdict: Verdict; estimate: string;
  meta: Meta; installed?: boolean; active?: boolean; benchmarkable?: boolean;
}

const QWEN = { vendor: "Qwen · Alibaba", family: "Qwen3.5", languages: ["multilingual"] as ModelLang[], license: APACHE };

const EXTRA: Extra[] = [
  {
    id: "parakeet-tdt-v2", kind: "asr", name: "Parakeet TDT 0.6B v2", sizeBytes: 661e6, verdict: "great", estimate: "≈ 30× быстрее реального времени",
    blurb: "самый точный для английской речи",
    meta: { vendor: "NVIDIA", family: "Parakeet", languages: ["en"], tags: ["english-only"], params: 600e6, license: "CC-BY-4.0", qualityRank: 70 },
  },
  {
    id: "silero-vad", kind: "voice", name: "silero-vad", sizeBytes: 2e6, verdict: "great", estimate: "работает на любом компьютере", installed: true, active: true, benchmarkable: false,
    blurb: "находит речь в записи — без неё запись не работает",
    meta: { vendor: "Silero", family: "Silero VAD", languages: ["multilingual"], tags: ["required", "tiny"], license: "MIT", qualityRank: 10 },
  },
  {
    id: "qwen3.5-2b", kind: "llm", name: "Qwen3.5 2B", sizeBytes: 1280835840, verdict: "great", estimate: "≈ 52 ток/с · 2,6 из 16 ГБ",
    blurb: "очень лёгкая, подходит для коротких итогов",
    meta: { ...QWEN, tags: [], params: 2e9, qualityRank: 35 },
  },
  {
    id: "qwen3.5-4b", kind: "llm", name: "Qwen3.5 4B", sizeBytes: 2740937888, verdict: "ok", estimate: "≈ 26 ток/с · 4,9 из 16 ГБ", installed: true,
    blurb: "хороший баланс скорости и качества",
    meta: { ...QWEN, tags: ["reasoning-off"], params: 4e9, qualityRank: 55 },
  },
  {
    id: "qwen3.5-9b", kind: "llm", name: "Qwen3.5 9B", sizeBytes: 5680522464, verdict: "slow", estimate: "≈ 9 ток/с · 9,8 из 16 ГБ",
    blurb: "заметно умнее, но тяжелее для этого компьютера",
    meta: { ...QWEN, tags: ["reasoning-off"], params: 9e9, qualityRank: 70 },
  },
  {
    id: "gigachat-3.1-lightning", kind: "llm", name: "GigaChat 3.1 Lightning", sizeBytes: 6474702976, verdict: "great", estimate: "≈ 44 ток/с · 7,4 из 16 ГБ",
    blurb: "сильный русский, быстрая благодаря MoE",
    meta: { vendor: "Сбер", family: "GigaChat", languages: ["ru", "en"], tags: ["moe", "ru-tuned"], params: 10e9, activeParams: 1.8e9, license: "MIT", recommended: true, qualityRank: 72 },
  },
  {
    id: "t-lite-it-2.1", kind: "llm", name: "T-lite-it-2.1", sizeBytes: 5027750240, verdict: "ok", estimate: "≈ 17 ток/с · 6,2 из 16 ГБ",
    blurb: "дообучена на русском, уверенно держит факты",
    meta: { vendor: "Т-Банк", family: "T-lite", languages: ["ru", "en"], tags: ["ru-tuned"], params: 8e9, license: APACHE, qualityRank: 66 },
  },
  {
    id: "yandexgpt-5-lite-8b", kind: "llm", name: "YandexGPT 5 Lite 8B", sizeBytes: 4920741184, verdict: "slow", estimate: "≈ 11 ток/с · 6,0 из 16 ГБ",
    blurb: "русская модель Яндекса с собственной лицензией",
    meta: {
      vendor: "Яндекс", family: "YandexGPT", languages: ["ru", "en"], tags: ["ru-tuned"], params: 8e9, license: "YandexGPT-5-Lite", licenseCustom: true,
      licenseUrl: "https://huggingface.co/yandex/YandexGPT-5-Lite-8B-instruct/blob/main/LICENSE", qualityRank: 64,
    },
  },
  {
    id: "qvikhr-3-4b", kind: "llm", name: "QVikhr-3 4B", sizeBytes: 2497281088, verdict: "great", estimate: "≈ 29 ток/с · 4,5 из 16 ГБ",
    blurb: "Qwen3, дообученная на русском",
    meta: { vendor: "Vikhr", family: "QVikhr", languages: ["ru", "en"], tags: ["ru-tuned", "reasoning-off"], params: 4e9, license: APACHE, qualityRank: 50 },
  },
  {
    id: "qvikhr-3-1.7b", kind: "llm", name: "QVikhr-3 1.7B", sizeBytes: 1107408608, verdict: "great", estimate: "≈ 61 ток/с · 2,2 из 16 ГБ",
    blurb: "самая маленькая с русским дообучением",
    meta: { vendor: "Vikhr", family: "QVikhr", languages: ["ru", "en"], tags: ["ru-tuned"], params: 1.7e9, license: APACHE, qualityRank: 30 },
  },
  {
    id: "ruadaptqwen3-4b", kind: "llm", name: "RuadaptQwen3 4B", sizeBytes: 2486507648, verdict: "great", estimate: "≈ 31 ток/с · 4,4 из 16 ГБ",
    blurb: "Qwen3 с русским токенизатором, быстрее на русском тексте",
    meta: { vendor: "RefalMachine", family: "Ruadapt", languages: ["ru", "en"], tags: ["ru-tuned"], params: 4e9, license: APACHE, qualityRank: 48 },
  },
  {
    id: "gemma-4-26b-a4b", kind: "llm", name: "gemma-4-26b-a4b", sizeBytes: 16800000000, verdict: "wontRun", estimate: "не хватит памяти: нужно 19 ГБ из 16",
    blurb: "большая MoE-модель, максимум качества",
    meta: { vendor: "Google", family: "Gemma 4", languages: ["multilingual"], tags: ["moe"], params: 26e9, activeParams: 3.8e9, license: APACHE, qualityRank: 85 },
  },
  {
    id: "gemma-4-31b", kind: "llm", name: "gemma-4-31b", sizeBytes: 19600000000, verdict: "wontRun", estimate: "не хватит памяти: нужно 23 ГБ из 16",
    blurb: "самая сильная в семействе",
    meta: { vendor: "Google", family: "Gemma 4", languages: ["multilingual"], tags: [], params: 31e9, license: APACHE, qualityRank: 90 },
  },
];

/** Seed models + metadata, plus the upcoming catalog. */
export function mockCatalog(seed: ModelEntry[]): ModelEntry[] {
  const out: ModelEntry[] = [];
  for (const m of seed) {
    if (m.id === "gemma-4-27b") continue; // replaced by the 26B-A4B / 31B pair below
    const meta = SEED_META[m.id];
    if (!meta) continue;
    // The preview shows an on-device summary model in use, so the "primary" badge and "make primary" both appear.
    const base = m.id === "gemma-4-e2b" ? { ...m, installed: true, active: true } : m;
    out.push({ ...base, ...meta });
  }
  for (const e of EXTRA) {
    out.push({
      id: e.id, kind: e.kind, name: e.name, sizeBytes: e.sizeBytes, blurb: e.blurb, installed: e.installed ?? false, active: e.active ?? false,
      verdict: e.verdict, estimate: e.estimate, benchmarkable: e.benchmarkable, ...e.meta,
    });
  }
  return out;
}

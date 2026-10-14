/**
 * Mock model catalog for the browser preview. Mirrors `catalog/models.json` (ids, names with their quantisation suffix,
 * vendors, licences, tags, quality ranks, descriptions) so the Models screen is judged against the real list; verdicts,
 * estimates and the `recommended` flags model a 16 GB M1 Pro (the demo hardware).
 *
 * Recommendations follow the backend contract: one per language group for speech (ru / multilingual / en) and one for summaries.
 * Parakeet Ultra beats v2 on English calls too, so v2 carries no badge here.
 */
import type { ModelEntry, ModelKind, ModelLang, Verdict } from "./types";

const APACHE = "Apache-2.0";
const SPDX = (id: string) => `https://spdx.org/licenses/${id}.html`;
const CC_BY = "https://creativecommons.org/licenses/by/4.0/";

interface Row {
  id: string; kind: ModelKind; name: string; blurb: string; sizeBytes: number; verdict: Verdict; estimate: string;
  vendor: string; family: string; languages: ModelLang[]; tags?: string[]; license: string; licenseUrl: string; licenseCustom?: boolean; homepage: string;
  params?: number; activeParams?: number; qualityRank: number;
  installed?: boolean; active?: boolean; recommended?: boolean; benchmarkable?: boolean; languageCount?: number;
  download?: ModelEntry["download"];
}

const asr = (r: Omit<Row, "kind">): Row => ({ kind: "asr", ...r });
const llm = (r: Omit<Row, "kind">): Row => ({ kind: "llm", ...r });

const HF = "https://huggingface.co/";
const GIGAAM_V3 = { vendor: "Сбер", family: "GigaAM", license: "MIT", licenseUrl: SPDX("MIT"), homepage: HF + "istupakov/gigaam-v3-onnx" };
const PARAKEET = { vendor: "NVIDIA", family: "Parakeet", license: "CC-BY-4.0", licenseUrl: CC_BY, params: 600e6 };
const GEMMA = { vendor: "Google", family: "Gemma 4", languages: ["multilingual"] as ModelLang[], license: APACHE, licenseUrl: SPDX(APACHE) };
const QWEN = { vendor: "Qwen · Alibaba", family: "Qwen3.5", languages: ["multilingual"] as ModelLang[], tags: ["reasoning-off"], license: APACHE };
const RU_EN = ["ru", "en"] as ModelLang[];
const GEMMA_DESC = "Локальная языковая модель для суммаризации встреч.";

const ROWS: Row[] = [
  // ---- speech ---------------------------------------------------------------------------------------------------
  asr({
    id: "gigaam-v3-e2e-rnnt-int8", name: "GigaAM v3 RNNT int8", blurb: "Распознавание русской речи, рекомендовано для русского языка.", sizeBytes: 226430792, verdict: "great", estimate: "≈ 54× реального времени",
    ...GIGAAM_V3, languages: ["ru"], tags: ["tiny"], params: 200e6, qualityRank: 100, installed: true, active: true, recommended: true,
  }),
  asr({
    id: "gigaam-v3-e2e-ctc-int8", name: "GigaAM v3 CTC int8", blurb: "CTC-вариант GigaAM v3 для русской речи.", sizeBytes: 224895354, verdict: "great", estimate: "≈ 61× реального времени",
    ...GIGAAM_V3, languages: ["ru"], tags: ["tiny"], params: 200e6, qualityRank: 86,
  }),
  asr({
    id: "gigaam-multilingual-ctc-int8", name: "GigaAM Multilingual CTC int8", blurb: "Мультиязычная CTC-модель для русского, английского, казахского, кыргызского и узбекского. Пишет без знаков препинания и заглавных букв; на русском немного уступает GigaAM v3.",
    sizeBytes: 224762597, verdict: "great", estimate: "≈ 58× реального времени",
    vendor: "Сбер", family: "GigaAM", languages: RU_EN, tags: ["tiny"], license: "MIT", licenseUrl: SPDX("MIT"), homepage: HF + "ai-sage/GigaAM-Multilingual", params: 220e6, qualityRank: 84,
  }),
  asr({
    id: "parakeet-tdt-0.6b-v3-ultra-int8", name: "Parakeet TDT 0.6B v3 Ultra int8",
    blurb: "В наших тестах: WER RU 17,5% против 22,3%; EN Earnings-22 15,8% против 21,2%; LibriSpeech 2,4% против 3,4%; путаница некириллических языков 2,5% против 5,2% у v3.",
    sizeBytes: 667974518, verdict: "ok", estimate: "≈ 13× реального времени",
    ...PARAKEET, vendor: "Moondream", languages: ["multilingual"], homepage: HF + "moondream/parakeet-ultra", qualityRank: 96, recommended: true, languageCount: 25,
  }),
  asr({
    id: "parakeet-tdt-0.6b-v3-int8", name: "Parakeet TDT 0.6B v3 int8", blurb: "25 языков; запасной вариант для старых установок (Ultra точнее по замерам).", sizeBytes: 670619706, verdict: "ok", estimate: "≈ 13× реального времени",
    ...PARAKEET, languages: ["multilingual"], homepage: HF + "istupakov/parakeet-tdt-0.6b-v3-onnx", qualityRank: 90, languageCount: 25,
    download: { state: { state: "running" }, doneBytes: 255e6, totalBytes: 670619706, bytesPerSec: 5.4e6, etaSecs: 77 },
  }),
  asr({
    id: "parakeet-tdt-0.6b-v2-int8", name: "Parakeet TDT 0.6B v2 int8", blurb: "Англоязычная модель распознавания речи; результат Open ASR лучше v3 по отчёту авторов. Только английский. На английских созвонах уступает Parakeet Ultra (21,5% ошибок против 15,8%).",
    sizeBytes: 661331448, verdict: "ok", estimate: "≈ 13× реального времени",
    ...PARAKEET, languages: ["en"], tags: ["english-only"], homepage: HF + "istupakov/parakeet-tdt-0.6b-v2-onnx", qualityRank: 88,
  }),
  // ---- voices / search ------------------------------------------------------------------------------------------
  {
    id: "silero-vad-v5", kind: "voice", name: "Silero VAD v5", blurb: "Автоматически устанавливаемое определение речи.", sizeBytes: 2327524, verdict: "great", estimate: "работает на любом компьютере", installed: true, active: true, benchmarkable: false,
    vendor: "Silero", family: "Silero VAD", languages: ["multilingual"], tags: ["required", "tiny"], license: "MIT", licenseUrl: SPDX("MIT"), homepage: "https://github.com/snakers4/silero-vad", params: 1e6, qualityRank: 50,
  },
  {
    id: "speaker-campplus-zh-en-16k", kind: "voice", name: "3D-Speaker CAM++", blurb: "Эмбеддинги голосов для разделения участников группового звонка.", sizeBytes: 28281164, verdict: "great", estimate: "≈ 200× реального времени", installed: true, active: true,
    vendor: "3D-Speaker", family: "CAM++", languages: ["multilingual"], tags: ["tiny"], license: APACHE, licenseUrl: SPDX(APACHE), homepage: "https://github.com/k2-fsa/sherpa-onnx", params: 20e6, qualityRank: 75,
  },
  {
    id: "multilingual-e5-small-int8", kind: "embed", name: "Multilingual E5 small int8", blurb: "Лёгкая мультиязычная модель семантического поиска.", sizeBytes: 135137323, verdict: "great", estimate: "≈ 900 абзацев в секунду", installed: true, active: true,
    vendor: "intfloat", family: "E5", languages: ["multilingual"], tags: ["tiny"], license: "MIT", licenseUrl: SPDX("MIT"), homepage: HF + "intfloat/multilingual-e5-small", params: 118e6, qualityRank: 90,
  },
  {
    id: "embeddinggemma-300m-q4f16", kind: "embed", name: "EmbeddingGemma 300M q4f16", blurb: "Качественные мультиязычные эмбеддинги; применяются Gemma Terms.", sizeBytes: 202286378, verdict: "great", estimate: "≈ 410 абзацев в секунду",
    vendor: "Google", family: "EmbeddingGemma", languages: ["multilingual"], tags: ["tiny"], license: "Gemma Terms", licenseUrl: "https://ai.google.dev/gemma/terms", licenseCustom: true, homepage: HF + "google/embeddinggemma-300m", params: 300e6, qualityRank: 89,
  },
  {
    id: "qwen3-embedding-0.6b-int8", kind: "embed", name: "Qwen3 Embedding 0.6B int8", blurb: "Самая качественная модель эмбеддингов в наборе.", sizeBytes: 624951244, verdict: "great", estimate: "≈ 190 абзацев в секунду", recommended: true,
    vendor: "Qwen · Alibaba", family: "Qwen3 Embedding", languages: ["multilingual"], tags: [], license: APACHE, licenseUrl: SPDX(APACHE), homepage: HF + "Qwen/Qwen3-Embedding-0.6B", params: 600e6, qualityRank: 94,
  },
  // ---- summaries ------------------------------------------------------------------------------------------------
  llm({
    id: "gemma-4-e2b-q4-0", name: "Gemma 4 E2B Q4_0", blurb: GEMMA_DESC + " Наш тест итогов: 83 из 100 на тестовых встречах, 55 на настоящем созвоне.", sizeBytes: 2841481184, verdict: "great", estimate: "≈ 48 ток/с · 3,5 из 16 ГБ",
    ...GEMMA, tags: [], homepage: HF + "ggml-org/gemma-4-E2B-it-GGUF", params: 5.1e9, activeParams: 2.3e9, qualityRank: 88, installed: true, active: true,
  }),
  llm({
    id: "gemma-4-e2b-q8-0", name: "Gemma 4 E2B Q8_0", blurb: GEMMA_DESC, sizeBytes: 4967497152, verdict: "ok", estimate: "≈ 31 ток/с · 5,6 из 16 ГБ",
    ...GEMMA, tags: [], homepage: HF + "ggml-org/gemma-4-E2B-it-GGUF", params: 5.1e9, activeParams: 2.3e9, qualityRank: 89,
  }),
  llm({
    id: "gemma-4-e4b-q4-0", name: "Gemma 4 E4B Q4_0", blurb: GEMMA_DESC, sizeBytes: 4590807392, verdict: "ok", estimate: "≈ 25 ток/с · 5,2 из 16 ГБ",
    ...GEMMA, tags: [], homepage: HF + "ggml-org/gemma-4-E4B-it-GGUF", params: 8e9, activeParams: 4.5e9, qualityRank: 90,
    download: { state: { state: "failed", message: "Связь прервалась", resumable: true }, doneBytes: 1.6e9, totalBytes: 4590807392, bytesPerSec: 0 },
  }),
  llm({
    id: "gemma-4-e4b-q8-0", name: "Gemma 4 E4B Q8_0", blurb: GEMMA_DESC, sizeBytes: 8031242688, verdict: "slow", estimate: "≈ 12 ток/с · 8,9 из 16 ГБ",
    ...GEMMA, tags: [], homepage: HF + "ggml-org/gemma-4-E4B-it-GGUF", params: 8e9, activeParams: 4.5e9, qualityRank: 91,
  }),
  llm({
    id: "gemma-4-12b-q4-0", name: "Gemma 4 12B Q4_0", blurb: GEMMA_DESC, sizeBytes: 7219673216, verdict: "slow", estimate: "≈ 9 ток/с · 8,1 из 16 ГБ",
    ...GEMMA, tags: [], homepage: HF + "ggml-org/gemma-4-12B-it-GGUF", params: 12e9, qualityRank: 94,
  }),
  llm({
    id: "gemma-4-26b-a4b-q4-0", name: "Gemma 4 26B-A4B Q4_0", blurb: GEMMA_DESC, sizeBytes: 14618145824, verdict: "wontRun", estimate: "не хватит памяти: нужно 17 ГБ из 16",
    ...GEMMA, tags: ["moe"], homepage: HF + "ggml-org/gemma-4-26B-A4B-it-GGUF", params: 25.2e9, activeParams: 3.8e9, qualityRank: 96,
  }),
  llm({
    id: "gemma-4-31b-q4-0", name: "Gemma 4 31B Q4_0", blurb: GEMMA_DESC, sizeBytes: 17992313088, verdict: "wontRun", estimate: "не хватит памяти: нужно 21 ГБ из 16",
    ...GEMMA, tags: [], homepage: HF + "ggml-org/gemma-4-31B-it-GGUF", params: 31e9, qualityRank: 100,
  }),
  llm({
    id: "qwen3.5-2b-q4-k-m", name: "Qwen3.5 2B Q4_K_M", blurb: "Компактная мультиязычная модель для черновых сводок и простых вопросов.", sizeBytes: 1280835840, verdict: "great", estimate: "≈ 52 ток/с · 2,6 из 16 ГБ",
    ...QWEN, licenseUrl: HF + "Qwen/Qwen3.5-2B/blob/main/LICENSE", homepage: HF + "unsloth/Qwen3.5-2B-GGUF", params: 1.9e9, qualityRank: 70,
  }),
  llm({
    id: "qwen3.5-4b-q4-k-m", name: "Qwen3.5 4B Q4_K_M", blurb: "Сбалансированная мультиязычная модель для подробных сводок и вопросов по встречам. Наш тест итогов: 82 из 100 на тестовых встречах и 83 на настоящем созвоне — лучшая из проверенных.",
    sizeBytes: 2740937888, verdict: "ok", estimate: "≈ 26 ток/с · 4,9 из 16 ГБ",
    ...QWEN, licenseUrl: HF + "Qwen/Qwen3.5-4B/blob/main/LICENSE", homepage: HF + "unsloth/Qwen3.5-4B-GGUF", params: 4.2e9, qualityRank: 92, installed: true, recommended: true,
  }),
  llm({
    id: "qwen3.5-9b-q4-k-m", name: "Qwen3.5 9B Q4_K_M", blurb: "Более ёмкая мультиязычная модель для длинных сводок и сложных вопросов.", sizeBytes: 5680522464, verdict: "slow", estimate: "≈ 9 ток/с · 9,8 из 16 ГБ",
    ...QWEN, licenseUrl: HF + "Qwen/Qwen3.5-9B/blob/main/LICENSE", homepage: HF + "unsloth/Qwen3.5-9B-GGUF", params: 9e9, qualityRank: 93,
  }),
  llm({
    id: "gigachat3.1-lightning-q4-k-m", name: "GigaChat 3.1 Lightning Q4_K_M", blurb: "Русско-английская MoE-модель для быстрых кратких сводок. Наш тест итогов: 80 из 100 (проверена частично). Медленно читает длинные встречи.",
    sizeBytes: 6474702976, verdict: "great", estimate: "≈ 44 ток/с · 7,4 из 16 ГБ",
    vendor: "Сбер", family: "GigaChat", languages: RU_EN, tags: ["moe"], license: "MIT", licenseUrl: SPDX("MIT"), homepage: HF + "ai-sage/GigaChat3.1-10B-A1.8B-GGUF", params: 10.7e9, activeParams: 1.8e9, qualityRank: 86,
  }),
  llm({
    id: "t-lite-it-2.1-q4-k-m", name: "T-lite-it-2.1 Q4_K_M", blurb: "По данным Т-Банка, показывает сильные результаты на русскоязычных тестах. Наш тест итогов: 72 из 100 на тестовых встречах, 35 на настоящем созвоне; путает дни недели.",
    sizeBytes: 5027750240, verdict: "ok", estimate: "≈ 17 ток/с · 6,2 из 16 ГБ",
    vendor: "Т-Банк", family: "T-lite", languages: ["ru"], tags: ["ru-tuned", "reasoning-off"], license: APACHE, licenseUrl: SPDX(APACHE), homepage: HF + "t-tech/T-lite-it-2.1-GGUF", params: 8.2e9, qualityRank: 80,
  }),
  llm({
    id: "yandexgpt-5-lite-8b-q4-k-m", name: "YandexGPT 5 Lite 8B Q4_K_M", blurb: "Русско-английская instruct-модель для сводок и вопросов по встречам.", sizeBytes: 4920741184, verdict: "slow", estimate: "≈ 11 ток/с · 6,0 из 16 ГБ",
    vendor: "Яндекс", family: "YandexGPT", languages: RU_EN, tags: [], license: "YandexGPT-5-Lite", licenseUrl: HF + "yandex/YandexGPT-5-Lite-8B-instruct/blob/main/LICENSE", licenseCustom: true,
    homepage: HF + "yandex/YandexGPT-5-Lite-8B-instruct-GGUF", params: 8e9, qualityRank: 78,
  }),
  llm({
    id: "qvikhr-3-4b-q4-k-m", name: "QVikhr-3 4B Q4_K_M", blurb: "Русско-английская модель с сильным результатом на Ru Arena; отключаем рассуждения для сводок. Наш тест итогов: 68 из 100.", sizeBytes: 2497281088, verdict: "great", estimate: "≈ 29 ток/с · 4,5 из 16 ГБ",
    vendor: "Vikhr", family: "QVikhr", languages: RU_EN, tags: ["ru-tuned", "reasoning-off"], license: APACHE, licenseUrl: SPDX(APACHE), homepage: HF + "Vikhrmodels/QVikhr-3-4B-Instruction-GGUF", params: 4e9, qualityRank: 72,
  }),
  llm({
    id: "qvikhr-3-1.7b-q4-k-m", name: "QVikhr-3 1.7B Q4_K_M", blurb: "Компактная русско-английская instruct-модель без режима рассуждений.", sizeBytes: 1107408608, verdict: "great", estimate: "≈ 61 ток/с · 2,2 из 16 ГБ",
    vendor: "Vikhr", family: "QVikhr", languages: RU_EN, tags: ["ru-tuned", "reasoning-off"], license: APACHE, licenseUrl: SPDX(APACHE), homepage: HF + "Vikhrmodels/QVikhr-3-1.7B-Instruction-noreasoning-GGUF", params: 1.7e9, qualityRank: 60,
  }),
  llm({
    id: "ruadapt-qwen3-4b-q4-k-m", name: "RuadaptQwen3 4B Q4_K_M", blurb: "Qwen3-4B с расширенным русским токенизатором для русскоязычных сводок.", sizeBytes: 2486507648, verdict: "great", estimate: "≈ 31 ток/с · 4,4 из 16 ГБ",
    vendor: "RefalMachine", family: "Ruadapt", languages: RU_EN, tags: ["ru-tuned", "reasoning-off"], license: APACHE, licenseUrl: SPDX(APACHE), homepage: HF + "RefalMachine/RuadaptQwen3-4B-Instruct-GGUF", params: 4e9, qualityRank: 74,
  }),
];

/** The mock catalog. A fresh install (onboarding preview) has nothing downloaded except what recording itself needs. */
export function mockCatalog(fresh = false): ModelEntry[] {
  return ROWS.map((r): ModelEntry => {
    const { installed = false, active = false, ...rest } = r;
    const required = (r.tags ?? []).includes("required");
    const keep = fresh ? required : installed;
    return {
      ...rest, tags: r.tags ?? [], installed: keep, active: fresh ? required : active,
      download: fresh ? undefined : r.download,
    };
  });
}

/**
 * Shared model catalog state: the Models screen, the Settings choosers and onboarding all read the same list, so a
 * download started in one place shows its progress everywhere. Call `attachModels()` from `onMount` and keep the
 * returned disposer; the engine-event subscription lives as long as at least one view is attached.
 */
import { api } from "./api";
import type { HardwareProfile, ModelEntry, RuntimeInfo } from "./api";
import { t } from "./i18n/index.svelte";
import { onEngineEvent } from "./state.svelte";
import { toast } from "./toast.svelte";
import { AppError, toAppError } from "./api/error";

export const mstore = $state<{
  models: ModelEntry[]; hardware: HardwareProfile | null; runtime: RuntimeInfo | null; loaded: boolean;
  /** Progress of a runtime install or repair (events with `item === runtime.id`), null when idle. */
  rtPct: number | null;
}>({ models: [], hardware: null, runtime: null, loaded: false, rtPct: null });

/** Called with each model that finished downloading and verifying (after the list was refreshed). */
const doneHandlers = new Set<(m: ModelEntry) => void>();
export function onModelDone(h: (m: ModelEntry) => void) { doneHandlers.add(h); return () => { doneHandlers.delete(h); }; }

export async function loadModels() {
  const o = await api.modelsOverview();
  mstore.hardware = o.hardware; mstore.models = o.models; mstore.runtime = o.runtime ?? null; mstore.loaded = true;
  if (mstore.runtime?.busy && mstore.rtPct === null) mstore.rtPct = 0;
}

export const modelById = (id: string | undefined | null) => (id ? mstore.models.find((m) => m.id === id) : undefined);

/** Runs a model action, refreshes the list, reports errors as a toast. */
export async function modelAct(fn: () => Promise<unknown>) { try { await fn(); await loadModels(); } catch (e) { toast.error(e); } }

/** Bytes a model takes on disk right now: the files when installed, the partial download otherwise. */
export const diskBytes = (m: ModelEntry) => (m.installed ? m.sizeBytes : (m.download?.doneBytes ?? 0));

/** Models the app needs to record (voice-activity detection): never offered for removal. */
export const isRequired = (m: ModelEntry) => m.tags.includes("required");

/**
 * Deletes a model's files (and any partial download). `switchTo` makes another installed model primary first, so the app is never
 * left without one. Returns false (after showing why) if the backend refused, e.g. while a recording is running.
 */
export async function removeModel(m: ModelEntry, switchTo?: string): Promise<boolean> {
  try {
    if (switchTo) await api.modelSetActive(switchTo);
    if (m.download) await api.modelCancel(m.id);
    await api.modelRemove(m.id);
    await loadModels();
    return true;
  } catch (e) {
    const a = toAppError(e);
    toast.error(/recording is in progress/i.test(a.message + (a.detail ?? "")) ? new AppError("Идёт запись — модель можно удалить после неё") : e);
    await loadModels();
    return false;
  }
}

let refs = 0;
let off: (() => void) | null = null;

function start() {
  off = onEngineEvent((e) => {
    if (e.type !== "download") return;
    const rt = mstore.runtime;
    if (rt && e.item === rt.id) {
      if (e.state.state === "done") { mstore.rtPct = null; void loadModels(); }
      else if (e.state.state === "failed") { mstore.rtPct = null; toast.error(t("Не удалось установить рантайм llama.cpp. Проверь интернет и повтори.")); void loadModels(); }
      else mstore.rtPct = Math.floor((e.doneBytes / Math.max(1, e.totalBytes)) * 100);
      return;
    }
    const m = mstore.models.find((x) => x.id === e.item);
    if (!m) return;
    if (e.state.state === "done") {
      void loadModels().then(() => { const fresh = modelById(m.id); if (fresh) doneHandlers.forEach((h) => h(fresh)); });
      return;
    }
    const prev = m.download;
    m.download = { state: e.state, doneBytes: e.doneBytes, totalBytes: e.totalBytes, bytesPerSec: e.bytesPerSec, etaSecs: e.bytesPerSec ? Math.ceil((e.totalBytes - e.doneBytes) / e.bytesPerSec) : prev?.etaSecs };
  });
}

export function attachModels(): () => void {
  if (refs++ === 0) start();
  void loadModels();
  let gone = false;
  return () => { if (gone) return; gone = true; if (--refs === 0) { off?.(); off = null; } };
}

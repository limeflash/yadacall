import type { Api } from "./types";
import { createMockApi } from "./mock";
import { createTauriApi } from "./tauri";

export * from "./types";

/** In a plain browser (`pnpm dev`) the same screens run on the in-memory mock. */
export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
export const api: Api = inTauri ? createTauriApi() : createMockApi();

import type { Api, IntegrationsApi } from "./types";
import { createMockApi } from "./mock";
import { createMockIntegrations } from "./mock_integrations";
import { createTauriApi } from "./tauri";
import { createTauriIntegrations } from "./tauri_integrations";

export * from "./types";

/** In a plain browser (`pnpm dev`) the same screens run on the in-memory mock. */
export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
export const api: Api = inTauri ? createTauriApi() : createMockApi();
/** Calendar accounts, agenda, connections and reminders: a separate boundary from `api` (own Rust trait). */
export const integrations: IntegrationsApi = inTauri ? createTauriIntegrations() : createMockIntegrations(api);

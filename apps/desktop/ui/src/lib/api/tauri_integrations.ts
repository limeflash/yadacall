/** `IntegrationsApi` over Tauri `invoke`. Command names are the snake_case twins of the method names. */
import { invoke as rawInvoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { IntegrationEvent, IntegrationsApi } from "./types";
import { toAppError } from "./error";

/** `invoke` that rejects with an `AppError` (short message + raw detail). */
const invoke = <T>(cmd: string, args?: Record<string, unknown>): Promise<T> => rawInvoke<T>(cmd, args).catch((e) => { throw toAppError(e); });

export function createTauriIntegrations(): IntegrationsApi {
  return {
    accounts: () => invoke("accounts"),
    addGoogleAccount: () => invoke("add_google_account"),
    cancelAddAccount: () => invoke("cancel_add_account"),
    removeAccount: (id) => invoke("remove_account", { id }),
    reauthAccount: (id) => invoke("reauth_account", { id }),
    setAccountPolicy: (id, policy) => invoke("set_account_policy", { id, policy }),
    setAccountVisible: (id, visible) => invoke("set_account_visible", { id, visible }),
    agenda: (range) => invoke("agenda", { range }),
    setEventRecording: (eventId, record) => invoke("set_event_recording", { eventId, record }),
    joinAndRecord: (eventId) => invoke("join_and_record", { eventId }),
    joinWithoutRecording: (eventId) => invoke("join_without_recording", { eventId }),
    connections: () => invoke("connections"),
    toggleWant: (service) => invoke("toggle_want", { service }),
    reminderSettings: () => invoke("reminder_settings"),
    setReminderSettings: (settings) => invoke("set_reminder_settings", { settings }),
    detectedCall: () => invoke("detected_call"),
    resolveDetectedCall: (id, record) => invoke("resolve_detected_call", { id, record }),
    async onIntegrations(cb) {
      return listen<IntegrationEvent>("integrations", (e) => cb(e.payload));
    },
  };
}

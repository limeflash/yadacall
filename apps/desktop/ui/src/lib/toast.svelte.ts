/** Toast slots as in the kit: one note (3.4 s, 8 s with an action) and one error (8 s) at a time. */
export type ToastItem = { key: number; message: string; actionLabel?: string; onAction?: () => void };
export const toasts = $state<{ note: ToastItem | null; error: ToastItem | null }>({ note: null, error: null });
const timers: Record<string, ReturnType<typeof setTimeout>> = {};

function put(slot: "note" | "error", v: ToastItem | null, ms: number) {
  clearTimeout(timers[slot]);
  toasts[slot] = v;
  if (v) timers[slot] = setTimeout(() => { toasts[slot] = null; }, ms);
}
export function toast(message: string, opts?: { actionLabel?: string; onAction?: () => void }) {
  put("note", { key: Date.now(), message, ...opts }, opts?.onAction ? 8000 : 3400);
}
toast.error = (message: string) => put("error", { key: Date.now(), message }, 8000);
export function dismissToast(slot: "note" | "error") { put(slot, null, 0); }

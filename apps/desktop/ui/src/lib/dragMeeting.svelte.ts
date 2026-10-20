/**
 * Pointer-based drag of a meeting card onto a folder chip. HTML5 drag-and-drop is not used: with Tauri's
 * `dragDropEnabled` (file drops) the webview on Windows swallows it, and a few lines of pointer code work everywhere.
 * Drop targets are any elements with `data-drop="<filter>"`.
 */
export const drag = $state<{ active: boolean; title: string; x: number; y: number; over: string | null }>({ active: false, title: "", x: 0, y: 0, over: null });
/** True for a moment after a drag ended, so the click that follows pointerup doesn't open the card. */
let justDragged = false;
export const wasDragged = () => justDragged;

const THRESHOLD = 6;

export function startMeetingDrag(e: PointerEvent, title: string, onDrop: (target: string) => void) {
  if (e.button !== 0 || e.pointerType === "touch") return;
  const sx = e.clientX, sy = e.clientY;
  let on = false;
  const target = (x: number, y: number) => (document.elementFromPoint(x, y) as HTMLElement | null)?.closest<HTMLElement>("[data-drop]")?.dataset.drop ?? null;
  const move = (ev: PointerEvent) => {
    if (!on) {
      if (Math.hypot(ev.clientX - sx, ev.clientY - sy) < THRESHOLD) return;
      on = true; drag.active = true; drag.title = title;
      document.documentElement.style.userSelect = "none";
      document.documentElement.style.cursor = "grabbing";
    }
    drag.x = ev.clientX; drag.y = ev.clientY; drag.over = target(ev.clientX, ev.clientY);
  };
  const end = (cancelled: boolean, ev?: PointerEvent) => {
    window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", up); window.removeEventListener("pointercancel", cancel); window.removeEventListener("keydown", esc, true);
    if (!on) return;
    const t = !cancelled && ev ? target(ev.clientX, ev.clientY) : null;
    justDragged = true; setTimeout(() => { justDragged = false; }, 0);
    document.documentElement.style.userSelect = ""; document.documentElement.style.cursor = "";
    drag.active = false; drag.over = null;
    if (t) onDrop(t);
  };
  const up = (ev: PointerEvent) => end(false, ev);
  const cancel = () => end(true);
  const esc = (ev: KeyboardEvent) => { if (ev.key === "Escape" && on) { ev.stopPropagation(); end(true); } };
  window.addEventListener("pointermove", move); window.addEventListener("pointerup", up); window.addEventListener("pointercancel", cancel); window.addEventListener("keydown", esc, true);
}

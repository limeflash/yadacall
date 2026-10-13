/** Whether the window is on screen (false when hidden, minimised or fully covered). */
export const isVisible = () => typeof document === "undefined" || document.visibilityState === "visible";

/** `setInterval` that only ticks while the window is visible, and costs nothing while it is not. */
export function visibleInterval(fn: () => void, ms: number): () => void {
  let id: ReturnType<typeof setInterval> | undefined;
  const sync = () => {
    if (isVisible()) { if (id === undefined) id = setInterval(fn, ms); }
    else if (id !== undefined) { clearInterval(id); id = undefined; }
  };
  document.addEventListener("visibilitychange", sync);
  sync();
  return () => { if (id !== undefined) clearInterval(id); id = undefined; document.removeEventListener("visibilitychange", sync); };
}

/** Marks `<html data-hidden>` while the window is not visible so CSS animations (mascot, pulses) pause too. */
export function installVisibilityFlag() {
  const sync = () => document.documentElement.toggleAttribute("data-hidden", !isVisible());
  document.addEventListener("visibilitychange", sync);
  sync();
}

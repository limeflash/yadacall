const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;
export const isMac = /Mac/i.test(ua) && !/iPhone|iPad/.test(ua);
/** True only inside the Tauri shell on macOS, where the overlay title bar puts the traffic lights over the sidebar. */
export const overlayTitleBar = isMac && typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
export const isWindows = /Windows/i.test(ua);
/** "⌥⇧R" on macOS, "Alt+Shift+R" elsewhere. */
export function prettyAccel(accel: string): string {
  if (!isMac) return accel;
  return accel.replace(/Alt\+/g, "⌥").replace(/Shift\+/g, "⇧").replace(/(Cmd|Command|Super)\+/g, "⌘").replace(/(Ctrl|Control)\+/g, "⌃").replace(/CommandOrControl\+/g, "⌘");
}

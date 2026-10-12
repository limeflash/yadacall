const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;
export const isMac = /Mac/i.test(ua) && !/iPhone|iPad/.test(ua);
export const isWindows = /Windows/i.test(ua);
/** "⌥⇧R" on macOS, "Alt+Shift+R" elsewhere. */
export function prettyAccel(accel: string): string {
  if (!isMac) return accel;
  return accel.replace(/Alt\+/g, "⌥").replace(/Shift\+/g, "⇧").replace(/(Cmd|Command|Super)\+/g, "⌘").replace(/(Ctrl|Control)\+/g, "⌃").replace(/CommandOrControl\+/g, "⌘");
}

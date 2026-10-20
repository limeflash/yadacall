import { convertFileSrc } from "@tauri-apps/api/core";
import { inTauri } from "./api";
import type { MeetingDetail } from "./api";

/**
 * URL for a file that lives in the meeting folder (a bookmark screenshot, "shots/000123456.jpg").
 * Absolute URLs pass through; a relative name needs the folder path from the backend (`MeetingDetail.dirPath`).
 * Returns `undefined` when it can't be resolved, and callers simply show nothing.
 */
export function meetingFileUrl(d: Pick<MeetingDetail, "dirPath"> | null | undefined, file?: string): string | undefined {
  if (!file) return undefined;
  if (/^(https?|data|blob|asset):/i.test(file)) return file;
  if (inTauri && d?.dirPath) return convertFileSrc(d.dirPath.replace(/[\\/]+$/, "") + "/" + file);
  return undefined;
}

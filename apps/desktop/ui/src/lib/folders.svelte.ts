import { api } from "./api";
import type { Folder, MeetingListItem } from "./api";
import { app } from "./state.svelte";
import { toast } from "./toast.svelte";
import { t } from "./i18n/index.svelte";

/** The accent palette folders can be coloured with (same keys as the accent picker). */
export const FOLDER_COLORS = ["mint", "lavender", "sky", "sand", "rose", "sage"] as const;
const COLOR_NAMES: Record<string, string> = { mint: "Мятный", lavender: "Лаванда", sky: "Небесный", sand: "Песочный", rose: "Роза", sage: "Шалфей" };
export const colorName = (key: string) => t(COLOR_NAMES[key] ?? key);
/** CSS colour of a folder; folders without one (or with an unknown key) get a neutral dot. */
export const folderColor = (f?: Pick<Folder, "color">) => (f?.color && (FOLDER_COLORS as readonly string[]).includes(f.color) ? `var(--accent-${f.color})` : "var(--faint)");

/** A list filter: everything, starred, no folder, or a folder id. */
export type FolderFilter = "all" | "fav" | "none" | (string & {});

/** Shared, lazily loaded list of folders (Meetings, Tasks, the meeting header and the ask bar all read it). */
export const folders = $state<{ list: Folder[]; loaded: boolean }>({ list: [], loaded: false });
/** The selected filter per screen, kept while the user opens a meeting and comes back. */
export const filters = $state<{ meetings: FolderFilter; tasks: FolderFilter }>({ meetings: "all", tasks: "all" });

export async function loadFolders() {
  try { folders.list = (await api.listFolders()).slice().sort((a, b) => a.order - b.order); }
  catch (e) { console.error("folders", e); }
  folders.loaded = true;
}
export const folderById = (id?: string | null) => (id ? folders.list.find((f) => f.id === id) : undefined);

/** Next palette colour that is the least used so far, so new folders look different from their neighbours. */
export function nextColor(): string {
  const used = (c: string) => folders.list.filter((f) => f.color === c).length;
  return [...FOLDER_COLORS].sort((a, b) => used(a) - used(b))[0];
}
export async function createFolder(name: string, color = nextColor()): Promise<Folder | undefined> {
  const n = name.trim(); if (!n) return;
  try { const f = await api.createFolder(n, color); await loadFolders(); return f; } catch (e) { toast.error(e); }
}
export async function patchFolder(id: string, patch: { name?: string; color?: string }) {
  const f = folders.list.find((x) => x.id === id);
  if (f) Object.assign(f, patch); // optimistic
  try { await api.updateFolder(id, patch); } catch (e) { toast.error(e); }
  await loadFolders(); app.rev++;
}
export async function removeFolder(id: string) {
  try { await api.deleteFolder(id); } catch (e) { toast.error(e); }
  for (const k of ["meetings", "tasks"] as const) if (filters[k] === id) filters[k] = "all";
  await loadFolders(); app.rev++;
}

/** Applies a filter to anything shaped like a meeting. */
export function matchesFilter(f: FolderFilter, x: { favorite: boolean; folderId?: string }): boolean {
  if (f === "all") return true;
  if (f === "fav") return x.favorite;
  if (f === "none") return !x.folderId;
  return x.folderId === f;
}

export async function setFavorite(m: Pick<MeetingListItem, "id" | "favorite">, favorite: boolean) {
  m.favorite = favorite;
  try { await api.setMeetingFavorite(m.id, favorite); toast(favorite ? t("В избранном — звук хранится всегда") : t("Убрано из избранного")); }
  catch (e) { m.favorite = !favorite; toast.error(e); }
  app.rev++;
}
export async function moveToFolder(m: Pick<MeetingListItem, "id" | "folderId">, folderId: string | null) {
  const prev = m.folderId;
  m.folderId = folderId ?? undefined;
  try {
    await api.setMeetingFolder(m.id, folderId);
    const f = folderById(folderId);
    toast(f ? t("Перенесено в «{name}»", { name: f.name }) : t("Убрано из папки"));
  } catch (e) { m.folderId = prev; toast.error(e); }
  app.rev++;
}

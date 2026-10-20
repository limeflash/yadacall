/** Demo data shared with the Rust backend (`src-tauri/src/demo_data.json`) so mock and Tauri modes show the same library. */
import raw from "../../../../src-tauri/src/demo_data.json";
import type { Meeting, MeetingListItem, Segment, Summary, Turn, ModelEntry, HardwareProfile, InputDevice, Citation } from "./types";

interface RawMeeting {
  id: string; title: string; start: string; durationSec: number; status: string | { processing: number };
  snippet: string; audioNote: string | null; hasAudio: boolean;
  participants: { key: string; name: string }[];
  turns: [string, number, number, string][];
  bookmarks: { atSec: number; label: string }[];
  summary: null | {
    about: string[]; decisions: { text: string; atSec: number }[];
    tasks: { text: string; owner?: string; due?: string; atSec?: number; done: boolean }[];
    openQuestions: ({ text: string; atSec: number } | string)[]; model: string;
  };
}

/** Moves the demo calendar so that its "today" (2026-10-12) is the real today. */
function shiftDays(): number {
  const base = new Date(raw.today + "T00:00:00");
  const now = new Date(); now.setHours(0, 0, 0, 0);
  return Math.round((now.getTime() - base.getTime()) / 86400000);
}
export function shiftIso(local: string, days = shiftDays()): string {
  const d = new Date(local);
  d.setDate(d.getDate() + days);
  return d.toISOString();
}

export interface DemoMeeting { list: MeetingListItem; meeting: Meeting; turns: Turn[]; summary?: Summary; hasAudio: boolean }

export function demoMeetings(): DemoMeeting[] {
  const days = shiftDays();
  return (raw.meetings as unknown as RawMeeting[]).map((m) => {
    const startedAt = shiftIso(m.start, days);
    const status: Meeting["status"] = m.status === "ready" ? { state: "ready" }
      : typeof m.status === "object" ? { state: "processing", stage: "transcribe", progress: m.status.processing } : { state: "recording" };
    const meeting: Meeting = {
      id: m.id, title: m.title, startedAt, durationMs: m.durationSec * 1000, language: "ru",
      tracks: [{ kind: "mic", device: "MacBook Pro", offsetMs: 0, samples: 0 }, { kind: "system", device: "Zoom", offsetMs: 0, samples: 0 }],
      bookmarks: m.bookmarks.map((b) => ({ id: `bm-${m.id.slice(-6)}-${b.atSec}`, atMs: b.atSec * 1000, label: b.label })),
      favorite: false,
      participants: m.participants.filter((p) => p.name).map((p) => ({ key: p.key, name: p.name })),
      asrModel: "gigaam-v3-rnnt", status,
    };
    const turns: Turn[] = m.turns.map(([speaker, s, e, text]) => ({ speaker, startMs: s * 1000, endMs: e * 1000, text }));
    const summary: Summary | undefined = m.summary ? {
      about: m.summary.about,
      decisions: m.summary.decisions.map((d) => ({ text: d.text, atMs: d.atSec * 1000 })),
      tasks: m.summary.tasks.map((t) => ({ text: t.text, owner: t.owner, due: t.due ? shiftIso(t.due + "T12:00:00", days).slice(0, 10) : undefined, atMs: t.atSec != null ? t.atSec * 1000 : undefined, done: t.done })),
      openQuestions: m.summary.openQuestions.map((q) => (typeof q === "string" ? q : q.text)),
      language: "ru", model: m.summary.model,
    } : undefined;
    const list: MeetingListItem = { id: m.id, title: m.title, startedAt, durationMs: meeting.durationMs, status, snippet: m.snippet, audioNote: m.audioNote ?? undefined, favorite: false };
    return { list, meeting, turns, summary, hasAudio: m.hasAudio };
  });
}

export function demoAsk() {
  const a = raw.ask as { question: string; answer: string; citations: { n: number; meeting: string; atSec: number; speaker: string; quote: string }[]; excerpts: number };
  const ms = demoMeetings();
  const citations: Citation[] = a.citations.map((c) => {
    const m = ms.find((x) => x.meeting.id === c.meeting);
    return { n: c.n, meetingId: c.meeting, meetingTitle: m?.meeting.title ?? "", startedAt: m?.meeting.startedAt ?? "", atMs: c.atSec * 1000, speaker: c.speaker, quote: c.quote };
  });
  return { answer: a.answer, citations, excerpts: a.excerpts };
}

export function demoModels(): { hardware: HardwareProfile; models: ModelEntry[]; devices: InputDevice[] } {
  const models = (raw.models as unknown as Array<Omit<ModelEntry, "download"> & { download?: ModelEntry["download"] }>).map((m) => ({ ...m })) as ModelEntry[];
  return { hardware: raw.hardware as HardwareProfile, models, devices: raw.inputDevices as InputDevice[] };
}

/** Lines the mock "recogniser" produces while recording. */
export const LIVE_SCRIPT: Pick<Segment, "speaker" | "text" | "track">[] = [
  { speaker: "them", track: "system", text: "А на мобильной кнопка не уезжает под клавиатуру?" },
  { speaker: "me", track: "mic", text: "Проверю на айфоне, но по макету она должна оставаться над клавиатурой." },
  { speaker: "them", track: "system", text: "Отлично, тогда по карточкам профиля — оставляем как в прошлой версии?" },
  { speaker: "me", track: "mic", text: "Да, только поправим отступы и заголовок раздела." },
  { speaker: "them", track: "system", text: "Хорошо, тогда я соберу замечания и отправлю к вечеру." },
];

/**
 * Browser-only implementation of `IntegrationsApi`, a twin of `src-tauri/src/integrations_demo.rs`: same demo
 * data (`demo_data.json`), same rules (policy + per-event choice, overlaps between accounts, expired accounts
 * hidden), agenda anchored to the moment the page was opened. It also plays the shell's part — reminders
 * and the "call not from the calendar" notice — as `integrations` events.
 */
import raw from "../../../../src-tauri/src/demo_data.json";
import type {
  Api, AgendaEvent, CalendarAccount, Connection, DetectedCall, IntegrationEvent, IntegrationsApi, JoinOutcome, RecordPolicy, ReminderSettings, TimeRange,
} from "./types";

interface RawEvent { id: string; account: string; title: string; startOffsetMin?: number; time?: string; durationMin: number; attendees: string[]; meetUrl?: string; meetingId?: string }
const data = raw.integrations as unknown as {
  accounts: CalendarAccount[]; newAccount: CalendarAccount; events: RawEvent[]; tomorrow: RawEvent[];
  connections: Connection[]; wantDefault: string[]; detectedCall: { platform: string; url: string; afterSec: number };
};

interface Ev { id: string; account: string; title: string; start: number; end: number; attendees: string[]; meetUrl?: string; meetingId?: string }
const WANT_KEY = "yadacall.mock.want";
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function createMockIntegrations(rec: Pick<Api, "recorderStart">): IntegrationsApi {
  const listeners = new Set<(e: IntegrationEvent) => void>();
  const emit = (e: IntegrationEvent) => listeners.forEach((l) => l(e));
  const t0 = Math.floor(Date.now() / 60000) * 60000;
  const tomorrow = new Date(); tomorrow.setDate(tomorrow.getDate() + 1);

  const events: Ev[] = [
    ...data.events.map((e) => ({ e, start: t0 + (e.startOffsetMin ?? 0) * 60000 })),
    ...data.tomorrow.map((e) => {
      const [h, m] = (e.time ?? "00:00").split(":").map(Number);
      const d = new Date(tomorrow); d.setHours(h, m, 0, 0);
      return { e, start: d.getTime() };
    }),
  ].map(({ e, start }) => ({ id: e.id, account: e.account, title: e.title, start, end: start + e.durationMin * 60000, attendees: e.attendees, meetUrl: e.meetUrl, meetingId: e.meetingId }));

  let accounts: CalendarAccount[] = data.accounts.map((a) => ({ ...a }));
  const chosen = new Map<string, boolean>();
  let settings: ReminderSettings = { remindMinutes: 2, openInAccount: true, detectUnscheduled: true };
  let detected: DetectedCall | null = null;
  let addToken = 0;
  let want = new Set<string>(data.wantDefault);
  try { const s = localStorage.getItem(WANT_KEY); if (s) want = new Set<string>(JSON.parse(s)); } catch { /* private mode */ }
  const saveWant = () => { try { localStorage.setItem(WANT_KEY, JSON.stringify([...want])); } catch { /* ignore */ } };

  const visible = (): AgendaEvent[] => {
    const live = events
      .map((e) => ({ e, a: accounts.find((x) => x.id === e.account) }))
      .filter((x): x is { e: Ev; a: CalendarAccount } => !!x.a && x.a.status === "ok" && x.a.visible)
      .sort((p, q) => p.e.start - q.e.start);
    return live.map(({ e, a }) => ({
      id: e.id, accountId: a.id, title: e.title, start: new Date(e.start).toISOString(), end: new Date(e.end).toISOString(),
      meetUrl: e.meetUrl, attendees: e.attendees, recordPolicy: a.recordPolicy,
      willRecord: chosen.get(e.id) ?? a.recordPolicy === "always",
      overlaps: live.filter((o) => o.e.id !== e.id && o.a.id !== a.id && o.e.start < e.end && e.start < o.e.end).map((o) => o.e.id),
      meetingId: e.meetingId,
    }));
  };
  const outcome = (id: string, record: boolean): JoinOutcome => {
    const e = events.find((x) => x.id === id), a = accounts.find((x) => x.id === e?.account);
    if (!e || !a) throw new Error("not found");
    chosen.set(id, record);
    return { title: e.title, accountEmail: a.email, record, endsAt: new Date(e.end).toISOString() };
  };
  const startRecording = (o: JoinOutcome) => { if (o.record) void rec.recorderStart({ title: o.title }); return o; };

  // ---- the shell's part: reminders + simulated notices ----------------------------------------------
  const reminded = new Set<string>();
  setInterval(() => {
    const now = Date.now();
    const next = visible().filter((e) => Date.parse(e.start) > now)[0];
    if (!next || reminded.has(next.id)) return;
    if (Date.parse(next.start) - now > settings.remindMinutes * 60000) return;
    reminded.add(next.id);
    const a = accounts.find((x) => x.id === next.accountId);
    emit({ type: "reminderDue", event: next, accountLabel: a?.label ?? "", accountEmail: a?.email ?? "", minutesBefore: settings.remindMinutes });
  }, 1000);
  setTimeout(() => {
    const a = accounts.find((x) => x.status === "expired");
    if (a) emit({ type: "authExpired", accountId: a.id, email: a.email });
  }, 25000);
  setTimeout(() => {
    if (!settings.detectUnscheduled || detected) return;
    detected = { id: "call-detected-1", platform: data.detectedCall.platform, url: data.detectedCall.url, detectedAt: new Date().toISOString() };
    emit({ type: "callDetected", call: detected });
  }, data.detectedCall.afterSec * 1000);

  return {
    async accounts() { return structuredClone(accounts); },
    async addGoogleAccount() {
      const token = ++addToken;
      for (let step = 1; step <= 3; step++) {
        emit({ type: "addProgress", step });
        await sleep(1500);
        if (token !== addToken) throw new Error("cancelled");
      }
      let acc = accounts.find((a) => a.id === data.newAccount.id);
      if (!acc) { acc = { ...data.newAccount }; accounts = [...accounts, acc]; }
      emit({ type: "agendaChanged" });
      return { ...acc };
    },
    async cancelAddAccount() { addToken++; },
    async removeAccount(id) { accounts = accounts.filter((a) => a.id !== id); emit({ type: "agendaChanged" }); },
    async reauthAccount(id) {
      const a = accounts.find((x) => x.id === id); if (!a) throw new Error("not found");
      a.status = "connecting"; emit({ type: "agendaChanged" });
      await sleep(2400);
      a.status = "ok"; emit({ type: "agendaChanged" });
      return { ...a };
    },
    async setAccountPolicy(id, policy: RecordPolicy) {
      const a = accounts.find((x) => x.id === id); if (!a) return;
      a.recordPolicy = policy;
      events.filter((e) => e.account === id).forEach((e) => chosen.delete(e.id));
    },
    async setAccountVisible(id, visibleNow) { const a = accounts.find((x) => x.id === id); if (a) a.visible = visibleNow; },
    async agenda(range: TimeRange) {
      const from = Date.parse(range.from), to = Date.parse(range.to);
      return visible().filter((e) => Date.parse(e.start) < to && Date.parse(e.end) > from);
    },
    async setEventRecording(id, record) { chosen.set(id, record); },
    async joinAndRecord(id) { return startRecording(outcome(id, true)); },
    async joinWithoutRecording(id) { return outcome(id, false); },
    async connections() {
      const working = accounts.filter((a) => a.status === "ok").length;
      return data.connections.map((c) => ({ ...c, accounts: c.state === "connected" ? working : undefined, want: want.has(c.id) }));
    },
    async toggleWant(service) {
      const c = data.connections.find((x) => x.id === service);
      if (!c || c.state !== "soon") throw new Error("not a coming-soon service");
      const on = !want.has(service);
      if (on) want.add(service); else want.delete(service);
      saveWant();
      return on;
    },
    async reminderSettings() { return { ...settings }; },
    async setReminderSettings(s) { settings = { ...s }; return { ...settings }; },
    async detectedCall() { return detected; },
    async resolveDetectedCall(id, record) {
      if (!detected || detected.id !== id) return;
      const call = detected; detected = null;
      if (record) void rec.recorderStart({ title: `${call.platform} · ${call.url.split("/").pop()}` });
    },
    async onIntegrations(cb) { listeners.add(cb); return () => { listeners.delete(cb); }; },
  };
}

import { i18n, locale, t } from "./i18n/index.svelte";

const p2 = (n: number) => (n < 10 ? "0" : "") + n;

/** 125000 -> "02:05", 3725000 -> "1:02:05". */
export function clock(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600), m = Math.floor(s / 60) % 60, r = s % 60;
  return h > 0 ? `${h}:${p2(m)}:${p2(r)}` : `${p2(Math.floor(s / 60))}:${p2(r)}`;
}

/** Local wall-clock "HH:MM". */
export function hhmm(iso: string | number | Date): string {
  const d = new Date(iso);
  return `${p2(d.getHours())}:${p2(d.getMinutes())}`;
}

/** "11:02 – 11:49" */
export function timeRange(startIso: string, durationMs: number): string {
  const s = new Date(startIso);
  return `${hhmm(s)} – ${hhmm(new Date(s.getTime() + durationMs))}`;
}

export function minutes(durationMs: number): string {
  const m = Math.max(1, Math.round(durationMs / 60000));
  return i18n.lang === "en" ? `${m} min` : `${m} мин`;
}

const sameDay = (a: Date, b: Date) => a.toDateString() === b.toDateString();

/** Group heading: "Сегодня", "Вчера", "Пятница, 9 октября". */
export function dayHeading(iso: string): string {
  const d = new Date(iso), now = new Date();
  if (sameDay(d, now)) return t("Сегодня");
  const y = new Date(now); y.setDate(y.getDate() - 1);
  if (sameDay(d, y)) return t("Вчера");
  const s = new Intl.DateTimeFormat(locale(), { weekday: "long", day: "numeric", month: "long" }).format(d);
  return s.charAt(0).toLocaleUpperCase() + s.slice(1);
}

/** "12 окт" */
export function shortDate(iso: string): string {
  return new Intl.DateTimeFormat(locale(), { day: "numeric", month: "short" }).format(new Date(iso)).replace(".", "");
}

export function dayKey(iso: string): string { return new Date(iso).toDateString(); }

export function bytes(n: number): string {
  const nf = (v: number, max: number) => new Intl.NumberFormat(locale(), { maximumFractionDigits: max }).format(v);
  if (n >= 1e9) return `${nf(n / 1e9, 2)} ${t("ГБ")}`;
  if (n >= 1e6) return `${nf(n / 1e6, 0)} ${t("МБ")}`;
  return `${nf(n / 1e3, 0)} ${t("КБ")}`;
}
export function mbps(bytesPerSec: number): string {
  return `${new Intl.NumberFormat(locale(), { maximumFractionDigits: 1 }).format(bytesPerSec / 1e6)} ${t("МБ/с")}`;
}

export function pct(n: number): string { return `${Math.round(n * 100)}%`; }

/** 101 -> "1:41", 5 -> "0:05". */
export function mss(sec: number): string {
  const s = Math.max(0, Math.floor(sec));
  return `${Math.floor(s / 60)}:${p2(s % 60)}`;
}

/** Countdown to a call: "через 4:02", "через 12 мин", "через 1 ч 47 мин", "идёт 0:12" once it started. */
export function until(leftSec: number): string {
  const s = Math.round(leftSec);
  if (s <= 0) return t("идёт {t}", { t: mss(-s) });
  if (s < 600) return t("через {t}", { t: mss(s) });
  const m = Math.round(s / 60);
  if (m < 60) return t("через {n} мин", { n: m });
  const h = Math.floor(m / 60), r = m % 60;
  return r ? t("через {h} ч {m} мин", { h, m: r }) : t("через {h} ч", { h });
}

/** "понедельник, 12 октября" */
export function longDate(d: Date): string {
  return new Intl.DateTimeFormat(locale(), { weekday: "long", day: "numeric", month: "long" }).format(d);
}

/** Calendar duration: "30 мин", "1 ч", "1 ч 30 мин". */
export function durLabel(min: number): string {
  const h = Math.floor(min / 60), m = min % 60;
  if (h === 0) return t("{n} мин", { n: m });
  return m ? t("{h} ч {m} мин", { h, m }) : t("{h} ч", { h });
}

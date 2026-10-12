import { en } from "./en";

/** Interface language. Russian is the source language of every string in the markup. */
export const i18n = $state<{ lang: "ru" | "en" }>({ lang: "ru" });

/** Translate a Russian source string. Unknown strings fall back to the Russian text. */
export function t(ru: string, vars?: Record<string, string | number>): string {
  let s = i18n.lang === "en" ? (en[ru] ?? ru) : ru;
  if (vars) for (const k in vars) s = s.replaceAll(`{${k}}`, String(vars[k]));
  return s;
}

export function locale(): string { return i18n.lang === "en" ? "en-GB" : "ru-RU"; }

/** Russian / English plural helper: plural(n, ["задача","задачи","задач"], ["task","tasks"]). */
export function plural(n: number, ru: [string, string, string], enForms: [string, string]): string {
  if (i18n.lang === "en") return n === 1 ? enForms[0] : enForms[1];
  const a = Math.abs(n) % 100, b = a % 10;
  if (a > 10 && a < 20) return ru[2];
  if (b > 1 && b < 5) return ru[1];
  if (b === 1) return ru[0];
  return ru[2];
}

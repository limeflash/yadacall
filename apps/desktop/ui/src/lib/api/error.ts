import { t } from "../i18n/index.svelte";

/** A backend error: `message` is short and user-facing (Russian source text), `detail` is the raw error. */
export class AppError extends Error {
  detail?: string;
  constructor(message: string, detail?: string) {
    super(message);
    this.detail = detail;
  }
  override toString() { return t(this.message); }
}

/** Normalises whatever `invoke` rejected with: `{ message, detail }` from `CmdError`, or a plain string. */
export function toAppError(e: unknown): AppError {
  if (e instanceof AppError) return e;
  if (e && typeof e === "object" && "message" in e && typeof (e as { message: unknown }).message === "string") {
    const o = e as { message: string; detail?: unknown };
    return new AppError(o.message, typeof o.detail === "string" && o.detail !== o.message ? o.detail : undefined);
  }
  return new AppError(String(e));
}

/** The message of any caught error, in the interface language. */
export function errText(e: unknown): string { return t(toAppError(e).message); }

// Server-generated text arrives as a stable message code + typed params ({ code, params }) next to an English
// fallback string. It is rendered here from i18n-server.json, the catalogue the server renders letters and
// reports from, so both always say the same thing. Unknown codes (an older web app) show the fallback.
import catalog from "./i18n-server.json";

export type Lang = "en" | "ja";
export interface Msg {
  code: string;
  params?: Record<string, unknown>;
}

const CATALOG: Record<string, { en: string; ja: string }> = catalog;

export function isMsg(v: unknown): v is Msg {
  return typeof v === "object" && v !== null && typeof (v as Msg).code === "string" && (v as Msg).code in CATALOG;
}

function param(lang: Lang, v: unknown): string {
  if (v === null || v === undefined) return "—";
  if (Array.isArray(v)) return v.map((x) => param(lang, x)).join(lang === "ja" ? "、" : ", ");
  if (isMsg(v)) return renderMsg(lang, v);
  if (typeof v === "object") return JSON.stringify(v);
  return String(v);
}

/** The message in `lang`; `fallback` (the server's English text) when `m` is not a known message. */
export function renderMsg(lang: Lang, m: unknown, fallback = ""): string {
  if (!isMsg(m)) return fallback;
  const tpl = CATALOG[m.code][lang];
  const p = m.params ?? {};
  return tpl.replace(/\{(\w+)\}/g, (all, k: string) => (k in p ? param(lang, p[k]) : all));
}

export const SERVER_CODES = Object.keys(CATALOG);

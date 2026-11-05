import type { TFn } from "./i18n";
import type { Claim, Snap } from "./types";

export function claimValue(c: Claim): string {
  const v = c.value_num ?? c.value_text ?? "";
  return c.value_unit ? `${v} ${c.value_unit}` : v;
}

export function snapValue(s?: Snap): string {
  return s ? `${s.num ?? s.text ?? ""} ${s.unit ?? ""}`.trim() : "";
}

/** Binary-prefixed file size with one decimal (e.g. "20.0 KB"); exact bytes stay in the title. */
export function fileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(1)} ${units[i]}`;
}

export function sourceLocation(t: TFn, loc: Record<string, string | number> | undefined): string {
  const known = ["page", "row", "col", "sheet", "cell", "line"];
  const parts = Object.entries(loc ?? {}).map(([k, v]) => (known.includes(k) ? t(`loc.${k}` as never, { v }) : `${k} ${v}`));
  return parts.length ? parts.join(t("loc.sep")) : t("evidence.noLocation");
}

/** Compact form for table cells; the caveat lives in the column note. */
export function confidenceShort(t: TFn, bps: number): string {
  return t("evidence.confidenceShort", { pct: (bps / 100).toFixed(0) });
}

export function confidence(t: TFn, bps: number): string {
  return t("evidence.confidence", { pct: (bps / 100).toFixed(0) });
}

import type { TFn } from "./i18n";
import type { Claim, Snap } from "./types";

export function claimValue(c: Claim): string {
  const v = c.value_num ?? c.value_text ?? "";
  return c.value_unit ? `${v} ${c.value_unit}` : v;
}

export function snapValue(s?: Snap): string {
  return s ? `${s.num ?? s.text ?? ""} ${s.unit ?? ""}`.trim() : "";
}

export function sourceLocation(t: TFn, loc: Record<string, string | number> | undefined): string {
  const parts = Object.entries(loc ?? {}).map(([k, v]) => `${k} ${v}`);
  return parts.length ? parts.join(", ") : t("evidence.noLocation");
}

export function confidence(t: TFn, bps: number): string {
  return t("evidence.confidence", { pct: (bps / 100).toFixed(0) });
}

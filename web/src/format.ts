import type { Claim } from "./types";

export function claimValue(c: Claim): string {
  const v = c.value_num ?? c.value_text ?? "";
  return c.value_unit ? `${v} ${c.value_unit}` : v;
}

export function sourceLocation(loc: Record<string, string | number>): string {
  const parts = Object.entries(loc).map(([k, v]) => `${k} ${v}`);
  return parts.length ? parts.join(", ") : "no location recorded";
}

export function confidence(bps: number): string {
  return `${(bps / 100).toFixed(0)}% (method-specific, not probability of truth)`;
}

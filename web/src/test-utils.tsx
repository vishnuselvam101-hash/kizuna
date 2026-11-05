import { render } from "@testing-library/react";
import axe from "axe-core";
import type { ReactNode } from "react";
import { Api } from "./api";
import { I18nProvider, type Lang } from "./i18n";
import { roleCan, SessionContext } from "./session";
import type { Me } from "./types";

export type Handler = (body: unknown, init: RequestInit) => unknown | Response;
export interface Call {
  method: string;
  path: string;
  body: unknown;
  headers: Record<string, string>;
}

/** Fake backend: routes are "METHOD /path" (query string ignored unless the key includes "?"). */
export function fakeApi(routes: Record<string, Handler | unknown>) {
  const calls: Call[] = [];
  const fetchFn = async (url: RequestInfo | URL, init?: RequestInit) => {
    const full = String(url).replace("/api/v1", "");
    const method = init?.method ?? "GET";
    const body = typeof init?.body === "string" ? JSON.parse(init.body) : init?.body;
    calls.push({ method, path: full, body, headers: (init?.headers ?? {}) as Record<string, string> });
    const r = routes[`${method} ${full}`] ?? routes[`${method} ${full.split("?")[0]}`];
    if (r === undefined) return new Response(JSON.stringify({ error: "not_found" }), { status: 404 });
    const out = typeof r === "function" ? (r as Handler)(body, init ?? {}) : r;
    if (out instanceof Response) return out;
    return new Response(typeof out === "string" ? out : JSON.stringify(out), { status: 200, headers: { "content-type": "application/json" } });
  };
  return { api: new Api("/api/v1", fetchFn as typeof fetch), calls };
}

export const ME: Me = {
  user_id: "u-admin",
  tenant_id: "t1",
  email: "admin@example.test",
  display_name: "Aiko Admin",
  role: "tenant_admin",
  mfa_enabled: true,
  session_mfa_verified: true,
};

export function renderWithSession(ui: ReactNode, { api, me = ME, lang = "en" }: { api: Api; me?: Me; lang?: Lang }) {
  return render(
    <I18nProvider initial={lang}>
      <SessionContext.Provider value={{ api, me, can: (p) => roleCan(me.role, p) }}>{ui}</SessionContext.Provider>
    </I18nProvider>,
  );
}

/** axe-core in jsdom. Colour contrast needs real layout, so it is checked in the browser run (PERF.md). */
export async function axeViolations(node: Element) {
  const r = await axe.run(node, { rules: { "color-contrast": { enabled: false } } });
  return r.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(" | ")}`);
}

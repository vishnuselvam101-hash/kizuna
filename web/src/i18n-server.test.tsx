import { fireEvent, screen, waitFor } from "@testing-library/react";
import { claim, conflictDetail, detectionSettings } from "./fixtures";
import { FindingsView } from "./FindingsView";
import catalog from "./i18n-server.json";
import { renderMsg, SERVER_CODES } from "./msg";
import { SecurityView } from "./SecurityView";
import { fakeApi, renderWithSession } from "./test-utils";
import type { Conflict } from "./types";

/** A run of three or more plain English words in one text (identifiers such as value_mismatch, qa.result or
 * SHA-256 do not count). */
function englishRun(s: string): string | null {
  let run: string[] = [];
  for (const w of s.split(/[^A-Za-z0-9'_{}.-]+/).map((x) => x.replace(/[.]+$/, ""))) {
    if (w.length >= 2 && /^[A-Za-z']+$/.test(w)) {
      run.push(w);
      if (run.length >= 3) return run.join(" ");
    } else if (w) run = [];
  }
  return null;
}

/** English runs per DOM text node (separate nodes are separate texts). */
function textRuns(root: Element): string[] {
  const out: string[] = [];
  const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let n = w.nextNode(); n; n = w.nextNode()) {
    const r = englishRun(n.textContent ?? "");
    if (r) out.push(r);
  }
  return out;
}

const placeholders = (s: string) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

test("every server message code has English and natural Japanese text", () => {
  const c: Record<string, { en: string; ja: string }> = catalog;
  expect(SERVER_CODES.length).toBeGreaterThan(100);
  for (const [code, e] of Object.entries(c)) {
    expect(e.en.trim(), code).not.toBe("");
    expect(e.ja.trim(), code).not.toBe("");
    for (const p of placeholders(e.en)) expect(placeholders(e.ja), `${code} drops {${p}}`).toContain(p);
    if (code === "text.verbatim") continue;
    const literal = e.ja.replace(/\{\w+\}/g, "");
    expect(/[぀-ヿ一-鿿]/.test(literal), `${code}: no Japanese`).toBe(true);
    expect(englishRun(literal), `${code}: English in JA`).toBeNull();
  }
});

test("messages render nested messages, lists and fall back to the server's English", () => {
  const m = { code: "invalidation.superseded", params: { why: { code: "materiality.exceeds_threshold", params: { threshold: "0.05" } } } };
  expect(renderMsg("ja", m)).toBe("証拠が訂正されました：変化量がしきい値 0.05 を超えています");
  expect(renderMsg("en", m)).toBe("evidence corrected: change exceeds threshold 0.05");
  const list = { code: "conflict.applicability.other_supplier", params: { supplier: "Beta", covered: ["ACME", "Gamma"] } };
  expect(renderMsg("ja", list)).toContain("ACME、Gamma");
  expect(renderMsg("ja", { code: "no.such.code" }, "as recorded")).toBe("as recorded");
  expect(renderMsg("ja", undefined, "as recorded")).toBe("as recorded");
});

// Server output for seeded data (shape and codes as kizuna-api returns them; see tests/i18n_messages.rs).
const msg = (code: string, params: Record<string, unknown> = {}) => ({ code, params });
const seeded: Conflict[] = [
  ["value_mismatch", msg("conflict.spec_value.sources_differ")],
  ["revision_mismatch", msg("conflict.revision.older", { role: msg("term.revision_role.ordered"), revision: "B", latest: "C" })],
  ["expired_evidence", msg("conflict.expiry.expired", { expired_on: "2026-01-31", reference_date: "2026-11-05" })],
  ["tolerance_violation", msg("conflict.tolerance.outside", { measured: "120.5", lower: "119.8", upper: "120.2", unit: "mm" })],
  ["conflicting_inspection", msg("conflict.inspection.lot_disagrees", { lot: "L1", predicate: "qa.result" })],
].map(([kind, m], i) => ({
  id: `cf-${i}`,
  kind: kind as string,
  subject_key: "BRK-100",
  status: "open",
  explanation: { rule: "spec_value", why: renderMsg("en", m), msg: m },
  detected_at: "2026-11-05T00:00:00Z",
}));

test("Japanese findings screen shows no English sentence for seeded server data", async () => {
  const detail = {
    ...conflictDetail,
    conflict: seeded[1],
    claim_a: claim("c-a", "B"),
    limitations: "Deterministic rule mfg-rules@2; extracted values may contain extraction errors.",
    limitations_msg: msg("limitations.finding", { rule: "mfg-rules@2", confidence: 9100 }),
    potentially_affected_decisions: [],
  };
  const { api } = fakeApi({ "GET /conflicts?status=open": seeded, "GET /conflicts/cf-1": detail });
  const { container } = renderWithSession(<FindingsView />, { api, lang: "ja" });
  await screen.findByText("発注リビジョン B は最新のリビジョン C より古い版です");
  fireEvent.click(screen.getAllByText("BRK-100")[1]);
  await screen.findByText(/決定的ルール mfg-rules@2/);
  for (const c of seeded) expect(container.textContent).not.toContain(c.explanation.why);
  expect(textRuns(container)).toEqual([]);
});

test("Japanese security screen shows incident summaries and action details in Japanese", async () => {
  const summary = msg("security.denied_requests", { count: 12 });
  const incident = {
    id: "i1",
    rule: "denied_requests",
    severity: "medium",
    subject: "u-viewer",
    summary: "12 denied requests within 10 minutes",
    summary_msg: summary,
    status: "open",
    event_ids: [],
    first_seen: "2026-11-05T00:00:00Z",
    last_seen: "2026-11-05T00:05:00Z",
    resolution: null,
  };
  const action = {
    id: "a1",
    kind: "revoke_user_sessions",
    target: "u-viewer",
    policy: "pre_authorised",
    status: "executed",
    detail: "2 sessions revoked",
    detail_msg: msg("action.sessions_revoked", { n: 2 }),
    decided_by: null,
    updated_at: "2026-11-05T00:05:00Z",
  };
  const { api } = fakeApi({
    "GET /security/summary": { open_incidents: 1, actions_awaiting_approval: 0, events_24h: {}, active_ip_blocks: [] },
    "GET /security/incidents": [incident],
    "GET /security/incidents/i1": { incident, actions: [action], events: [] },
    "GET /security/detection-settings": detectionSettings(),
  });
  const { container } = renderWithSession(<SecurityView />, { api, lang: "ja" });
  const cell = await screen.findByText("10分以内に権限のない操作が 12 回拒否されました");
  fireEvent.click(cell);
  await waitFor(() => expect(screen.getByText("セッションを 2 件無効化しました")).toBeTruthy());
  expect(container.textContent).not.toContain(incident.summary);
  expect(container.textContent).not.toContain(action.detail);
  expect(textRuns(container)).toEqual([]);
});

test("server-rendered text follows the UI language (Accept-Language)", async () => {
  const { api, calls } = fakeApi({ "GET /conflicts?status=open": [] });
  renderWithSession(<FindingsView />, { api, lang: "ja" });
  await waitFor(() => expect(calls.length).toBeGreaterThan(0));
  expect(calls[0].headers["accept-language"]).toBe("ja");
});

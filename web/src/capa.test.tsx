import { fireEvent, screen, within } from "@testing-library/react";
import { CapaView, type CapaCase, type CapaDetail } from "./CapaView";
import { axeViolations, fakeApi, ME, renderWithSession } from "./test-utils";

const kase: CapaCase = {
  id: "k1",
  reference: "CA-1A2B3C4D",
  title: "SPC SHAFT-7 mfg.dimension.diameter: WE1",
  source_kind: "spc",
  source_ref: "part|SHAFT-7|mfg.dimension.diameter",
  subject_key: "SHAFT-7",
  supplier_key: "ACME",
  status: "open",
  reopened: 1,
  closure_requested_by: null,
  closed_by: null,
  created_by: "u-admin",
  created_at: "2026-11-05T01:00:00Z",
  updated_at: "2026-11-05T02:00:00Z",
  progress: 5,
  overdue_actions: 1,
};
const step = (n: number, status: string, content = {}, missing: string[] = []) => ({ discipline: n, code: `d${n}`, status, content, missing, completed_at: status === "done" ? "2026-11-05T01:30:00Z" : null });
const detail: CapaDetail = {
  case: kase,
  source: { center: 10, sigma: 0.118, violations: [{ index: 12, rule: "WE1", claim: "c13" }] },
  source_links: [{ claim: "c13" }],
  steps: [
    step(1, "done", { members: [{ user: "u-admin", role: "leader" }, { user: "u2", role: "member" }] }),
    step(2, "done", { statement: "Diameter out", is_is_not: [{ dimension: "what", is: "diameter", is_not: "length" }], evidence: [{ claim: "c13" }] }),
    step(3, "done", { items: [{ description: "Sort stock", evidence: [{ claim: "c13" }] }] }),
    step(4, "done", { items: [{ kind: "occurrence", description: "Worn wheel", evidence: [{ claim: "c13" }] }] }),
    step(5, "done", {}),
    step(6, "open", { statement: "Back in control", evidence: [{ artifact: "a9" }] }, ["verification_needs_independent_evidence", "spc_too_few_new_points"]),
    step(7, "open"),
    step(8, "open"),
  ],
  actions: [{ id: "x1", description: "Replace wheel", owner_id: "u2", owner_name: "Ken Reviewer", due_date: "2026-11-04", causes: [0], status: "open", evidence: [], done_comment: null, overdue: true }],
  supplier_requests: [{ id: "r1", reference: "CA-1A2B3C4D-S1", supplier: "ACME", disciplines: [3, 4, 5], due_date: "2026-11-30", status: "draft", created_by: "u2" }],
  supplier_responses: [{ request: "r1", artifact: "a9", document: "acme-8d.pdf", added_at: "2026-11-05T01:40:00Z" }],
  events: [
    { at: "2026-11-05T01:00:00Z", actor: "u-admin", actor_kind: "user", kind: "capa.opened", discipline: null, detail: {} },
    { at: "2026-11-05T01:50:00Z", actor: null, actor_kind: "system", kind: "capa.reopened", discipline: 6, detail: { cause: "spc_point_beyond_limits" } },
  ],
  evidence: {
    c13: { type: "claim", subject: "SHAFT-7", predicate: "mfg.dimension.diameter", value: "12.500", unit: "mm", document: null, usable: true, origin: "internal" },
    a9: { type: "artifact", document: "acme-8d.pdf", kind: "other", usable: true, origin: "supplier" },
  },
  candidates: ["c13"],
  users: [
    { id: "u-admin", name: "Aiko Admin", role: "tenant_admin" },
    { id: "u2", name: "Ken Reviewer", role: "reviewer" },
  ],
  spc_recheck: "toofewpoints",
};

function api(d: CapaDetail = detail) {
  return fakeApi({
    "GET /capa/cases?status=open": [kase],
    "GET /capa/cases/k1": d,
    "GET /capa/notifications": [{ id: "n1", case_id: "k1", reference: kase.reference, description: "Replace wheel", kind: "action_overdue", due_date: "2026-11-04", read_at: null }],
    "PUT /capa/cases/k1/steps/6": d,
    "POST /capa/cases/k1/steps/6/complete": new Response(JSON.stringify({ error: "step_requirements_unmet" }), { status: 409 }),
    "POST /capa/cases/k1/closure/approve": { ...d, case: { ...d.case, status: "closed" } },
  });
}

async function openCase() {
  fireEvent.click(await screen.findByRole("button", { name: kase.reference }));
  return screen.findByRole("tablist");
}

test.each(["en", "ja"] as const)("case list and 8D case view have no axe violations (%s)", async (lang) => {
  const { api: a } = api();
  const { container } = renderWithSession(<CapaView />, { api: a, lang });
  await openCase();
  if (lang === "ja") {
    for (const term of ["是正処置", "D3 暫定処置", "D4 根本原因", "D5 恒久対策", "D6 効果確認", "D7 再発防止・水平展開"]) expect(screen.getAllByText(new RegExp(term)).length).toBeGreaterThan(0);
    expect(screen.getByText(/サプライヤー自身の資料だけでは不十分です/)).toBeTruthy();
  } else {
    expect(screen.getByText(/cite independent new evidence/)).toBeTruthy();
  }
  expect(await axeViolations(container)).toEqual([]);
});

test("opens on the first open step, shows what is missing, and the step tabs work with arrow keys", async () => {
  const { api: a } = api();
  renderWithSession(<CapaView />, { api: a });
  const tabs = await openCase();
  const d6 = within(tabs).getByRole("tab", { name: /D6 Effectiveness/ });
  expect(d6.getAttribute("aria-selected")).toBe("true");
  // Step status badges read "open" / "done", not the "Open a case" button text (capa.open).
  expect(d6.textContent).toBe("D6 Effectiveness open");
  expect(within(tabs).getByRole("tab", { name: /D1 Team/ }).textContent).toBe("D1 Team done");
  expect(screen.getByText("Not enough new measurements yet for the SPC re-check.")).toBeTruthy();
  fireEvent.keyDown(d6, { key: "ArrowLeft" });
  const d5 = within(tabs).getByRole("tab", { name: /D5 Corrective actions/ });
  expect(d5.getAttribute("aria-selected")).toBe("true");
  expect(document.activeElement).toBe(d5);
  // D5 shows the overdue action and offers to record implementation.
  expect(screen.getByText("Replace wheel")).toBeTruthy();
  expect(screen.getAllByText("Overdue").length).toBeGreaterThan(0);
  expect(screen.getByRole("button", { name: "Record implementation" })).toBeTruthy();
  fireEvent.keyDown(d5, { key: "End" });
  expect(within(tabs).getByRole("tab", { name: /D8 Closure/ }).getAttribute("aria-selected")).toBe("true");
});

test("saving sends only filled fields; completing with missing evidence shows the server's refusal", async () => {
  const { api: a, calls } = api();
  renderWithSession(<CapaView />, { api: a });
  await openCase();
  fireEvent.change(screen.getByLabelText("Verification summary"), { target: { value: "Incoming inspection confirms" } });
  fireEvent.click(screen.getByRole("button", { name: "Save draft" }));
  await screen.findByText("Draft saved.");
  expect(calls.find((c) => c.method === "PUT")?.body).toEqual({ content: { statement: "Incoming inspection confirms", evidence: [{ artifact: "a9" }] } });
  fireEvent.click(screen.getByRole("button", { name: "Complete step" }));
  expect((await screen.findByRole("alert")).textContent).toMatch(/still lacks required evidence/);
  // Supplier documents are marked as such wherever they are cited.
  expect(screen.getAllByText("supplier").length).toBeGreaterThan(0);
});

test("four-eyes: the closure requester cannot approve; someone else can", async () => {
  const pending = { ...detail, case: { ...kase, status: "closure_pending", closure_requested_by: ME.user_id } };
  const { api: a } = api(pending);
  renderWithSession(<CapaView />, { api: a });
  await openCase();
  fireEvent.click(screen.getByRole("tab", { name: /D8 Closure/ }));
  expect((screen.getByRole("button", { name: "Approve closure" }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText(/the person who requested closure cannot approve it/)).toBeTruthy();

  const other = { ...detail, case: { ...kase, status: "closure_pending", closure_requested_by: "u2" } };
  const second = api(other);
  const { unmount } = renderWithSession(<CapaView />, { api: second.api });
  unmount();
  document.body.innerHTML = "";
  renderWithSession(<CapaView />, { api: second.api });
  await openCase();
  fireEvent.click(screen.getByRole("tab", { name: /D8 Closure/ }));
  fireEvent.click(screen.getByRole("button", { name: "Approve closure" }));
  await screen.findByText("Case closed.");
  expect(second.calls.some((c) => c.method === "POST" && c.path === "/capa/cases/k1/closure/approve")).toBe(true);
});

test("viewers see the case but get no editing controls", async () => {
  const { api: a } = api();
  renderWithSession(<CapaView />, { api: a, me: { ...ME, role: "viewer" } });
  await openCase();
  expect(screen.queryByRole("button", { name: "Save draft" })).toBeNull();
  expect(screen.queryByRole("form", { name: "Open a case" })).toBeNull();
  expect(screen.getByText("Action overdue since 2026-11-04: Replace wheel (CA-1A2B3C4D)")).toBeTruthy();
});

test("recording an action's implementation keeps the form open when the server refuses it", async () => {
  const { api: a } = api();
  a.request = ((orig) =>
    async (method: string, path: string, body?: unknown) =>
      path === "/capa/actions/x1/done" ? Promise.reject(Object.assign(new Error("evidence_required"), { code: "evidence_required", status: 422 })) : orig(method, path, body))(a.request.bind(a)) as typeof a.request;
  renderWithSession(<CapaView />, { api: a });
  const tabs = await openCase();
  fireEvent.click(within(tabs).getByRole("tab", { name: /D5 Corrective actions/ }));
  fireEvent.click(screen.getByRole("button", { name: "Record implementation" }));
  fireEvent.change(screen.getByLabelText("Implementation note"), { target: { value: "New wheel fitted" } });
  const buttons = screen.getAllByRole("button", { name: "Record implementation" });
  fireEvent.click(buttons[buttons.length - 1]);
  await screen.findByRole("alert");
  expect((screen.getByLabelText("Implementation note") as HTMLInputElement).value).toBe("New wheel fitted");
});

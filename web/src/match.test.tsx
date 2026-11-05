import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { MatchView, ReRequestView, type EvidenceRequest, type LineSummary, type MatchDetail, type Plan } from "./MatchView";
import { axeViolations, fakeApi, ME, renderWithSession } from "./test-utils";

const line = (n: string, critical: number, kinds: string[] = []): LineSummary => ({
  purchase_order: "PO-77",
  po_line: n,
  part: `MX-${n}00`,
  supplier: "NAGOYA FORGE",
  ordered_revision: "C",
  certificates: 2,
  critical,
  major: 0,
  minor: 0,
  kinds,
  acceptance: null,
});

const detail: MatchDetail = {
  match: {
    rule_version: "mfg-match@1",
    line: { purchase_order: "PO-77", line: "2", part: "MX-200" },
    supplier: "NAGOYA FORGE",
    ordered_revision: "B",
    drawing_revision: "B",
    latest_drawing_revision: "B",
    certificates: [{ doc: "a-ir", linked_by: "purchase_order", lots: ["L-2"], heats: [] }],
    checks: [
      { check: "revision", status: "match", po: { value: "B", claims: ["c-po"] }, drawing: { value: "B", claims: ["c-dw"] }, certificate: { value: "B", claims: ["c-ir"] } },
      { check: "dimension.length", status: "mismatch", po: { value: null, claims: [] }, drawing: { value: "80 +0.1/-0.1 mm", claims: ["c-dw"] }, certificate: { value: "80.3 mm", claims: ["c-ir"] } },
    ],
    findings: [
      {
        kind: "measurement_out_of_tolerance",
        severity: "critical",
        check: "dimension.length",
        po_claims: ["c-po"],
        drawing_claims: ["c-dw"],
        certificate_claims: ["c-ir"],
        certificate_doc: "a-ir",
        explanation: { rule: "mfg-match@1", why: "measured 80.3 mm is outside 80 +0.1/-0.1 mm" },
      },
    ],
  },
  blocking: 1,
  citations: {
    claims: {
      "c-po": { predicate: "po.revision_ordered", value_text: "B", value_num: null, value_unit: null, revision: null, artifact_id: "a-po", source_location: { row: 3, col: 5 } },
      "c-dw": { predicate: "mfg.dimension.length", value_text: null, value_num: "80", value_unit: "mm", revision: "B", artifact_id: "a-dw", source_location: { page: 1, line: 5 } },
      "c-ir": { predicate: "qa.measured.length", value_text: null, value_num: "80.3", value_unit: "mm", revision: null, artifact_id: "a-ir", source_location: { page: 1, line: 7 } },
    },
    documents: {
      "a-po": { kind: "purchase_order", display_name: "po-77.csv", sha256: "ab".repeat(32) },
      "a-dw": { kind: "drawing", display_name: "dm-200.pdf", sha256: "cd".repeat(32) },
      "a-ir": { kind: "inspection_report", display_name: "ir-2.pdf", sha256: "ef".repeat(32) },
    },
  },
  runs: [],
};

function matchApi() {
  return fakeApi({
    "GET /match/lines": { rule_version: "mfg-match@1", lines: [line("1", 0), line("2", 1, ["measurement_out_of_tolerance"])] },
    "GET /match/line": detail,
    "POST /match/line/accept": { decision: { id: "d-1" } },
  });
}

test.each(["en", "ja"] as const)("3-way match view is axe-clean with a selected line (%s)", async (lang) => {
  const { api } = matchApi();
  const { container } = renderWithSession(<MatchView />, { api, lang });
  const region = await screen.findByRole("region", { name: lang === "ja" ? /J・K/ : /Use J and K/ });
  fireEvent.keyDown(region, { key: "j" });
  fireEvent.keyDown(region, { key: "Enter" });
  await screen.findByRole("table", { name: lang === "ja" ? /照合表/ : /Match matrix/ });
  expect(await axeViolations(container)).toEqual([]);
});

test("a finding cites the purchase order, the drawing and the certificate with exact locations", async () => {
  const { api, calls } = matchApi();
  renderWithSession(<MatchView />, { api });
  fireEvent.click(await screen.findByText("MX-200"));
  const f = await screen.findByText("Measured value out of tolerance");
  const card = f.closest("li")!;
  for (const side of ["Purchase order", "Drawing / spec", "Certificate"]) expect(within(card).getByRole("region", { name: side })).toBeTruthy();
  expect(within(card).getByText(/ir-2\.pdf · page 1, line 7/)).toBeTruthy();
  expect(within(card).getByText(/po-77\.csv · row 3, col 5/)).toBeTruthy();
  expect(calls.some((c) => c.path === "/match/line?po=PO-77&line=2&part=MX-200")).toBe(true);
  // Approval is not offered while blocking findings are open; a rationale is required.
  const outcome = screen.getByRole("combobox", { name: "Outcome" }) as HTMLSelectElement;
  expect(outcome.value).toBe("conditional");
  expect((within(outcome).getByRole("option", { name: "Approved" }) as HTMLOptionElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Record acceptance decision" }));
  expect(screen.getByRole("textbox", { name: "Rationale" }).getAttribute("aria-invalid")).toBe("true");
  expect(calls.some((c) => c.path === "/match/line/accept")).toBe(false);
  fireEvent.change(screen.getByRole("textbox", { name: "Rationale" }), { target: { value: "Deviation accepted by design" } });
  fireEvent.click(screen.getByRole("button", { name: "Record acceptance decision" }));
  await screen.findByText("Acceptance decision recorded.");
  expect(calls.find((c) => c.path === "/match/line/accept")?.body).toEqual({
    po: "PO-77",
    line: "2",
    part: "MX-200",
    outcome: "conditional",
    rationale: "Deviation accepted by design",
  });
});

test("viewers see the match but cannot record an acceptance", async () => {
  const { api } = matchApi();
  renderWithSession(<MatchView />, { api, me: { ...ME, role: "viewer" } });
  fireEvent.click(await screen.findByText("MX-200"));
  await screen.findByText("Measured value out of tolerance");
  expect(screen.queryByRole("button", { name: "Record acceptance decision" })).toBeNull();
});

const plan: Plan = {
  engine: "mfg-resupply@1",
  decisions_considered: 2,
  suppliers: [
    {
      supplier: "NAGOYA FORGE",
      documents: [
        {
          doc: "a-ir",
          name: "ir-1.pdf",
          sha256: "ef".repeat(32),
          items: [{ part: "MX-100", predicate: "qa.measured.length", purchase_order: "PO-77", po_line: "1", lot: "L-1", heat: null }],
          reasons: [{ predicate: "mfg.tolerance.length.plus", from: "0.1 mm", to: "0.03 mm", cause: "superseded" }],
        },
      ],
      baseline_count: 2,
      exact: true,
      uncovered_items: [],
      decision_ids: ["d-1"],
    },
  ],
  no_supplier_action: [{ decision_id: "d-2", changed_predicates: ["mfg.approval.status"] }],
};

const req: EvidenceRequest = {
  id: "r-1",
  reference: "RR-0001",
  supplier: "NAGOYA FORGE",
  status: "draft",
  requested_count: 1,
  baseline_count: 2,
  exact: true,
  body_ja: "NAGOYA FORGE 御中\n件名：品質証明書類の再提出のお願い",
  body_en: "To: NAGOYA FORGE\nSubject: Request to resubmit quality documents",
  content_sha256: "12".repeat(32),
  created_by: "u-other",
  created_at: "2026-11-05T09:00:00Z",
  review_comment: null,
};

function rrApi() {
  return fakeApi({
    "GET /evidence-requests/preview": plan,
    "GET /evidence-requests": [req],
    "POST /evidence-requests": [req],
    "GET /evidence-requests/r-1/document": () => new Response("【下書き】", { status: 200, headers: { "content-type": "text/plain" } }),
    "POST /evidence-requests/r-1/approve": { ...req, status: "approved" },
  });
}

test.each(["en", "ja"] as const)("re-request view is axe-clean with an open draft (%s)", async (lang) => {
  const { api } = rrApi();
  const { container } = renderWithSession(<ReRequestView />, { api, lang });
  fireEvent.click(await screen.findByText("RR-0001"));
  await screen.findByRole("region", { name: lang === "ja" ? "依頼書本文" : "Request letter" });
  expect(await axeViolations(container)).toEqual([]);
});

test("re-request shows only the stale document, drafts, switches language, downloads and approves", async () => {
  const { api, calls } = rrApi();
  renderWithSession(<ReRequestView />, { api });
  expect(await screen.findByText("ir-1.pdf")).toBeTruthy();
  expect(screen.getAllByText("1 of 2 documents").length).toBeGreaterThan(0);
  expect(screen.getByText(/1 affected decisions need internal review only/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Create request drafts" }));
  await screen.findByText("1 drafts created.");
  const letter = await screen.findByRole("region", { name: "Request letter" });
  expect(letter.textContent).toContain("To: NAGOYA FORGE");
  fireEvent.click(screen.getByRole("button", { name: "日本語" }));
  expect(screen.getByRole("region", { name: "Request letter" }).textContent).toContain("御中");
  expect(screen.getByText(/needs approval by a second reviewer/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Download (.txt)" }));
  await waitFor(() => expect(calls.some((c) => c.path === "/evidence-requests/r-1/document?lang=ja")).toBe(true));
  fireEvent.click(screen.getByRole("button", { name: "Approve for sending" }));
  await screen.findByText(/Request approved/);
  expect(calls.find((c) => c.path === "/evidence-requests/r-1/approve")?.body).toEqual({ comment: null });
  expect(screen.queryByRole("button", { name: "Approve for sending" })).toBeNull();
});

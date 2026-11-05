import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { traceGraph, TraceView } from "./DecisionsView";
import { GraphSvg, highlighted } from "./graph";
import { axeViolations, fakeApi, renderWithSession } from "./test-utils";
import type { Claim, Trace } from "./types";
import { simPreview, WhatIfPanel, type SimResult } from "./WhatIfPanel";

const claim: Claim = {
  id: "c-len",
  subject_type: "part",
  subject_key: "brk-1",
  predicate: "mfg.dimension.length",
  value_text: null,
  value_num: "100",
  value_unit: "mm",
  revision: "B",
  scope: {},
  artifact_id: "a-dw",
  artifact_display_name: "dm-1.pdf",
  source_location: { page: 1 },
  extraction_method: "rules:mfg-map@3",
  confidence_bps: 9000,
  review_status: "unreviewed",
  recorded_at: "2026-11-01T00:00:00Z",
  superseded_at: null,
};

const empty = { appear_total: 0, clear_total: 0, changed_total: 0, appear: [], clear: [], changed: [] };

const result: SimResult = {
  simulation: "whatif@1",
  read_only: true,
  change: { type: "supersede", before: [claim], before_total: 1, after: { ...claim, id: "", value_num: "101" }, simulated_claim_id: "sim" },
  decisions: {
    affected_total: 2,
    material_total: 2,
    newly_flagged_total: 2,
    bounded: false,
    reached: null,
    limit: 250000,
    review_batch: false,
    truncated: false,
    sample: [
      {
        decision_id: "d-a",
        kind: "po_line_acceptance",
        title: "Accept BRK-1",
        subject_key: "brk-1",
        status_before: "valid",
        status_after: "potentially_affected",
        material: true,
        cause_kind: "superseded",
        reason: "evidence corrected: change exceeds threshold 0.5",
        path: ["c-len", "sim", "d-a"],
        semantic_change: { from: "100 mm", to: "101 mm" },
      },
      {
        decision_id: "d-b",
        kind: "po_line_acceptance",
        title: "Ship BRK-1",
        subject_key: "brk-1",
        status_before: "valid",
        status_after: "potentially_affected",
        material: true,
        cause_kind: "upstream_decision",
        reason: "depends on potentially affected decision d-a",
        path: ["c-len", "sim", "d-a", "d-b"],
        semantic_change: { upstream_decision: "d-a" },
      },
    ],
  },
  matching: {
    ...empty,
    appear_total: 1,
    appear: [{ purchase_order: "po-1", po_line: "1", part: "brk-1", kind: "measurement_out_of_tolerance", severity: "critical" }],
  },
  integrity: {
    signals: { ...empty, clear_total: 1, clear: [{ fingerprint: "f1", supplier: "acme", why: "dates out of order", why_ja: "日付の順序が不自然です" }] },
    drift: empty,
  },
  resupply: { suppliers: [{ supplier: "ACME", documents: [{ doc: "a-ir", name: "ir-1.pdf" }], baseline_count: 3 }] },
  graph: {
    nodes: [
      { id: "c-len", kind: "claim", label: "mfg.dimension.length (before)", tone: "warn", mark: "changed" },
      { id: "d-a", kind: "decision", label: "Accept BRK-1", tone: "crit", mark: "affected" },
    ],
    edges: [["c-len", "d-a"]],
  },
};

afterEach(() => simPreview.set(null));

test("simulates a value change read-only and shows before/after, decisions, findings and the re-request", async () => {
  const { api, calls } = fakeApi({ "POST /simulations": result });
  const onClose = vi.fn();
  const { container } = renderWithSession(<WhatIfPanel claim={claim} onClose={onClose} />, { api });
  const num = screen.getByLabelText("New number");
  fireEvent.change(num, { target: { value: "101" } });
  fireEvent.click(screen.getByRole("button", { name: "Simulate" }));
  await screen.findByText("Accept BRK-1", { selector: "td" });
  const body = calls.find((c) => c.path === "/simulations")!.body as { change: { type: string; claim_id: string; correction: Record<string, unknown> }; record: boolean };
  expect(body.change.type).toBe("supersede");
  expect(body.change.claim_id).toBe("c-len");
  expect(body.change.correction.value_num).toBe("101");
  expect(body.change.correction.predicate).toBe("mfg.dimension.length");
  expect(body.record).toBe(false);
  const now = screen.getByRole("region", { name: "Now" });
  const after = screen.getByRole("region", { name: "After the change" });
  expect(within(now).getByText("100 mm")).toBeTruthy();
  expect(within(after).getByText("101 mm")).toBeTruthy();
  expect(screen.getByText("po-1/1 brk-1: Measured value out of tolerance")).toBeTruthy();
  expect(screen.getByText(/dates out of order/)).toBeTruthy();
  expect(screen.getByText(/ACME: 1 of 3 documents/)).toBeTruthy();
  expect(screen.getByText(/Nothing was changed/)).toBeTruthy();
  expect(simPreview.get()).toEqual(result);
  expect(await axeViolations(container)).toEqual([]);
  // Escape closes the panel and ends the preview.
  fireEvent.keyDown(screen.getByRole("button", { name: "Simulate" }), { key: "Escape" });
  expect(onClose).toHaveBeenCalled();
  expect(simPreview.get()).toBeNull();
});

test("withdrawal of a whole document, in Japanese, with an audited record", async () => {
  const { api, calls } = fakeApi({
    "POST /simulations": { ...result, change: { ...result.change, type: "withdraw", after: null }, run_id: "0190aaaa-bbbb" },
  });
  const { container } = renderWithSession(<WhatIfPanel doc={{ id: "a-ir", name: "ir-1.pdf" }} onClose={() => {}} />, { api, lang: "ja" });
  expect(screen.getByText(/ir-1.pdf の現在のすべてのクレーム/)).toBeTruthy();
  fireEvent.click(screen.getByLabelText("このシミュレーションを監査ログに記録する"));
  fireEvent.click(screen.getByRole("button", { name: "シミュレーション実行" }));
  await screen.findByText(/取り下げ — 証拠ではなくなります/);
  expect(screen.getByText(/日付の順序が不自然です/)).toBeTruthy();
  expect(screen.getByText(/監査ログに記録しました（実行 0190aaaa）/)).toBeTruthy();
  const body = calls.find((c) => c.path === "/simulations")!.body as { change: unknown; record: boolean };
  expect(body).toMatchObject({ change: { type: "withdraw", artifact_id: "a-ir" }, record: true });
  expect(await axeViolations(container)).toEqual([]);
});

test("large impacts: counts, review-batch notice and a truncated sample", async () => {
  const big: SimResult = { ...result, decisions: { ...result.decisions, affected_total: 24979, material_total: 24979, review_batch: true, truncated: true } };
  const { api } = fakeApi({ "POST /simulations": big });
  renderWithSession(<WhatIfPanel claim={claim} onClose={() => {}} />, { api });
  fireEvent.click(screen.getByRole("button", { name: "Simulate" }));
  await screen.findByText(/background review batch/);
  expect(screen.getByText(/Showing 2 of 24,979 affected decisions/)).toBeTruthy();
});

test("server refusals are shown, nothing is previewed", async () => {
  const { api } = fakeApi({ "POST /simulations": new Response(JSON.stringify({ error: "already_superseded" }), { status: 409 }) });
  renderWithSession(<WhatIfPanel claim={claim} onClose={() => {}} />, { api });
  fireEvent.click(screen.getByRole("button", { name: "Simulate" }));
  await waitFor(() => expect(document.querySelector(".notice")).toBeTruthy());
  expect(simPreview.get()).toBeNull();
});

test("the trace view highlights decisions and evidence of the active preview", async () => {
  const trace = {
    decision: {
      id: "d-a",
      kind: "po_line_acceptance",
      title: "Accept BRK-1",
      subject_type: "part",
      subject_key: "brk-1",
      outcome: "approved",
      rationale: "ok",
      effective_from: null,
      decided_by: "u",
      decided_at: "2026-11-01T00:00:00Z",
      status: "valid",
      derivation_sha256: "ab".repeat(32),
      supersedes: null,
      evidence_claim_ids: ["c-len"],
    },
    derivation_reproducible: true,
    evidence: [{ claim_id: "c-len", predicate: "mfg.dimension.length", snapshot_at_decision: { num: "100", unit: "mm" }, changed: false }],
    changed_since_decision: [],
    invalidations: [],
  } as unknown as Trace;
  const { api } = fakeApi({});
  const { container } = renderWithSession(<TraceView t={trace} onAction={() => {}} canAct={false} />, { api });
  expect(screen.queryByText(/would become/)).toBeNull();
  act(() => simPreview.set(result));
  expect(await screen.findByText(/this decision would become/)).toBeTruthy();
  // SVG fallback in jsdom: both nodes highlighted, labelled for assistive technology.
  expect(container.querySelectorAll(".g-node.hl").length).toBe(2);
  expect(screen.getByRole("button", { name: /Accept BRK-1 — Affected/ })).toBeTruthy();
  expect(await axeViolations(container)).toEqual([]);
  fireEvent.click(screen.getByRole("button", { name: "Clear preview" }));
  expect(container.querySelectorAll(".g-node.hl").length).toBe(0);
  const g = traceGraph(trace);
  expect([...highlighted(g, new Set(["d-a"]))]).toEqual(["d-a"]);
});

test("svg graph dims nodes outside the highlight", () => {
  const { api } = fakeApi({});
  const { container } = renderWithSession(<GraphSvg g={result.graph} label="impact" onPick={() => {}} />, { api });
  expect(container.querySelectorAll(".g-node.hl").length).toBe(2);
  expect(container.querySelectorAll(".g-edge.hl").length).toBe(1);
});

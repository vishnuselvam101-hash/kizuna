import { fireEvent, screen } from "@testing-library/react";
import { DriftView, IntegrityView, type DriftDetail, type SignalDetail } from "./IntegrityView";
import { axeViolations, fakeApi, renderWithSession } from "./test-utils";

const signal = {
  id: "s1",
  kind: "table_copy",
  supplier_key: "ACME STEEL",
  part_key: "BRK-300",
  characteristic: null,
  strength: "moderate",
  explanation: { why: "All 6 values on the certificates for L1003 and L1010 are identical. This pattern can have ordinary causes and is not a finding of wrongdoing.", why_ja: "L1003 と L1010 の証明書で全 6 項目の値が一致しています。不正の判断ではありません。" },
  statistic: { identical: 6, shared: 6, chance_all_pairs: 2.2e-5 },
  evidence: [{ artifact: "a1", document: "certificates_acme.csv", claim: "c1", field: "qa.measured.length", value: "120.111 mm", location: { row: 4 } }],
  status: "open",
  detected_at: "2026-11-05T01:00:00Z",
};
const signalDetail: SignalDetail = {
  signal,
  documents: [{ artifact_id: "a1", display_name: "certificates_acme.csv", sha256: "ab".repeat(32) }],
  reviews: [],
  requires_human_review: true,
  limitations: "Strength is not a probability of wrongdoing.",
};
const drift = {
  id: "d1",
  kind: "silent_change",
  drawing_key: "D-901",
  part_key: "BRK-400",
  earlier_artifact: "a1",
  later_artifact: "a2",
  earlier_revision: "B",
  later_revision: "B",
  changes: [{ predicate: "mfg.dimension.length", category: "dimension", change: "changed", before: [{ value: "120 mm", claim: "c1", location: { line: 4 } }], after: [{ value: "121 mm", claim: "c2", location: { line: 4 } }] }],
  strength: "strong",
  explanation: { why: "1 field(s) of D-901 differ between D-901_B.pdf and D-901_B_resent.pdf, but both carry revision B.", why_ja: "D-901 は同じリビジョン B ですが 1 項目が異なります。" },
  status: "open",
  detected_at: "2026-11-05T01:00:00Z",
};
const driftDetail: DriftDetail = {
  drift,
  documents: [
    { artifact_id: "a1", display_name: "D-901_B.pdf", sha256: "cd".repeat(32) },
    { artifact_id: "a2", display_name: "D-901_B_resent.pdf", sha256: "ef".repeat(32) },
  ],
  reviews: [],
  requires_human_review: true,
  potentially_affected_decisions: [{ id: "dec1", title: "Release BRK-400 rev B", status: "valid" }],
  limitations: "Compares extracted fields.",
};

function api() {
  return fakeApi({
    "GET /integrity/signals?status=open": [signal],
    "GET /integrity/signals/s1": signalDetail,
    "POST /integrity/signals/s1/review": { ...signal, status: "follow_up" },
    "GET /integrity/drift?status=open": [drift],
    "GET /integrity/drift/d1": driftDetail,
    "POST /integrity/drift/d1/review": { drift: { ...drift, status: "confirmed" }, decisions_flagged: 1 },
    "POST /integrity/run": { signals_new: 3, drift_new: 2, marked_obsolete: 0 },
  });
}

test.each(["en", "ja"] as const)("integrity signals list and detail have no axe violations (%s)", async (lang) => {
  const { api: a } = api();
  const { container } = renderWithSession(<IntegrityView />, { api: a, lang });
  fireEvent.click(await screen.findByRole("button", { name: "ACME STEEL" }));
  await screen.findByRole("heading", { name: lang === "ja" ? "根拠の箇所" : "Evidence locations" });
  expect(screen.getAllByText(lang === "ja" ? /不正の判断ではありません/ : /not a finding of wrongdoing/).length).toBeGreaterThan(0);
  expect(await axeViolations(container)).toEqual([]);
});

test.each(["en", "ja"] as const)("revision drift list and diff have no axe violations (%s)", async (lang) => {
  const { api: a } = api();
  const { container } = renderWithSession(<DriftView />, { api: a, lang });
  fireEvent.click(await screen.findByRole("button", { name: "D-901" }));
  await screen.findByText("121 mm");
  expect(screen.getByText("Release BRK-400 rev B")).toBeTruthy();
  expect(await axeViolations(container)).toEqual([]);
});

test("a signal review needs a reason and sends only allowed decisions", async () => {
  const { api: a, calls } = api();
  renderWithSession(<IntegrityView />, { api: a });
  fireEvent.click(await screen.findByRole("button", { name: "ACME STEEL" }));
  fireEvent.click(await screen.findByRole("button", { name: "Follow up with supplier" }));
  expect(await screen.findByRole("alert")).toBeTruthy();
  expect(calls.some((c) => c.method === "POST" && c.path.endsWith("/review"))).toBe(false);
  fireEvent.change(screen.getByLabelText("Reason"), { target: { value: "Ask for the L1010 test record" } });
  fireEvent.click(screen.getByRole("button", { name: "Follow up with supplier" }));
  await screen.findByText("Review recorded.");
  const post = calls.find((c) => c.method === "POST" && c.path === "/integrity/signals/s1/review");
  expect(post?.body).toEqual({ decision: "follow_up", comment: "Ask for the L1010 test record" });
  // Signals never offer a reject action.
  expect(screen.queryByRole("button", { name: /reject/i })).toBeNull();
});

test("confirming a drift reports the decisions flagged for review", async () => {
  const { api: a, calls } = api();
  renderWithSession(<DriftView />, { api: a });
  fireEvent.click(await screen.findByRole("button", { name: "D-901" }));
  fireEvent.change(await screen.findByLabelText("Reason"), { target: { value: "Length changed without a new revision" } });
  fireEvent.click(screen.getByRole("button", { name: "Confirm drift" }));
  await screen.findByText("Drift confirmed; 1 decisions flagged for review.");
  expect(calls.find((c) => c.path === "/integrity/drift/d1/review")?.body).toEqual({ decision: "confirm_drift", comment: "Length changed without a new revision" });
});

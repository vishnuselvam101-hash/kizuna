import { fireEvent, render, screen } from "@testing-library/react";
import { TraceView, type Trace } from "./DecisionsView";

const trace: Trace = {
  decision: { id: "d1", kind: "supplier_part_release", title: "Release BRK-100", subject_key: "BRK-100", outcome: "approved", status: "potentially_affected", decided_at: "2026-11-05", derivation_sha256: "ab".repeat(32), rationale: "ok" },
  derivation_reproducible: true,
  evidence: [{ claim_id: "c1", predicate: "mfg.dimension.length", snapshot_at_decision: { num: "120", unit: "mm" }, current_value: { num: "121", unit: "mm" }, changed: true }],
  invalidations: [{ reason: "evidence corrected: value changed", material: true, recorded_at: "2026-11-06", cause_kind: "superseded" }],
  changed_since_decision: [],
};

test("affected decision shows then-vs-now evidence and asks for human action", () => {
  const acts: string[] = [];
  render(<TraceView t={trace} onAction={(a) => acts.push(a)} />);
  expect(screen.getByText("120 mm")).toBeTruthy();
  expect(screen.getByText("121 mm")).toBeTruthy();
  expect(screen.getByText(/reproducible/)).toBeTruthy();
  expect(screen.getByText(/A reviewer must reconfirm or revoke/)).toBeTruthy();
  fireEvent.click(screen.getByText("Reconfirm with current evidence"));
  expect(acts).toEqual(["reconfirm"]);
});

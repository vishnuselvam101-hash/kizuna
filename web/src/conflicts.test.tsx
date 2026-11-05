import { fireEvent, render, screen } from "@testing-library/react";
import { ConflictDetailView, type ConflictDetail } from "./ConflictsView";

const claim = (id: string, v: string, pred: string) => ({
  id, subject_type: "part", subject_key: "BRK-100", predicate: pred, value_text: null, value_num: v, value_unit: "mm",
  revision: "C", artifact_id: "a1", source_location: { row: 2, col: 4 }, extraction_method: "worker:csv@1+mfg-map@1",
  confidence_bps: 9000, review_status: "unreviewed", recorded_at: "", superseded_at: null,
});

const detail: ConflictDetail = {
  conflict: { id: "k1", kind: "tolerance_violation", subject_key: "BRK-100", status: "open", explanation: { why: "measured value outside nominal ± tolerance" }, detected_at: "" },
  claim_a: claim("c1", "120.5", "qa.measured.length"),
  claim_b: claim("c2", "120", "mfg.dimension.length"),
  sources: [{ claim_id: "c1", display_name: "inspection.csv", location: { row: 2, col: 4 } }],
  reviews: [],
  requires_human_review: true,
  limitations: "Deterministic rule mfg-rules@1",
};

test("finding shows what, why, both sources, limitations and review actions", () => {
  const calls: string[][] = [];
  render(<ConflictDetailView d={detail} onReview={(d, c) => calls.push([d, c])} />);
  expect(screen.getByText(/measured value outside/)).toBeTruthy();
  expect(screen.getByText(/inspection.csv · row 2, col 4/)).toBeTruthy();
  expect(screen.getByText(/manual entry/)).toBeTruthy();
  expect(screen.getByText(/Human review required/)).toBeTruthy();
  expect(screen.getByText(/Limitations/)).toBeTruthy();
  fireEvent.change(screen.getByLabelText("review comment"), { target: { value: "deviation EDR-1" } });
  fireEvent.click(screen.getByText("Approve (waive)"));
  expect(calls).toEqual([["approve", "deviation EDR-1"]]);
});

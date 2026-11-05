import { fireEvent, render, screen, within } from "@testing-library/react";
import { DataTable } from "./DataTable";
import { claimValue } from "./format";
import { FindingDetail } from "./FindingsView";
import { en, I18nProvider, ja, translate } from "./i18n";
import { DEFAULT_SETTINGS, validateSettings } from "./IntegrationsView";
import { EvidenceTable } from "./RecordsView";
import { claim, conflictDetail } from "./fixtures";

const wrap = (ui: React.ReactNode, lang: "en" | "ja" = "en") => render(<I18nProvider initial={lang}>{ui}</I18nProvider>);

test("en and ja dictionaries have identical, non-empty keys", () => {
  expect(Object.keys(ja).sort()).toEqual(Object.keys(en).sort());
  for (const [k, v] of Object.entries(ja)) expect(v.trim(), k).not.toBe("");
  expect(translate("ja", "users.revoked", { n: 2, email: "a@b" })).toBe("a@b のセッションを2件終了しました。");
});

test("finding detail answers every DIRECTIVE §12 question", () => {
  wrap(<FindingDetail d={conflictDetail} onReview={() => {}} canReview />);
  for (const h of ["What was detected", "Why", "Supporting source", "Contradicting source", "Confidence and limitations", "Human review", "Decisions that may be affected"])
    expect(screen.getByRole("heading", { name: h })).toBeTruthy();
  const sup = screen.getByRole("region", { name: "Supporting source" });
  expect(within(sup).getByText("S355")).toBeTruthy();
  expect(within(sup).getByText("BRK-100 spec rev C.pdf")).toBeTruthy();
  const con = screen.getByRole("region", { name: "Contradicting source" });
  expect(within(con).getByText("S275")).toBeTruthy();
  expect(within(con).getByText("row 14, col 3")).toBeTruthy();
  expect(screen.getByText(/Lowest source confidence: 70%/)).toBeTruthy();
  expect(screen.getByText(/Deterministic rule v1/)).toBeTruthy();
  expect(screen.getByText(/Required\. Nothing proceeds/)).toBeTruthy();
  expect(screen.getByText("Release BRK-100 from Supplier A")).toBeTruthy();
});

test("finding without a second source says so instead of hiding the section", () => {
  wrap(<FindingDetail d={{ ...conflictDetail, claim_b: null, potentially_affected_decisions: [] }} onReview={() => {}} canReview={false} />, "ja");
  expect(screen.getByRole("heading", { name: "矛盾する出典" })).toBeTruthy();
  expect(screen.getByText(/第2の出典はありません/)).toBeTruthy();
  expect(screen.getByText("これらのクレームに依拠する判断は記録されていません。")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "証拠を却下" })).toBeNull();
});

test("approve and reject need a reason; request-evidence does not", () => {
  const onReview = vi.fn();
  wrap(<FindingDetail d={conflictDetail} onReview={onReview} canReview />);
  fireEvent.click(screen.getByRole("button", { name: "Reject evidence" }));
  expect(onReview).not.toHaveBeenCalled();
  expect(screen.getAllByRole("alert").map((a) => a.textContent).join(" ")).toMatch(/reason is required/);
  fireEvent.click(screen.getByRole("button", { name: "Request more evidence" }));
  expect(onReview).toHaveBeenLastCalledWith("request_evidence", "", []);
  fireEvent.change(screen.getByLabelText("Reason"), { target: { value: "PO is outdated" } });
  fireEvent.click(screen.getByRole("button", { name: "Approve (documented waiver)" }));
  expect(onReview).toHaveBeenLastCalledWith("approve", "PO is outdated", []);
});

test("rejecting a finding requires naming the faulty evidence and sends it", () => {
  const onReview = vi.fn();
  wrap(<FindingDetail d={conflictDetail} onReview={onReview} canReview />);
  fireEvent.change(screen.getByLabelText("Reason"), { target: { value: "PO superseded by rev D" } });
  fireEvent.click(screen.getByRole("button", { name: "Reject evidence" }));
  expect(onReview).not.toHaveBeenCalled();
  expect(screen.getByRole("alert").textContent).toMatch(/evidence you tick is rejected/);
  const group = screen.getByRole("group", { name: /Which evidence is wrong/ });
  fireEvent.click(within(group).getByRole("checkbox", { name: /Contradicting source/ }));
  fireEvent.click(screen.getByRole("button", { name: "Reject evidence" }));
  expect(onReview).toHaveBeenLastCalledWith("reject", "PO superseded by rev D", ["c-b"]);
});

test("evidence table shows disagreeing claims with source, method and review state", () => {
  wrap(<EvidenceTable groups={[{ predicate: "mfg.material", current: [claim("a", "S355"), claim("b", "S275")], superseded_count: 1, disagreement: true }]} />);
  expect(screen.getByText("S355")).toBeTruthy();
  expect(screen.getByText("sources disagree")).toBeTruthy();
  expect(screen.getAllByText("Unreviewed")).toHaveLength(2);
  expect(screen.getAllByText(/page 2/)).toHaveLength(2);
  // Source name comes from the claim itself (artifact_display_name), no /artifacts join.
  expect(screen.getAllByText(/BRK-100 spec rev C\.pdf/)).toHaveLength(2);
  expect(claimValue(claim("x", "", { value_text: null, value_num: "12.5", value_unit: "mm" }))).toBe("12.5 mm");
});

type Row = { id: string; name: string; n: number };
const rows = (k: number): Row[] => Array.from({ length: k }, (_, i) => ({ id: `r${i}`, name: `item-${String(i).padStart(3, "0")}`, n: k - i }));
const cols = [
  { key: "name", label: "Name", value: (r: Row) => r.name, render: (r: Row) => r.name },
  { key: "n", label: "N", value: (r: Row) => r.n, render: (r: Row) => r.n },
];

test("data table sorts, filters and supports j/k + Enter", () => {
  const onActivate = vi.fn();
  wrap(<DataTable label="Things" rows={rows(8)} cols={cols} rowKey={(r) => r.id} onActivate={onActivate} />);
  const nHeader = screen.getAllByRole("columnheader")[1];
  fireEvent.click(within(nHeader).getByRole("button"));
  expect(nHeader.getAttribute("aria-sort")).toBe("ascending");
  expect(screen.getAllByRole("row")[1].textContent).toContain("item-007");
  const region = screen.getByRole("region", { name: /Things/ });
  fireEvent.keyDown(region, { key: "j" });
  fireEvent.keyDown(region, { key: "j" });
  fireEvent.keyDown(region, { key: "k" });
  fireEvent.keyDown(region, { key: "Enter" });
  expect(onActivate).toHaveBeenCalledWith(expect.objectContaining({ name: "item-006" }));
  fireEvent.change(screen.getByRole("searchbox"), { target: { value: "item-003" } });
  expect(screen.getAllByRole("row")).toHaveLength(2);
  expect(screen.getByText("1 of 8")).toBeTruthy();
});

test("data table windows rendering above 200 rows", () => {
  wrap(<DataTable label="Many" rows={rows(5000)} cols={cols} rowKey={(r) => r.id} />);
  const rendered = screen.getAllByRole("row").length;
  expect(rendered).toBeLessThan(60);
  expect(screen.getByRole("table").getAttribute("aria-rowcount")).toBe("5001");
});

test("import settings validation rejects out-of-range values and empty required mappings", () => {
  expect(validateSettings(DEFAULT_SETTINGS)).toEqual([]);
  const bad = { ...DEFAULT_SETTINGS, pdf: { ...DEFAULT_SETTINGS.pdf, max_pages: 9999 }, mapping: { ...DEFAULT_SETTINGS.mapping, value: " " } };
  expect(validateSettings(bad)).toEqual(["pdf.max_pages", "mapping.value"]);
});

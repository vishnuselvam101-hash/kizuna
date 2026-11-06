import { fireEvent, screen } from "@testing-library/react";
import { DebtView, type DebtItems } from "./DebtView";
import { axeViolations, fakeApi, renderWithSession } from "./test-utils";

const items: DebtItems = {
  version: "mfg-debt@1",
  as_of: "2026-11-06T00:00:00Z",
  kinds: ["missing_required", "unconfirmed", "superseded", "re_requested", "expired"],
  by_kind: { missing_required: 1, unconfirmed: 1 },
  rows: [
    {
      kind: "missing_required",
      target: { type: "requirement", name: "inspection_result" },
      label: "inspection_result",
      detail: { any_of: ["qa.result", "qa.measured."] },
      since: "2026-10-01T00:00:00Z",
      age_days: 36,
      blocks: 1,
      decisions: [{ id: "d1", title: "Release BRK-1", kind: "part_release", part: "BRK-1", supplier: "ACME" }],
      parts: ["BRK-1"],
      suppliers: ["ACME"],
    },
    {
      kind: "unconfirmed",
      target: { type: "claim", id: "c1" },
      label: "qa.material",
      detail: { predicate: "qa.material" },
      since: "2026-10-20T00:00:00Z",
      age_days: 17,
      blocks: 2,
      decisions: [
        { id: "d2", title: "Release SH-9 lot 1", kind: "part_release", part: "SH-9", supplier: "KANTO" },
        { id: "d3", title: "Release SH-9 lot 2", kind: "part_release", part: "SH-9", supplier: "KANTO" },
      ],
      parts: ["SH-9"],
      suppliers: ["KANTO"],
    },
  ],
  total_rows: 2,
  limitations: ["debt.limit.obsolete_untimed"],
};

function api() {
  return fakeApi({
    "GET /evidence-debt/items?limit=500": items,
    "GET /evidence-debt/items?limit=500&kind=superseded": { ...items, rows: [], total_rows: 0 },
    "GET /evidence-debt/items?limit=500&as_of=2026-10-01": { ...items, rows: [], total_rows: 0 },
    "GET /evidence-debt": { totals: { decisions: 3, with_debt: 3 } },
    "GET /evidence-debt?as_of=2026-10-01": { totals: { decisions: 1, with_debt: 0 } },
  });
}

test.each(["en", "ja"] as const)("evidence debt list and detail have no axe violations (%s)", async (lang) => {
  const { api: a } = api();
  const { container } = renderWithSession(<DebtView />, { api: a, lang });
  fireEvent.click(await screen.findByRole("button", { name: "qa.material" }));
  expect(await screen.findByText("Release SH-9 lot 2")).toBeTruthy();
  expect(screen.getByRole("button", { name: lang === "ja" ? "検査結果" : "Inspection result" })).toBeTruthy();
  expect(await axeViolations(container)).toEqual([]);
});

test("filters by kind and date and never writes", async () => {
  const { api: a, calls } = api();
  renderWithSession(<DebtView />, { api: a });
  await screen.findByText("3 accepted decisions, 3 with evidence debt.");
  fireEvent.change(screen.getByLabelText("Debt"), { target: { value: "superseded" } });
  await screen.findByText("No evidence debt on this date.");
  fireEvent.change(screen.getByLabelText("Debt"), { target: { value: "" } });
  fireEvent.change(screen.getByLabelText("As known on"), { target: { value: "2026-10-01" } });
  await screen.findByText("1 accepted decisions, 0 with evidence debt.");
  expect(calls.every((c) => c.method === "GET")).toBe(true);
});

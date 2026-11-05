import { render, screen } from "@testing-library/react";
import { App } from "./App";
import { CompareView } from "./CompareView";
import { DashboardView } from "./DashboardView";
import { DecisionsView } from "./DecisionsView";
import { FindingsView } from "./FindingsView";
import { artifact, claim, conflict, dashboardSummary, subjects, decision, users } from "./fixtures";
import { I18nProvider } from "./i18n";
import { IntegrationsView } from "./IntegrationsView";
import { RecordsView } from "./RecordsView";
import { ReportsView } from "./ReportsView";
import { UsersView } from "./UsersView";
import { axeViolations, fakeApi, renderWithSession } from "./test-utils";

const { api } = fakeApi({
  "GET /artifacts": [artifact],
  "GET /claims": [claim("c-a", "S355")],
  "GET /conflicts?status=open": [conflict],
  "GET /dashboard/summary": dashboardSummary,
  "GET /subjects": subjects,
  "GET /evidence/part/BRK-100": { predicates: [] },
  "GET /decisions": [decision],
  "GET /audit/verify": { intact: true, events_checked: 1, problem: null, head_seq: 1, head_hash: "aa" },
  "GET /users": users,
  "GET /audit/events?limit=200": [],
  "GET /entities/candidates": { candidates: [{ left: "BRK-100", right: "BRK100", outcome: "possible_match", reasons: ["separator_difference"] }] },
});

test.each(["en", "ja"] as const)("login screen has no axe violations (%s)", async (lang) => {
  const { container } = render(<App api={api} lang={lang} />);
  expect(await axeViolations(container)).toEqual([]);
  expect(document.documentElement.lang).toBe(lang);
});

const screens: [string, () => React.ReactNode, string | RegExp][] = [
  ["dashboard", () => <DashboardView onNavigate={() => {}} />, "Evidence graph"],
  ["records", () => <RecordsView />, "Possible match"],
  ["findings", () => <FindingsView />, "BRK-100"],
  ["decisions", () => <DecisionsView />, "Release BRK-100 from Supplier A"],
  ["compare", () => <CompareView />, "Compare"],
  ["reports", () => <ReportsView />, "Organisation audit report"],
  ["users", () => <UsersView />, "rev@example.test"],
  ["integrations", () => <IntegrationsView />, "Column mapping"],
];

test.each(screens)("%s has no axe violations", async (_n, ui, ready) => {
  const { container } = renderWithSession(ui(), { api });
  await screen.findAllByText(ready);
  expect(await axeViolations(container)).toEqual([]);
});

test("Japanese UI renders without English leftovers in the chrome", async () => {
  render(
    <I18nProvider initial="ja">
      <App api={api} lang="ja" />
    </I18nProvider>,
  );
  expect(screen.getByRole("button", { name: "サインイン" })).toBeTruthy();
  expect(screen.getByLabelText("組織")).toBeTruthy();
});

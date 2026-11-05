import { render, screen } from "@testing-library/react";
import { AiExtractionPanel } from "./AiExtractionPanel";
import { App } from "./App";
import { DetectionSettingsPanel } from "./DetectionSettingsPanel";
import { CompareView } from "./CompareView";
import { DashboardView } from "./DashboardView";
import { DecisionsView } from "./DecisionsView";
import { FindingsView } from "./FindingsView";
import { aiExtraction, artifact, claim, conflict, dashboardSummary, detectionSettings, subjects, decision, users } from "./fixtures";
import { I18nProvider } from "./i18n";
import { IntegrationsView } from "./IntegrationsView";
import { RecordsView } from "./RecordsView";
import { SecurityView } from "./SecurityView";
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
  "GET /integrations/import-settings": { version: 0, settings: {} },
  "GET /integrations/ai-extraction": aiExtraction(),
  "GET /settings/timezone": { version: 0, timezone: "Asia/Tokyo", default: "Asia/Tokyo", today: "2026-11-05" },
  "GET /security/summary": { open_incidents: 0, actions_awaiting_approval: 0, events_24h: {}, active_ip_blocks: [] },
  "GET /security/incidents": [],
  "GET /security/detection-settings": detectionSettings(),
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
  ["integrations: AI-assisted extraction", () => <IntegrationsView />, "extract-small-1"],
  ["security: detection thresholds", () => <SecurityView />, "Failed sign-ins before lock"],
];

test.each(screens)("%s has no axe violations", async (_n, ui, ready) => {
  const { container } = renderWithSession(ui(), { api });
  await screen.findAllByText(ready);
  expect(await axeViolations(container)).toEqual([]);
}, 30_000); // integrations renders ~400 time-zone options; axe needs longer than the 5 s default on a busy host

test("Japanese UI renders without English leftovers in the chrome", async () => {
  render(
    <I18nProvider initial="ja">
      <App api={api} lang="ja" />
    </I18nProvider>,
  );
  expect(screen.getByRole("button", { name: "サインイン" })).toBeTruthy();
  expect(screen.getByLabelText("組織")).toBeTruthy();
});

test("AI extraction (not configured, enabled) and adaptive thresholds have no axe violations", async () => {
  const { api: a2 } = fakeApi({
    "GET /integrations/ai-extraction": aiExtraction({ provider: { configured: false }, settings: { enabled: true, notice_version: "x" } }),
    "GET /security/detection-settings": detectionSettings({ effective: { ...detectionSettings().effective, mode: "adaptive" } }),
  });
  const { container } = renderWithSession(
    <>
      <AiExtractionPanel />
      <DetectionSettingsPanel />
    </>,
    { api: a2, lang: "ja" },
  );
  await screen.findByText(/運用者により未設定/);
  await screen.findByLabelText("最大倍率");
  expect(await axeViolations(container)).toEqual([]);
});

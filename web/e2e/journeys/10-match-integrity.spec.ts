// What-if simulation of an evidence change, 3-way match (PO ↔ drawing ↔ certificate) with an acceptance decision,
// and certificate integrity signals (a mill sheet whose chemistry cannot add up) reviewed by a person.
import { expect, test } from "@playwright/test";
import { axe, ctx, go, ja, login, logout, noHorizontalScroll, row, shot, status, upload } from "../helpers";

test.describe.configure({ mode: "serial" });
const TITLE = "JA-BRK-1001 量産承認（改訂B）";

test("what-if: simulating a material change shows the decision it would affect, and changes nothing", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "reviewer");
  await go(page, c, "decisions", "decisions");
  const before = (await row(page, TITLE).first().textContent()) ?? "";
  await go(page, c, "evidence", "records");
  await page.getByLabel(c.t("records.filter")).fill("JA-BRK-1001");
  await page.getByRole("button", { name: /JA-BRK-1001/ }).click();
  const ev = page.getByRole("region", { name: c.t("records.evidenceFor", { key: "JA-BRK-1001" }), exact: true });
  const pre = c.t("claim.reviewFor", { predicate: "mfg.material", value: "§" }).split("§")[0];
  await ev.getByRole("row").filter({ hasText: "仕様書_JA-BRK-1001.xlsx" }).locator(`button[aria-label^="${pre}"]`).first().click();
  await page.getByRole("button", { name: c.t("whatif.open"), exact: true }).click();
  const sim = page.getByRole("region", { name: c.t("whatif.title"), exact: true });
  await expect(sim.getByRole("radio", { name: c.t("whatif.mode.supersede") })).toBeChecked();
  await sim.getByLabel(c.t("whatif.valueText")).fill("SUS316");
  await sim.getByLabel(c.t("whatif.reason")).fill("材質変更の影響確認");
  await sim.getByRole("button", { name: c.t("whatif.run"), exact: true }).click();
  await expect(sim.getByText(c.t("whatif.readOnly"))).toBeVisible();
  await expect(sim.getByRole("region", { name: c.t("whatif.after"), exact: true })).toContainText("SUS316");
  await expect(sim.getByRole("row").filter({ hasText: TITLE })).toBeVisible();
  await axe(page, "what-if result");
  await noHorizontalScroll(page, "what-if result");
  await shot(page, info, "whatif");
  // Escape closes the simulation; the real decision is untouched.
  await sim.getByRole("button", { name: c.t("whatif.run"), exact: true }).focus();
  await page.keyboard.press("Escape");
  await expect(sim).toHaveCount(0);
  await go(page, c, "decisions", "decisions");
  await expect(row(page, TITLE).first()).toHaveText(before);
});

test("3-way match: a PO line with blocking findings cannot be approved; a reasoned rejection is recorded", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "reviewer");
  await go(page, c, "conflicts", "match");
  const lines = page.getByRole("region", { name: c.t("match.lines"), exact: true });
  await lines.getByRole("row").filter({ hasText: "PO-J-7002" }).first().click();
  const detail = page.getByRole("article").filter({ has: page.locator("#match-h") });
  await expect(detail.locator("#match-h")).toContainText("PO-J-7002");
  for (const side of ["po", "drawing", "certificate"]) await expect(detail.getByRole("columnheader", { name: c.t(`match.side.${side}`), exact: true })).toBeVisible();
  await expect(detail.getByRole("heading", { level: 3, name: c.t("match.findings", { n: "§" }).split("§")[0], exact: false })).toBeVisible();
  const outcome = detail.getByRole("combobox", { name: c.t("decisions.outcome"), exact: true });
  await expect(outcome.locator("option[value=approved]")).toHaveAttribute("disabled", "");
  await outcome.selectOption("rejected");
  await detail.getByRole("button", { name: c.t("match.acceptBtn"), exact: true }).click();
  await expect(detail.getByRole("textbox", { name: c.t("decisions.rationale"), exact: true })).toHaveAttribute("aria-invalid", "true");
  await detail.getByRole("textbox", { name: c.t("decisions.rationale"), exact: true }).fill("ミルシートの材質が図面と不一致。ロット返品。");
  await detail.getByRole("button", { name: c.t("match.acceptBtn"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("match.accepted"));
  await expect(lines.getByRole("row").filter({ hasText: "PO-J-7002" }).first()).toContainText(c.tv("decision.status", "valid"));
  await axe(page, "3-way match");
  await noHorizontalScroll(page, "3-way match");
  await shot(page, info, "match");
});

test("integrity signal: a mill sheet whose chemistry exceeds 100 % is flagged for a person, never rejected", async ({ page }, info) => {
  const c = ctx(info);
  const file = "ミルシート_JA-BRK-1001_L2402.pdf";
  await login(page, c, "analyst");
  await go(page, c, "evidence", "documents");
  await upload(page, c, ja(file), "inspection_report");
  await expect(status(page)).toContainText(c.tv("artifact.status", "accepted"));
  await page.getByRole("button", { name: c.t("docs.extractFor", { name: file }), exact: true }).click();
  await expect(status(page)).toContainText(c.t("docs.extracted", { n: "§", labels: "§" }).split("§")[1].trim());
  await logout(page, c);

  await login(page, c, "reviewer");
  await go(page, c, "conflicts", "integrity");
  await page.getByRole("button", { name: c.t("integrity.run"), exact: true }).click();
  await expect(status(page)).toContainText(c.t("integrity.ran", { signals: "§", drift: "§", obsolete: "§" }).split("§")[0].trim());
  const list = page.getByRole("region", { name: c.t("integrity.list"), exact: true });
  const sig = list.getByRole("row").filter({ hasText: c.tv("integrity.kind", "chemistry_sum") });
  await expect(sig).toHaveCount(1);
  await sig.click();
  const d = page.getByRole("article").filter({ hasText: file });
  await expect(d).toContainText("100.8");
  await expect(d).toContainText(c.t("integrity.neverReject"));
  await d.getByRole("button", { name: c.t("integrity.followUp"), exact: true }).click();
  await expect(d.getByRole("alert")).toHaveText(c.t("finding.commentHint"));
  await d.getByRole("textbox", { name: c.t("finding.comment") }).fill("Ni 80.50 は 8.05 の転記誤りの可能性。仕入先に原本を確認。");
  await d.getByRole("button", { name: c.t("integrity.followUp"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("integrity.reviewed"));
  await page.getByLabel(c.t("findings.status")).selectOption("follow_up");
  await expect(list.getByRole("row").filter({ hasText: c.tv("integrity.kind", "chemistry_sum") })).toHaveCount(1);
  // The certificate's claims stay evidence: a signal never rejects anything by itself.
  await go(page, c, "evidence", "records");
  await page.getByLabel(c.t("records.filter")).fill("JA-BRK-1001");
  await page.getByRole("button", { name: /JA-BRK-1001/ }).click();
  const ev = page.getByRole("region", { name: c.t("records.evidenceFor", { key: "JA-BRK-1001" }), exact: true });
  await expect(ev.getByRole("row").filter({ hasText: file }).filter({ hasText: "80.5" }).first()).not.toContainText(c.tv("review", "rejected"));
  await axe(page, "integrity evidence");
  await shot(page, info, "integrity");
});

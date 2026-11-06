// Conflict detection and review: approve (waiver), reject (naming the faulty evidence), request evidence,
// and the four-eyes rule (whoever uploaded the evidence may not review the finding).
import { expect, test, type Page } from "@playwright/test";
import { alert, axe, ctx, go, login, noEnglishSentences, noHorizontalScroll, shot, status, type Ctx } from "../helpers";

test.describe.configure({ mode: "serial" });

async function openFinding(page: Page, c: Ctx, kind: string, key: string) {
  const list = page.getByRole("region", { name: c.t("findings.list"), exact: true });
  await list.getByRole("row").filter({ hasText: c.tv("conflict.kind", kind) }).filter({ hasText: key }).first().getByRole("button", { name: key }).click();
  await expect(page.getByRole("heading", { level: 2, name: new RegExp(key) })).toBeVisible();
}

test("analyst runs detection: the mill sheet's SUS316 contradicts the specified SUS304", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "analyst");
  await go(page, c, "conflicts", "findings");
  await page.getByRole("button", { name: c.t("findings.run"), exact: true }).click();
  await expect(status(page)).toContainText(c.t("findings.ran", { n: "§", obsolete: "§" }).split("§")[1].trim());
  await openFinding(page, c, "value_mismatch", "JA-BRK-1001");
  const f = page.locator("article.finding");
  await expect(f).toContainText("SUS316");
  await expect(f).toContainText("SUS304");
  await expect(f).toContainText("ミルシート_JA-BRK-1001.pdf");
  await noEnglishSentences(page, c, "findings");
  // Analysts may run detection but not decide reviews.
  await expect(page.getByRole("button", { name: c.t("finding.reject"), exact: true })).toHaveCount(0);
  await axe(page, "finding detail");
  await noHorizontalScroll(page, "finding detail");
  await shot(page, info, "finding");
});

test("four-eyes: reviewer2 uploaded the mill sheet and is refused when reviewing that finding", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "reviewer2");
  await go(page, c, "conflicts", "findings");
  await openFinding(page, c, "value_mismatch", "JA-BRK-1001");
  await page.getByRole("textbox", { name: c.t("finding.comment") }).fill("Looks fine to me");
  await page.getByRole("button", { name: c.t("finding.approve"), exact: true }).click();
  await expect(alert(page)).toHaveText(c.t("err.no_self_review"));
});

test("reviewer: reject needs a reason and the faulty evidence; request evidence; approve a waiver", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "reviewer");
  await go(page, c, "conflicts", "findings");
  await openFinding(page, c, "value_mismatch", "JA-BRK-1001");
  await page.getByRole("button", { name: c.t("finding.reject"), exact: true }).click();
  await expect(page.locator("#f-comment-hint")).toHaveAttribute("role", "alert");
  await page.getByRole("textbox", { name: c.t("finding.comment") }).fill("ミルシートの材質がSUS316。仕様はSUS304。ロット差し戻し。");
  await page.getByRole("button", { name: c.t("finding.reject"), exact: true }).click();
  await expect(page.getByRole("alert").filter({ hasText: c.t("finding.faultyNeeded") })).toBeVisible();
  await page.getByRole("checkbox", { name: /SUS316/ }).check();
  await page.getByRole("button", { name: c.t("finding.reject"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("findings.reviewed"));
  await expect(page.locator("article.finding")).toContainText(c.tv("conflict.status", "resolved_rejected"));

  await openFinding(page, c, "revision_mismatch", "JA-BRK-1001");
  await page.getByRole("button", { name: c.t("finding.requestEvidence"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("findings.reviewed"));
  await expect(page.locator("article.finding")).toContainText(c.tv("conflict.status", "needs_evidence"));

  await openFinding(page, c, "missing_approval", "JA-BRK-1001");
  await page.getByRole("textbox", { name: c.t("finding.comment") }).fill("旧版Aの発注は取消済み（購買部確認）。");
  await page.getByRole("button", { name: c.t("finding.approve"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("findings.reviewed"));
  await page.getByLabel(c.t("findings.status")).selectOption("resolved_approved");
  await expect(page.getByRole("region", { name: c.t("findings.list"), exact: true })).toContainText("JA-BRK-1001");
});

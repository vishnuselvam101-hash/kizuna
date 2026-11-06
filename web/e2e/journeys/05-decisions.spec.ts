// Revision comparison, then the decision lifecycle: record a decision citing evidence → evidence is superseded →
// the decision appears under affected decisions → reviewer re-reviews (reconfirm) → decision trace.
import { expect, test, type Page } from "@playwright/test";
import { alert, axe, ctx, go, ja, login, noHorizontalScroll, row, shot, status, upload, type Ctx } from "../helpers";

test.describe.configure({ mode: "serial" });
const TITLE = "JA-BRK-1001 量産承認（改訂B）";

async function evidenceRow(page: Page, c: Ctx, predicate: string, doc: string) {
  await go(page, c, "evidence", "records");
  await page.getByLabel(c.t("records.filter")).fill("JA-BRK-1001");
  await page.getByRole("button", { name: /JA-BRK-1001/ }).click();
  const ev = page.getByRole("region", { name: c.t("records.evidenceFor", { key: "JA-BRK-1001" }), exact: true });
  // Rows of one field are recognised by their review button, which names the field and value.
  const pre = c.t("claim.reviewFor", { predicate, value: "§" }).split("§")[0];
  const r = ev
    .getByRole("row")
    .filter({ hasText: doc })
    .filter({ has: page.locator(`button[aria-label^="${pre}"]`) })
    .first();
  await expect(r).toBeVisible();
  return r;
}

test("revision comparison: rev B → C of JA-BRK-1001 shows the changed length", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "analyst");
  await go(page, c, "evidence", "documents");
  await upload(page, c, ja("補足仕様_JA-BRK-1001.pdf"), "spec_document");
  await expect(status(page)).toContainText(c.tv("artifact.status", "accepted"));
  await page.getByRole("button", { name: c.t("docs.extractFor", { name: "補足仕様_JA-BRK-1001.pdf" }), exact: true }).click();
  await expect(status(page)).toContainText(c.t("docs.extracted", { n: "§", labels: "§" }).split("§")[1].trim());
  await go(page, c, "evidence", "revisions");
  await page.getByLabel(c.t("compare.key")).fill("JA-BRK-1001");
  await page.getByLabel(c.t("compare.from")).fill("B");
  await page.getByLabel(c.t("compare.to")).fill("C");
  await page.getByRole("button", { name: c.t("compare.run"), exact: true }).click();
  const len = row(page, "mfg.dimension.length");
  await expect(len).toContainText("120");
  await expect(len).toContainText("125");
  await expect(len).toContainText(c.tv("change", "changed"));
  await axe(page, "revision comparison");
  await noHorizontalScroll(page, "revision comparison");
  await shot(page, info, "compare");
});

test("reviewer records a decision citing claim IDs copied from the evidence table", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "reviewer");
  const ids: string[] = [];
  for (const p of ["mfg.material", "mfg.dimension.length"]) {
    const r = await evidenceRow(page, c, p, "仕様書_JA-BRK-1001.xlsx");
    const id = (await r.locator(".claim-id code").getAttribute("title"))!;
    expect(id).toMatch(/^[0-9a-f-]{36}$/);
    ids.push(id);
  }
  await go(page, c, "decisions", "decisions");
  await page.locator("summary", { hasText: c.t("decisions.record") }).click();
  await page.getByLabel(c.t("decisions.title")).fill(TITLE);
  await page.getByRole("textbox", { name: c.t("compare.part"), exact: true }).fill("JA-BRK-1001");
  await page.getByLabel(c.t("decisions.kind")).selectOption("first_article_approval");
  await page.getByLabel(c.t("decisions.rationale")).fill("初品検査合格、材質SUS304・全長120mmを仕様書で確認。");
  // Evidence is picked from the record's claims; the copied claim ID finds each one.
  const picker = page.getByRole("group", { name: c.t("decisions.evidenceIds"), exact: true });
  for (const id of ids) {
    await picker.getByLabel(c.t("picker.search")).fill(id);
    const items = picker.getByRole("listitem");
    await expect(items).toHaveCount(1);
    await expect(items.locator("code")).toHaveAttribute("title", id);
    await items.getByRole("checkbox").check();
  }
  await expect(picker).toContainText(c.t("picker.selected", { n: ids.length }));
  await page.getByRole("button", { name: c.t("decisions.recordBtn"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("decisions.recorded"));
  await expect(row(page, TITLE)).toContainText(c.tv("decision.status", "valid"));
  await axe(page, "decisions");
});

test("analyst supersedes the cited length; the decision becomes potentially affected", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "analyst");
  const r = await evidenceRow(page, c, "mfg.dimension.length", "仕様書_JA-BRK-1001.xlsx");
  await r.locator("button", { hasText: c.t("evidence.supersede") }).click();
  await page.getByLabel(c.t("evidence.newValue")).fill("121");
  await page.getByLabel(c.t("evidence.reason")).fill("仕様書の転記誤り。原図では121 mm。");
  await page.getByRole("button", { name: c.t("evidence.supersedeBtn"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("evidence.supersededMsg"));
  await go(page, c, "decisions", "affected");
  await expect(row(page, TITLE)).toContainText(c.tv("decision.status", "potentially_affected"));
});

test("four-eyes: the decision's author cannot reconfirm it; a second reviewer re-reviews from the trace", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "reviewer");
  await go(page, c, "decisions", "affected");
  await page.getByRole("button", { name: TITLE }).click();
  await page.locator("article.trace").getByRole("textbox", { name: c.t("finding.comment") }).fill("自分で確認しました。");
  await page.locator("article.trace").getByRole("button", { name: c.t("trace.reconfirm"), exact: true }).click();
  await expect(alert(page)).toHaveText(c.t("err.no_self_review"));
  await page.getByRole("button", { name: c.t("shell.signOut"), exact: true }).click();
  await login(page, c, "reviewer2");
  await go(page, c, "decisions", "affected");
  await page.getByRole("button", { name: TITLE }).click();
  const trace = page.locator("article.trace");
  await expect(trace).toContainText(c.t("trace.mustReview"));
  const changed = trace.getByRole("row").filter({ hasText: "mfg.dimension.length" });
  await expect(changed).toContainText("120");
  await expect(changed).toContainText("121");
  await expect(changed).toContainText(c.t("common.yes"));
  await axe(page, "decision trace (affected)");
  await shot(page, info, "trace-affected");
  await trace.getByRole("textbox", { name: c.t("finding.comment") }).fill("121 mmでも組付け公差内。設計部確認済み。");
  await trace.getByRole("button", { name: c.t("trace.reconfirm"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("trace.reconfirmed"));
  await expect(page.locator("article.trace")).toContainText(c.tv("decision.status", "valid"));
  await go(page, c, "decisions", "decisions");
  await page.getByRole("button", { name: TITLE }).first().click();
  await expect(page.locator("article.trace")).toContainText(c.tv("bool", "reproducible"));
  await noHorizontalScroll(page, "decision trace");
});

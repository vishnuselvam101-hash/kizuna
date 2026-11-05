// Evidence explorer (source locations, disagreement) and entity resolution confirm / reject.
import { expect, test } from "@playwright/test";
import { axe, ctx, go, login, noHorizontalScroll, shot } from "../helpers";

test.describe.configure({ mode: "serial" });

test("evidence explorer: a component's claims show document, exact location, method and disagreement", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "viewer");
  await go(page, c, "evidence", "records");
  await page.getByLabel(c.t("records.filter")).fill("JA-BRK");
  await page.getByRole("button", { name: /JA-BRK-1001/ }).click();
  const ev = page.getByRole("region", { name: c.t("records.evidenceFor", { key: "JA-BRK-1001" }), exact: true });
  const material = ev.getByRole("row").filter({ hasText: "SUS316" });
  await expect(material).toContainText("ミルシート_JA-BRK-1001.pdf");
  await expect(material).toContainText(c.t("loc.page", { v: 1 }));
  await expect(ev.getByRole("row").filter({ hasText: "qa.material" })).toContainText(c.t("evidence.disagree"));
  const sheet = ev.getByRole("row").filter({ hasText: "仕様書_JA-BRK-1001.xlsx" }).first();
  await expect(sheet).toContainText(c.t("loc.sheet", { v: "仕様" }));
  // The scanned drawing is pixels only: its claims were read by OCR.
  await expect(ev.getByRole("row").filter({ hasText: "スキャン図面_JA-BRK-1001.pdf" }).first()).toContainText("SUS304");
  await expect(ev.getByTestId("claim-id").first()).toHaveText(/^[0-9a-f-]{36}$/);
  await axe(page, "evidence explorer");
  await noHorizontalScroll(page, "evidence explorer");
  await shot(page, info, "evidence");
});

test("entity resolution: reviewer confirms one identifier variant as the same part and rejects another", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "reviewer");
  await go(page, c, "evidence", "records");
  const dups = page.getByRole("region", { name: c.t("records.duplicates"), exact: true });
  const cand = dups.getByRole("row").filter({ hasText: "JA-SFT2002" });
  await expect(cand).toContainText(c.tv("entity.reason", "separator_difference"));
  await axe(page, "duplicate candidates");
  await cand.getByRole("button", { name: c.t("records.same"), exact: true }).click();
  await expect(cand).toHaveCount(0);
  if (c.lang === "en") {
    const other = dups.getByRole("row").filter({ hasText: "MTR4517" });
    await other.getByRole("button", { name: c.t("records.different"), exact: true }).click();
    await expect(other).toHaveCount(0);
  }
  await expect(dups.getByText(c.t("records.noDuplicates"))).toBeVisible();
});

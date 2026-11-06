// Security monitoring: a brute-force incident, containment (lock account) approve → revert, resolve; and
// quarantine release under four-eyes (the uploader cannot release, a second MFA-verified admin can).
import { expect, test, type Page } from "@playwright/test";
import { join } from "node:path";
import { alert, axe, ctx, DATA, fillLogin, go, login, logout, noHorizontalScroll, row, shot, status, upload, type Ctx } from "../helpers";

test.describe.configure({ mode: "serial" });

async function failLogins(page: Page, c: Ctx, n: number) {
  await page.goto("/");
  for (let i = 0; i < n; i++) {
    await fillLogin(page, c, "victim", `wrong-password-${i}xx`);
    await page.getByRole("button", { name: c.t("login.submit"), exact: true }).click();
    await expect(alert(page)).toBeVisible();
  }
}

test("brute force: failed sign-ins raise an incident; admin approves the account lock, then reverts it and resolves", async ({ page }, info) => {
  test.setTimeout(240_000);
  const c = ctx(info);
  await failLogins(page, c, 3); // thresholds were lowered to 3/3 in the admin journey
  await login(page, c, "admin");
  await go(page, c, "admin", "security");
  const incidents = page.getByRole("table", { name: c.t("sec.incidents"), exact: true });
  // Detection runs every minute on the server; reload the page like an operator would.
  await expect(async () => {
    await go(page, c, "overview");
    await go(page, c, "admin", "security");
    await expect(incidents.getByRole("row").filter({ hasText: c.tv("sec.rule", "brute_force_account") })).toBeVisible({ timeout: 2000 });
  }).toPass({ timeout: 150_000, intervals: [10_000] });
  await incidents.getByRole("row").filter({ hasText: c.tv("sec.rule", "brute_force_account") }).first().click();
  const lock = page.getByRole("row").filter({ hasText: c.tv("sec.action", "lock_account") });
  await expect(lock).toContainText(c.t("sec.needs_approval"));
  await axe(page, "incident detail");
  await shot(page, info, "incident");
  await lock.getByRole("button", { name: c.t("sec.approve"), exact: true }).click();
  await expect(lock).toContainText(c.tv("sec.astatus", "executed"));
  await lock.getByRole("button", { name: c.t("sec.revert"), exact: true }).click();
  await expect(lock).toContainText(c.tv("sec.astatus", "reverted"));
  await page.getByLabel(c.t("sec.comment")).fill("社内テストによる失敗ログイン。対応完了。");
  await page.getByRole("button", { name: c.t("sec.resolve"), exact: true }).click();
  await expect(incidents.getByRole("row").filter({ hasText: c.tv("sec.rule", "brute_force_account") }).first()).toContainText(c.tv("sec.istatus", "resolved"));
  await noHorizontalScroll(page, "security incidents");
  await logout(page, c);
  // The account works again after the revert.
  await login(page, c, "victim");
});

test("quarantine release: the uploading admin is refused (four-eyes); a second admin releases it", async ({ page }, info) => {
  const c = ctx(info);
  const file = "malicious/drawing_launch.pdf";
  await login(page, c, "admin");
  await go(page, c, "evidence", "documents");
  await upload(page, c, join(DATA, "files", file), "drawing");
  await expect(status(page)).toContainText(c.tv("artifact.status", "quarantined"));
  // The uploader sees why they may not release it; the release form is not offered (the server refuses it too).
  await page.getByRole("button", { name: c.t("quarantine.reviewFor", { name: "drawing_launch.pdf" }), exact: true }).click();
  const own = page.getByRole("region", { name: c.t("quarantine.title", { name: "drawing_launch.pdf" }), exact: true });
  await expect(own.getByRole("note")).toHaveText(c.t("quarantine.ownUpload"));
  await expect(own).toContainText(c.t("quarantine.you"));
  await expect(own.getByRole("button", { name: c.t("quarantine.release"), exact: true })).toHaveCount(0);
  await logout(page, c);

  await login(page, c, "admin2");
  await go(page, c, "evidence", "documents");
  await page.getByRole("button", { name: c.t("quarantine.reviewFor", { name: "drawing_launch.pdf" }), exact: true }).click();
  const rel = page.getByRole("region", { name: c.t("quarantine.title", { name: "drawing_launch.pdf" }), exact: true });
  await expect(rel).toContainText("drawing_launch.pdf");
  await expect(rel).toContainText("pdf_");
  const go_ = rel.getByRole("button", { name: c.t("quarantine.release"), exact: true });
  // A written reason and the explicit check are required before release.
  await expect(go_).toBeDisabled();
  await rel.getByLabel(c.t("quarantine.reason")).fill("発行元に原本を確認。Launchアクションは図面ビューアの既定動作で無害。");
  await expect(go_).toBeDisabled();
  await rel.getByRole("checkbox", { name: c.t("quarantine.confirmCheck") }).check();
  await axe(page, "release confirmation");
  await go_.click();
  await expect(status(page)).toHaveText(c.t("quarantine.released", { name: "drawing_launch.pdf" }));
  await expect(row(page, "drawing_launch.pdf")).toContainText(c.tv("artifact.status", "accepted"));
});

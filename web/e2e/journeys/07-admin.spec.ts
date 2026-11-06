// Administration (MFA-verified admin): users & roles, import settings + re-extract, AI opt-in with the local
// mock provider, detection thresholds.
import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import { alert, axe, ctx, email, go, login, noHorizontalScroll, PASSWORD, row, shot, status } from "../helpers";

test.describe.configure({ mode: "serial" });

test("users & roles: add a user, change role, view and end sessions, disable and re-enable", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "admin");
  await go(page, c, "admin", "users");
  const nu = `qc.${c.lang}@e2e.example`;
  const form = page.getByRole("region", { name: c.t("users.create"), exact: true });
  await form.getByLabel(c.t("users.name")).fill("品質管理 太郎");
  await form.getByLabel(c.t("users.email")).fill(nu);
  await form.getByLabel(c.t("users.role")).selectOption("analyst");
  await form.getByLabel(c.t("users.initialPassword")).fill(PASSWORD);
  await form.getByRole("button", { name: c.t("users.createBtn"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("users.created"));
  await page.getByLabel(c.t("users.roleFor", { email: nu })).selectOption("reviewer");
  await expect(status(page)).toHaveText(c.t("users.roleChanged", { email: nu }));
  await page.getByRole("button", { name: c.t("users.sessionsFor", { email: email("viewer") }), exact: true }).click();
  await expect(page.getByRole("region", { name: c.t("users.sessionsFor", { email: email("viewer") }), exact: true })).toBeVisible();
  await page.getByRole("button", { name: c.t("users.revokeFor", { email: email("viewer") }), exact: true }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: c.t("common.confirm"), exact: true }).click();
  await expect(status(page)).toContainText(email("viewer"));
  await page.getByRole("button", { name: c.t("users.disableFor", { email: nu }), exact: true }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: c.t("common.confirm"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("users.disabled", { email: nu }));
  await page.getByRole("button", { name: c.t("users.enableFor", { email: nu }), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("users.enabled", { email: nu }));
  await expect(row(page, nu)).toContainText(c.tv("role", "reviewer"));
  await axe(page, "users");
  await noHorizontalScroll(page, "users");
  await shot(page, info, "users");
});

test("import settings: save Shift_JIS CSV settings; analyst re-extracts the CSV with a recorded reason", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "admin");
  await go(page, c, "admin", "integrations");
  await page.getByLabel(c.t("integrations.encoding")).selectOption("shift_jis");
  // Out-of-range values are flagged before anything is sent.
  const header = page.getByRole("region", { name: c.t("integrations.csv"), exact: true }).getByLabel(c.t("integrations.headerRow"));
  await header.fill("101");
  await page.getByRole("button", { name: c.t("integrations.save"), exact: true }).click();
  await expect(alert(page)).toHaveText(c.t("integrations.invalid", { n: 1 }));
  await header.fill("1");
  await page.getByRole("button", { name: c.t("integrations.save"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("integrations.saved", { v: 1 }));
  await axe(page, "import settings");
  await noHorizontalScroll(page, "import settings");
  await shot(page, info, "integrations");
  await page.getByRole("button", { name: c.t("shell.signOut"), exact: true }).click();

  await login(page, c, "analyst");
  await go(page, c, "evidence", "documents");
  await page.getByRole("button", { name: c.t("docs.reextractFor", { name: "発注一覧_SJIS.csv" }), exact: true }).click();
  const confirm = page.getByRole("button", { name: c.t("docs.reextractConfirm"), exact: true });
  await expect(confirm).toBeDisabled();
  await page.getByLabel(c.t("docs.reextractReason")).fill("取込設定をShift_JISに変更したため");
  await confirm.click();
  await expect(status(page)).toContainText(c.t("docs.reextracted", { n: "§", sup: "§", same: "§", flagged: "§" }).split("§")[1].trim());
});

test("AI-assisted extraction: admin accepts the notice and turns it on; re-extraction maps the maker column 製造元 via the local provider", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "admin");
  await go(page, c, "admin", "integrations");
  const ai = page.getByRole("region", { name: c.t("ai.title"), exact: true });
  await expect(ai.getByTestId("ai-notice")).not.toBeEmpty();
  await expect(ai).toContainText("mock-local");
  const on = ai.getByRole("button", { name: c.t("ai.turnOn"), exact: true });
  await expect(on).toBeDisabled();
  await ai.getByRole("checkbox").check();
  await on.click();
  await expect(ai.getByRole("status")).toContainText(c.t("ai.turnedOn", { v: "§" }).split("§")[0].trim());
  await expect(ai).toContainText(c.tv("ai.status", "on"));
  await axe(page, "AI opt-in");
  await page.getByRole("button", { name: c.t("shell.signOut"), exact: true }).click();

  await login(page, c, "analyst");
  await go(page, c, "evidence", "documents");
  await page.getByRole("button", { name: c.t("docs.reextractFor", { name: "仕様書_JA-BRK-1001.xlsx" }), exact: true }).click();
  await page.getByLabel(c.t("docs.reextractReason")).fill("AI補助抽出を有効化したため");
  const resp = page.waitForResponse((r) => r.url().endsWith("/reextract"));
  await page.getByRole("button", { name: c.t("docs.reextractConfirm"), exact: true }).click();
  const r = await (await resp).json();
  expect(r.ai_claims_suggested, JSON.stringify(r)).toBeGreaterThan(0);
  expect(readFileSync(process.env.E2E_AI_LOG!, "utf8")).toContain("supplier");
});

test("detection thresholds: an inconsistent lock threshold is refused; lowered thresholds are saved", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "admin");
  await go(page, c, "admin", "security");
  const det = page.getByRole("region", { name: c.t("det.title"), exact: true });
  const bf = det.getByLabel(c.t("det.thresholdFor", { rule: c.t("det.t.brute_force") }));
  const lock = det.getByLabel(c.t("det.thresholdFor", { rule: c.t("det.t.brute_force_lock") }));
  await bf.fill("4");
  await lock.fill("3");
  await det.getByRole("button", { name: c.t("det.save"), exact: true }).click();
  await expect(det.getByRole("alert")).toHaveText(c.t("det.invalid", { n: 1 }));
  await expect(lock).toHaveAttribute("aria-invalid", "true");
  await bf.fill("3");
  await det.getByRole("button", { name: c.t("det.save"), exact: true }).click();
  await expect(det.getByRole("status")).toHaveText(c.t("det.saved", { v: 1 }));
  await axe(page, "security + thresholds");
  await noHorizontalScroll(page, "security");
});

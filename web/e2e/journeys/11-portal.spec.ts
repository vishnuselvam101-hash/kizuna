// Supplier portal journey: a PO revision correction makes the match decision need the supplier certificates again → the
// re-request is drafted and approved (four-eyes) → the admin invites a supplier user and addresses the request →
// the supplier, on a phone, activates the account (password + authenticator), opens the request and uploads.
import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import { applyTheme, axe, ctx, go, ja, login, logout, noHorizontalScroll, row, shot, status, totpAt } from "../helpers";

test.describe.configure({ mode: "serial" });

test("a PO revision correction drafts supplier re-requests; a second reviewer approves one", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "analyst");
  await go(page, c, "evidence", "records");
  await page.getByLabel(c.t("records.filter")).fill("JA-BRK-1001");
  await page.getByRole("button", { name: /JA-BRK-1001/ }).click();
  // PO-J-7002 ordered revision A; the accepted match decision (test 10) depends on it.
  const ev = page.getByRole("region", { name: c.t("records.evidenceFor", { key: "JA-BRK-1001" }), exact: true });
  const po = ev.getByRole("row").filter({ has: page.getByRole("button", { name: c.t("claim.reviewFor", { predicate: "po.revision_ordered", value: "A" }), exact: true }) });
  await po.locator("button", { hasText: c.t("evidence.supersede") }).click();
  await page.getByLabel(c.t("evidence.newValue")).fill("B");
  await page.getByLabel(c.t("evidence.reason")).fill("注文書の改訂記載誤り。正しくは改訂B。");
  await page.getByRole("button", { name: c.t("evidence.supersedeBtn"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("evidence.supersededMsg"));
  await logout(page, c);

  await login(page, c, "reviewer");
  await go(page, c, "decisions", "rerequests");
  const plan = page.getByRole("region", { name: c.t("rr.plan"), exact: true });
  // Only the certificates that state the inspected revision must be resent.
  await expect(plan).toContainText("検査成績書_JA-BRK-1001.pdf");
  await plan.getByRole("button", { name: c.t("rr.createDrafts"), exact: true }).click();
  await expect(status(page)).toContainText(c.t("rr.created", { n: "§" }).split("§")[1].trim());
  await logout(page, c);

  await login(page, c, "reviewer2");
  await go(page, c, "decisions", "rerequests");
  const drafts = page.getByRole("region", { name: c.t("rr.drafts"), exact: true });
  await drafts.getByRole("row").filter({ hasText: c.tv("rr.status", "draft") }).first().click();
  const detail = page.getByRole("article").filter({ has: page.locator("#rr-h") });
  await expect(detail.getByRole("region", { name: c.t("rr.letter"), exact: true })).toContainText("JA-BRK-1001");
  await detail.getByRole("button", { name: c.t("rr.approve"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("rr.approved"));
  await axe(page, "supplier re-request");
});

test("supplier portal on a phone: invitation, authenticator activation, request, upload", async ({ page, browser }, info) => {
  test.setTimeout(240_000);
  const c = ctx(info);
  const run = Date.now().toString(36);
  const supplierEmail = `supplier-${c.lang}-${run}@e2e.example`;
  await login(page, c, "admin");
  // The approved re-request (drafts are not shown to suppliers).
  await go(page, c, "decisions", "rerequests");
  const approved = page.getByRole("region", { name: c.t("rr.drafts"), exact: true }).getByRole("row").filter({ hasText: c.tv("rr.status", "approved") }).first();
  const ref = (await approved.locator(".mono").first().textContent())!.trim();
  await go(page, c, "admin", "users");
  const panel = page.getByRole("region", { name: c.t("supplierUsers.title"), exact: true });
  // Address the approved request to a new supplier record created by the invitation.
  const attach = panel.getByRole("form", { name: c.t("supplierUsers.attach") });
  const reqSelect = attach.getByRole("combobox", { name: c.t("supplierUsers.request"), exact: true });
  const supplierName = `${c.lang.toUpperCase()} Toho Seiko ${run}`;
  const invite = panel.getByRole("form", { name: c.t("supplierUsers.invite") });
  await invite.getByLabel(c.t("supplierUsers.newSupplierName")).fill(supplierName);
  await invite.getByLabel(c.t("supplierUsers.name")).fill("北陸 花子");
  await invite.getByLabel(c.t("supplierUsers.email")).fill(supplierEmail);
  await invite.getByRole("button", { name: c.t("supplierUsers.inviteBtn"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("supplierUsers.invited", { email: supplierEmail }));
  const link = (await panel.getByRole("region", { name: c.t("supplierUsers.copy"), exact: true }).locator("code").textContent())!.trim();
  expect(link).toMatch(/\/portal#invite=/);
  const option = reqSelect.locator("option").filter({ hasText: ref });
  await reqSelect.selectOption((await option.getAttribute("value"))!);
  await attach.getByRole("combobox", { name: c.t("supplierUsers.supplier"), exact: true }).selectOption({ label: supplierName });
  await attach.getByRole("button", { name: c.t("supplierUsers.attachBtn"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("supplierUsers.attached", { ref, supplier: supplierName }));
  await expect(row(panel, supplierEmail)).toContainText(c.tv("supplierUsers.status", "invited"));
  await axe(page, "supplier portal users");

  // The supplier's phone: separate browser context, no tenant session.
  const phone = await browser.newContext({
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
    deviceScaleFactor: 2,
    locale: c.lang === "ja" ? "ja-JP" : "en-GB",
    baseURL: info.project.use.baseURL,
  });
  const s = await phone.newPage();
  await s.goto(link.replace(/^https?:\/\/[^/]+/, ""));
  await expect(s.getByRole("heading", { level: 1, name: c.t("portal.invite.title") })).toBeVisible();
  if ((await s.locator("html").getAttribute("lang")) !== c.lang) await s.getByRole("button", { name: c.t("prefs.language", undefined) }).first().click();
  await applyTheme(s, c);
  await axe(s, "portal invitation");
  await noHorizontalScroll(s, "portal invitation");
  const password = "Ein-Lieferant-2026!";
  await s.getByLabel(c.t("portal.invite.newPassword")).fill(password);
  await s.getByRole("button", { name: c.t("portal.invite.continue"), exact: true }).click();
  await expect(s.getByText(c.t("portal.invite.for", { email: supplierEmail, supplier: supplierName }))).toBeVisible();
  const secret = (await s.locator("code").first().textContent())!.trim();
  await axe(s, "portal authenticator setup");
  await noHorizontalScroll(s, "portal authenticator setup");
  await s.getByLabel(c.t("portal.code")).fill(totpAt(secret, Math.floor(Date.now() / 30_000)));
  await s.getByRole("button", { name: c.t("portal.invite.finish"), exact: true }).click();

  // Signed in: only the request addressed to this supplier.
  await expect(s.getByRole("heading", { level: 1, name: c.t("portal.requests") })).toBeVisible();
  const open = s.getByRole("button", { name: c.t("portal.open", { ref }), exact: true });
  await expect(open).toBeVisible();
  await expect(s.getByRole("button", { name: new RegExp(c.t("portal.open", { ref: "§" }).split("§")[0]) })).toHaveCount(1);
  await axe(s, "portal requests");
  await noHorizontalScroll(s, "portal requests");
  await shot(s, info, "portal-requests");
  await open.click();
  await expect(s.getByRole("heading", { level: 1, name: ref })).toBeVisible();
  await expect(s.getByRole("main")).toContainText(/検査成績書_JA-BRK-1001\.pdf|ミルシート_JA-BRK-1001\.pdf/);
  await s.getByLabel(c.t("portal.file")).setInputFiles({ name: "ミルシート_JA-BRK-1001_再提出.pdf", mimeType: "application/pdf", buffer: readFileSync(ja("ミルシート_JA-BRK-1001.pdf")) });
  await s.getByRole("button", { name: c.t("portal.send"), exact: true }).click();
  await expect(s.getByRole("status").filter({ hasText: "ミルシート_JA-BRK-1001_再提出.pdf" }).first()).toBeVisible();
  await expect(s.getByRole("region", { name: c.t("portal.uploads") }).or(s.locator("section", { hasText: c.t("portal.uploads") })).first()).toContainText("ミルシート_JA-BRK-1001_再提出.pdf");
  await axe(s, "portal request detail");
  await noHorizontalScroll(s, "portal request detail");
  await shot(s, info, "portal-upload");
  // Keyboard: the back link and the sign-out button are reachable and work.
  await s.getByRole("button", { name: c.t("portal.back"), exact: true }).focus();
  await s.keyboard.press("Enter");
  await expect(s.getByRole("heading", { level: 1, name: c.t("portal.requests") })).toBeVisible();
  await s.getByRole("button", { name: c.t("portal.signOut"), exact: true }).click();
  await expect(s.getByLabel(c.t("portal.password"), { exact: true })).toBeVisible();
  await phone.close();

  // Back in the tenant: the supplier user is active.
  await page.reload();
  await login(page, c, "admin");
  await go(page, c, "admin", "users");
  await expect(row(page.getByRole("region", { name: c.t("supplierUsers.title"), exact: true }), supplierEmail)).toContainText(c.tv("supplierUsers.status", "active"));
});

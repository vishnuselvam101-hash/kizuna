// Sign-in, sign-out, TOTP enrolment, WebAuthn (virtual authenticator) enrolment + sign-in, recovery code.
import { expect, test, type CDPSession, type Page } from "@playwright/test";
import { alert, applyTheme, axe, ctx, email, fillLogin, go, login, logout, PASSWORD, rememberSecret, shot, totp, type Ctx, type User } from "../helpers";

test.describe.configure({ mode: "serial" });

const recovery: string[] = [];

async function account(page: Page, c: Ctx) {
  // The account page opens from the signed-in user's name in the top bar.
  await page.locator("header button.who").click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(c.t("nav.account"));
}

async function enrolTotp(page: Page, c: Ctx, u: User) {
  await login(page, c, u);
  await account(page, c);
  const panel = page.getByRole("region", { name: c.t("account.totp"), exact: true });
  await panel.getByLabel(c.t("account.currentPassword")).fill(PASSWORD);
  await panel.getByRole("button", { name: c.t("account.totpStart"), exact: true }).click();
  const secret = (await panel.locator("code.secret").innerText()).replace(/\s+/g, "");
  expect(secret).toMatch(/^[A-Z2-7]{32}$/);
  // The otpauth:// setup link is offered as a QR code for the phone.
  await expect(panel.getByRole("img", { name: c.t("account.qrLabel"), exact: true })).toBeVisible();
  rememberSecret(c, u, secret);
  await panel.getByLabel(c.t("account.totpCode")).fill(await totp(c, u));
  await panel.getByRole("button", { name: c.t("account.totpVerify"), exact: true }).click();
  await expect(panel.getByRole("status")).toHaveText(c.t("account.totpDone"));
  await expect(panel).toContainText(c.t("account.totpOn"));
  await axe(page, "account after TOTP enrolment");
  await logout(page, c);
}

test("sign-in: wrong password is refused with a clear message; correct password signs in; sign-out ends the session", async ({ page }, info) => {
  const c = ctx(info);
  await page.goto("/");
  await applyTheme(page, c);
  await axe(page, "login");
  await shot(page, info, "login");
  await fillLogin(page, c, "viewer", "wrong-password-123");
  await page.getByRole("button", { name: c.t("login.submit"), exact: true }).click();
  await expect(alert(page)).toHaveText(c.t("login.failed"));
  await fillLogin(page, c, "viewer");
  await page.getByRole("button", { name: c.t("login.submit"), exact: true }).click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(c.t("nav.dashboard"));
  await expect(page.locator(".who")).toHaveText(`E2E viewer · ${c.tv("role", "viewer")}`);
  await logout(page, c);
  // The session is gone: reloading shows the login page, nothing from the tenant stays rendered.
  await page.reload();
  await expect(page.getByRole("heading", { name: c.t("login.title") })).toBeVisible();
});

test("TOTP: admins enrol an authenticator from the account page and sign in with a code (MFA-verified session)", async ({ page }, info) => {
  const c = ctx(info);
  await enrolTotp(page, c, "admin");
  await enrolTotp(page, c, "admin2");
  // Review decisions (waivers, rejections, entity merges) are privileged too: reviewers enrol as well.
  await enrolTotp(page, c, "reviewer");
  await enrolTotp(page, c, "reviewer2");
  // Password alone now asks for the code; a wrong code is refused.
  await page.goto("/");
  await fillLogin(page, c, "admin");
  await page.getByRole("button", { name: c.t("login.submit"), exact: true }).click();
  await page.getByLabel(c.t("login.totp")).fill("000000");
  await page.getByRole("button", { name: c.t("login.submit"), exact: true }).click();
  await expect(alert(page)).toHaveText(c.t("login.failed"));
  await login(page, c, "admin");
  await go(page, c, "admin", "users");
  await expect(page.getByText(c.t("users.mfaNeeded"))).toHaveCount(0);
  await expect(page.getByRole("row").filter({ hasText: email("admin2") })).toContainText(c.tv("bool", "enabled"));
});

async function virtualKey(page: Page): Promise<{ cdp: CDPSession; id: string }> {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  const { authenticatorId } = await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: { protocol: "ctap2", transport: "usb", hasResidentKey: true, hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true },
  });
  return { cdp, id: authenticatorId };
}

test("WebAuthn: enrol a security key, keep the recovery codes, sign in with the key", async ({ page }, info) => {
  const c = ctx(info);
  const key = await virtualKey(page);
  await login(page, c, "keyuser");
  await account(page, c);
  const panel = page.getByRole("region", { name: c.t("account.keys"), exact: true });
  await panel.getByText(c.t("account.addKey"), { exact: true }).click();
  await panel.getByLabel(c.t("account.keyLabel")).fill("YubiKey 5C");
  await panel.getByLabel(c.t("account.currentPassword")).fill(PASSWORD);
  await panel.getByRole("button", { name: c.t("account.addKeyBtn"), exact: true }).click();
  const codes = page.getByRole("region", { name: c.t("account.codesTitle"), exact: true }).getByRole("listitem");
  await expect(codes).toHaveCount(10);
  recovery.push(...(await codes.allInnerTexts()).map((s) => s.trim()));
  await axe(page, "recovery codes");
  await shot(page, info, "recovery-codes");
  await page.getByRole("button", { name: c.t("account.codesSaved"), exact: true }).click();
  await expect(page.getByRole("heading", { name: c.t("login.title") })).toBeVisible();
  // Password alone is no longer enough: the form switches to the security-key step and the key signs in.
  await fillLogin(page, c, "keyuser");
  await page.getByRole("button", { name: c.t("login.submit"), exact: true }).click();
  await expect(page.getByText(c.t("login.keyStep"), { exact: true })).toBeVisible();
  await page.getByRole("button", { name: c.t("login.useKey"), exact: true }).click();
  await expect(page.getByRole("button", { name: c.t("shell.signOut"), exact: true })).toBeVisible();
  await account(page, c);
  await expect(page.getByRole("region", { name: c.t("account.keys"), exact: true }).getByRole("row").filter({ hasText: "YubiKey 5C" })).toHaveCount(1);
  await expect(page.locator("dl.kv")).toContainText(c.tv("account.session", "verified"));
  await logout(page, c);
  const { credentials } = await key.cdp.send("WebAuthn.getCredentials", { authenticatorId: key.id });
  expect(credentials).toHaveLength(1);
});

test("recovery code: a user who lost the key signs in once with a code", async ({ page }, info) => {
  const c = ctx(info);
  expect(recovery.length).toBe(10);
  await page.goto("/");
  await applyTheme(page, c);
  await fillLogin(page, c, "keyuser");
  await page.getByRole("button", { name: c.t("login.submit"), exact: true }).click();
  await page.getByRole("button", { name: c.t("login.lostKey"), exact: true }).click();
  await page.getByLabel(c.t("login.recoveryCode")).fill(recovery[0]);
  await page.getByRole("button", { name: c.t("login.useRecovery"), exact: true }).click();
  await expect(page.getByRole("status")).toHaveText(c.t("login.recoveredText"));
  await page.getByRole("button", { name: c.t("login.continue"), exact: true }).click();
  await expect(page.getByRole("button", { name: c.t("shell.signOut"), exact: true })).toBeVisible();
  await account(page, c);
  // Recovery removed the lost key: the account is back to password sign-in until a new key is enrolled.
  await expect(page.getByRole("region", { name: c.t("account.keys"), exact: true })).toContainText(c.t("account.noKeys"));
  await expect(page.locator("dl.kv")).toContainText(c.tv("account.session", "unverified"));
  await logout(page, c);
});

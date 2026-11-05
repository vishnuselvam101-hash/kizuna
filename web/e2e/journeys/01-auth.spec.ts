// Sign-in, sign-out, TOTP enrolment, WebAuthn (virtual authenticator) enrolment + sign-in, recovery code.
import { expect, test, type CDPSession, type Page } from "@playwright/test";
import { alert, applyTheme, axe, ctx, email, fillLogin, go, login, logout, PASSWORD, rememberSecret, shot, totp, type Ctx, type User } from "../helpers";

test.describe.configure({ mode: "serial" });

const recovery: string[] = [];

async function account(page: Page, c: Ctx) {
  await page.getByRole("button", { name: c.t("nav.account") }).click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(c.t("nav.account"));
}

async function enrolTotp(page: Page, c: Ctx, u: User) {
  await login(page, c, u);
  await account(page, c);
  const panel = page.getByRole("region", { name: c.t("acc.totpTitle") });
  await panel.getByLabel(c.t("acc.password")).fill(PASSWORD);
  await panel.getByRole("button", { name: c.t("acc.totpStart") }).click();
  const secret = (await panel.getByTestId("totp-secret").innerText()).replace(/\s+/g, "");
  expect(secret).toMatch(/^[A-Z2-7]{32}$/);
  await expect(panel.getByTestId("totp-uri")).toContainText("otpauth://totp/");
  rememberSecret(c, u, secret);
  await panel.getByLabel(c.t("acc.totpCode")).fill(await totp(c, u));
  await panel.getByRole("button", { name: c.t("acc.totpConfirm") }).click();
  await expect(page.getByRole("status")).toHaveText(c.t("acc.totpDone"));
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
  await page.getByRole("button", { name: c.t("login.submit") }).click();
  await expect(alert(page)).toHaveText(c.t("login.failed"));
  await fillLogin(page, c, "viewer");
  await page.getByRole("button", { name: c.t("login.submit") }).click();
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
  // Password alone now asks for the code; a wrong code is refused.
  await page.goto("/");
  await fillLogin(page, c, "admin");
  await page.getByRole("button", { name: c.t("login.submit") }).click();
  await page.getByLabel(c.t("login.totp")).fill("000000");
  await page.getByRole("button", { name: c.t("login.submit") }).click();
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
  const panel = page.getByRole("region", { name: c.t("acc.keyTitle") });
  await panel.getByLabel(c.t("acc.password")).fill(PASSWORD);
  await panel.getByLabel(c.t("acc.keyLabel")).fill("YubiKey 5C");
  await panel.getByRole("button", { name: c.t("acc.keyRegister") }).click();
  const codes = page.getByRole("region", { name: c.t("acc.recoveryTitle") }).getByRole("listitem");
  await expect(codes).toHaveCount(10);
  recovery.push(...(await codes.allInnerTexts()).map((s) => s.trim()));
  await axe(page, "recovery codes");
  await shot(page, info, "recovery-codes");
  await page.getByRole("button", { name: c.t("acc.recoverySaved") }).click();
  await expect(page.getByRole("heading", { name: c.t("login.title") })).toBeVisible();
  // Password alone is no longer enough: the form switches to the security-key step and the key signs in.
  await fillLogin(page, c, "keyuser");
  await page.getByRole("button", { name: c.t("login.submit") }).click();
  await expect(page.getByRole("button", { name: c.t("shell.signOut") })).toBeVisible();
  await account(page, c);
  await expect(page.getByRole("region", { name: c.t("acc.keyTitle") })).toContainText(c.tv("bool", "enabled"));
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
  await page.getByRole("button", { name: c.t("login.submit") }).click();
  await page.getByRole("button", { name: c.t("login.useRecovery") }).click();
  await page.getByLabel(c.t("login.recoveryCode")).fill(recovery[0]);
  await page.getByRole("button", { name: c.t("login.submit") }).click();
  await expect(page.getByRole("button", { name: c.t("shell.signOut") })).toBeVisible();
  await account(page, c);
  // Recovery removed the lost key: the account is back to password sign-in until a new key is enrolled.
  await expect(page.getByRole("region", { name: c.t("acc.keyTitle") })).toContainText(c.tv("bool", "not_enabled"));
  await logout(page, c);
});

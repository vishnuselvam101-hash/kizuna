import { expect, type Locator, type Page, type TestInfo } from "@playwright/test";
import { createHmac } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { DICTS, translate, type Lang } from "../src/i18n";

const creds = JSON.parse(readFileSync(process.env.E2E_CREDS ?? "", "utf8")) as {
  tenants: Record<Lang, string>;
  password: string;
  domain: string;
};
export const PASSWORD = creds.password;
export const DATA = process.env.E2E_DATA ?? "";
const HERE = dirname(fileURLToPath(import.meta.url));
export const SHOTS = join(HERE, "results", "screenshots");

export type User = "admin" | "admin2" | "reviewer" | "reviewer2" | "analyst" | "auditor" | "viewer" | "keyuser" | "victim";
export const email = (u: User) => `${u}@${creds.domain}`;

export function ctx(info: TestInfo) {
  const lang = (info.project.metadata.lang ?? "en") as Lang;
  const theme = (info.project.metadata.theme ?? "dark") as "dark" | "light";
  const t = (k: string, v?: Record<string, string | number>) => translate(lang, k, v);
  const tv = (g: string, v: string) => DICTS[lang][`${g}.${v}`] ?? v.replaceAll("_", " ");
  return { lang, theme, t, tv, tenant: creds.tenants[lang] };
}
export type Ctx = ReturnType<typeof ctx>;

// ---- TOTP (RFC 6238, SHA-1, 30 s, 6 digits) — what an authenticator app computes from the shown secret.
// Enrolled secrets survive worker restarts (a failed test restarts the worker): kept in the run's work dir.
const SECRETS = join(process.env.E2E_WORK ?? ".", "totp-secrets.json");
const secrets = new Map<string, string>(existsSync(SECRETS) ? Object.entries(JSON.parse(readFileSync(SECRETS, "utf8"))) : []);
const STEPS = join(process.env.E2E_WORK ?? ".", "totp-steps.json");
const lastStep = new Map<string, number>(existsSync(STEPS) ? Object.entries(JSON.parse(readFileSync(STEPS, "utf8"))) : []);
function b32(s: string): Buffer {
  const a = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let bits = "";
  for (const c of s.replace(/=+$/, "").toUpperCase()) bits += a.indexOf(c).toString(2).padStart(5, "0");
  return Buffer.from(bits.match(/.{8}/g)!.map((b) => parseInt(b, 2)));
}
export function totpAt(secret: string, step: number): string {
  const msg = Buffer.alloc(8);
  msg.writeBigUInt64BE(BigInt(step));
  const h = createHmac("sha1", b32(secret)).update(msg).digest();
  const o = h[h.length - 1] & 15;
  return String((h.readUInt32BE(o) & 0x7fffffff) % 1_000_000).padStart(6, "0");
}
export function rememberSecret(c: Ctx, u: User, secret: string) {
  secrets.set(`${c.tenant}/${u}`, secret);
  writeFileSync(SECRETS, JSON.stringify(Object.fromEntries(secrets)), { mode: 0o600 });
}
export const hasTotp = (c: Ctx, u: User) => secrets.has(`${c.tenant}/${u}`);
/** A fresh code; waits for the next 30 s step if this user's last code was used in the current one (replay guard). */
export async function totp(c: Ctx, u: User): Promise<string> {
  const key = `${c.tenant}/${u}`;
  const secret = secrets.get(key)!;
  let step = Math.floor(Date.now() / 30_000);
  if ((lastStep.get(key) ?? -1) >= step) {
    await new Promise((r) => setTimeout(r, (step + 1) * 30_000 - Date.now() + 300));
    step = Math.floor(Date.now() / 30_000);
  }
  lastStep.set(key, step);
  writeFileSync(STEPS, JSON.stringify(Object.fromEntries(lastStep)));
  return totpAt(secret, step);
}

// ---- page helpers
export async function applyTheme(page: Page, c: Ctx) {
  const pressed = await page.getByRole("button", { name: c.t("prefs.lightTheme"), exact: true }).first().getAttribute("aria-pressed");
  if ((pressed === "true") !== (c.theme === "light")) await page.getByRole("button", { name: c.t("prefs.lightTheme"), exact: true }).first().click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", c.theme);
}

export async function fillLogin(page: Page, c: Ctx, u: User, password = PASSWORD) {
  await page.getByLabel(c.t("login.org")).fill(c.tenant);
  await page.getByLabel(c.t("login.email")).fill(email(u));
  await page.getByLabel(c.t("login.password"), { exact: true }).fill(password);
}

/** Signs in through the login form (adds the authenticator code when the account has TOTP). */
export async function login(page: Page, c: Ctx, u: User) {
  await page.goto("/");
  await expect(page.locator("html")).toHaveAttribute("lang", c.lang);
  await applyTheme(page, c);
  await fillLogin(page, c, u);
  await page.getByRole("button", { name: c.t("login.submit"), exact: true }).click();
  if (hasTotp(c, u)) {
    await page.getByLabel(c.t("login.totp")).fill(await totp(c, u));
    await page.getByRole("button", { name: c.t("login.submit"), exact: true }).click();
  }
  await expect(page.getByRole("button", { name: c.t("shell.signOut"), exact: true })).toBeVisible();
}

export async function logout(page: Page, c: Ctx) {
  await page.getByRole("button", { name: c.t("shell.signOut"), exact: true }).click();
  await expect(page.getByRole("heading", { name: c.t("login.title") })).toBeVisible();
}

/** Navigates like a user: primary section, then the sub-page tab (if the section has more than one). */
export async function go(page: Page, c: Ctx, section: string, view?: string) {
  await page.getByRole("navigation", { name: c.t("shell.primaryNav") }).getByRole("button", { name: c.t(`section.${section}`), exact: true }).click();
  const sub = page.getByRole("navigation", { name: c.t("shell.sectionNav") });
  if (view && (await sub.count())) await sub.getByRole("button", { name: c.t(`nav.${view}`), exact: true }).click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(c.t(`nav.${view ?? (section === "overview" ? "dashboard" : section)}`));
}

export const status = (page: Page) => page.getByRole("status").filter({ hasNotText: /^$/ }).first();
export const alert = (page: Page) => page.locator("p[role=alert]").first();

export async function upload(page: Page, c: Ctx, file: string, kind: string) {
  await page.getByLabel(c.t("docs.kind")).selectOption(kind);
  await page.getByLabel(c.t("docs.file")).setInputFiles(file);
  await page.getByRole("button", { name: c.t("docs.uploadBtn"), exact: true }).click();
}

export function row(scope: Page | Locator, text: string | RegExp): Locator {
  return scope.getByRole("row").filter({ hasText: text });
}

// ---- accessibility: axe-core (WCAG 2.2 A/AA rules), every violation fails with its targets listed.
const require_ = createRequire(import.meta.url);
const AXE = readFileSync(require_.resolve("axe-core/axe.min.js"), "utf8");
export async function axe(page: Page, label: string) {
  // Evaluate from the top of the page: axe's target-size rule counts controls partly under the sticky top bar
  // after a scroll as undersized, which depends on scroll position rather than the page.
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.addScriptTag({ content: AXE });
  const res = await page.evaluate(async () => {
    // @ts-expect-error injected global
    const r = await window.axe.run(document, { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"] } });
    return r.violations.map((v: { id: string; impact: string; nodes: { target: string[] }[] }) => `${v.id} (${v.impact}): ${v.nodes.slice(0, 4).map((n) => n.target.join(" ")).join(" | ")}`);
  });
  expect(res, `axe violations on ${label}`).toEqual([]);
}

export async function noHorizontalScroll(page: Page, label: string) {
  const over = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(over, `horizontal page scroll on ${label}`).toBeLessThanOrEqual(1);
}

export async function shot(page: Page, info: TestInfo, name: string) {
  await page.screenshot({ path: join(SHOTS, `${info.project.name}-${name}.png`), fullPage: true });
}

export const ja = (f: string) => join(DATA, "ja", f);

/**
 * On Japanese screens, server-generated text (finding explanations, incident summaries, reasons) must be
 * Japanese: no text node may hold a run of three or more plain English words. Identifiers (value_mismatch,
 * qa.result, SHA-256) and document data in Latin script with fewer than three words pass.
 */
export async function noEnglishSentences(page: Page, c: Ctx, where: string) {
  if (c.lang !== "ja") return;
  const runs = await page.locator("main").evaluate((root) => {
    const out: string[] = [];
    const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let n = w.nextNode(); n; n = w.nextNode()) {
      let run: string[] = [];
      for (const x of (n.textContent ?? "").split(/[^A-Za-z0-9'_{}.-]+/).map((y) => y.replace(/[.]+$/, ""))) {
        if (x.length >= 2 && /^[A-Za-z']+$/.test(x)) {
          run.push(x);
          if (run.length >= 3) {
            out.push((n.textContent ?? "").trim());
            break;
          }
        } else if (x) run = [];
      }
    }
    return out;
  });
  expect(runs, `English sentences on the Japanese ${where} screen`).toEqual([]);
}

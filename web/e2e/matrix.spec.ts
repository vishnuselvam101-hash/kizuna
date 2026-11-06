// Every page state (supplier portal sign-in included) × {EN, JA} × {1440, 390} × {dark, light}: axe (WCAG 2.2 AA rules), no horizontal page
// scroll, full-page screenshot. One MFA-verified admin session; language, theme and viewport are switched the
// way a user does (Prefs toggles, window size), so nothing is re-logged.
import { expect, test, type Page } from "@playwright/test";
import { DICTS, translate, type Lang } from "../src/i18n";
import { applyTheme, axe, ctx, go, login, noHorizontalScroll, shot, type Ctx } from "./helpers";

const VIEWPORTS = [
  { name: "1440", width: 1440, height: 900 },
  { name: "390", width: 390, height: 844 },
];

function variant(base: Ctx, lang: Lang, theme: "dark" | "light"): Ctx {
  const t = (k: string, v?: Record<string, string | number>) => translate(lang, k, v);
  const tv = (g: string, v: string) => DICTS[lang][`${g}.${v}`] ?? v.replaceAll("_", " ");
  return { ...base, lang, theme, t, tv };
}

type State = { name: string; open: (p: Page, c: Ctx) => Promise<void> };
const STATES: State[] = [
  { name: "dashboard", open: (p, c) => go(p, c, "overview") },
  { name: "documents", open: (p, c) => go(p, c, "evidence", "documents") },
  {
    name: "records",
    open: async (p, c) => {
      await go(p, c, "evidence", "records");
      await p.getByRole("button", { name: /JA-BRK-1001/ }).first().click();
      await expect(p.getByRole("region", { name: c.t("records.evidenceFor", { key: "JA-BRK-1001" }), exact: true }).getByRole("table")).toBeVisible();
    },
  },
  {
    name: "revisions",
    open: async (p, c) => {
      await go(p, c, "evidence", "revisions");
      await p.getByLabel(c.t("compare.key")).fill("JA-BRK-1001");
      await p.getByLabel(c.t("compare.from")).fill("B");
      await p.getByLabel(c.t("compare.to")).fill("C");
      await p.getByRole("button", { name: c.t("compare.run"), exact: true }).click();
      await expect(p.getByRole("region", { name: c.t("compare.result", { key: "JA-BRK-1001", from: "B", to: "C" }), exact: true })).toBeVisible();
    },
  },
  {
    name: "findings",
    open: async (p, c) => {
      await go(p, c, "conflicts", "findings");
      await p.getByRole("region", { name: c.t("findings.list"), exact: true }).getByRole("row").nth(1).getByRole("button").first().click();
      await expect(p.locator("article.finding")).toBeVisible();
    },
  },
  {
    name: "decision-trace",
    open: async (p, c) => {
      await go(p, c, "decisions", "decisions");
      await p.getByRole("region", { name: c.t("decisions.list"), exact: true }).getByRole("row").nth(1).getByRole("button").first().click();
      await expect(p.locator("article.trace")).toBeVisible();
      await p.locator("summary", { hasText: c.t("decisions.record") }).click();
    },
  },
  {
    name: "match",
    open: async (p, c) => {
      await go(p, c, "conflicts", "match");
      await p.getByRole("region", { name: c.t("match.lines"), exact: true }).getByRole("row").filter({ hasText: "PO-J-7002" }).first().click();
      await expect(p.locator("#match-h")).toBeVisible();
    },
  },
  {
    name: "integrity",
    open: async (p, c) => {
      await go(p, c, "conflicts", "integrity");
      await p.getByLabel(c.t("findings.status")).selectOption("follow_up");
      await p.getByRole("region", { name: c.t("integrity.list"), exact: true }).getByRole("row").nth(1).click();
      await expect(p.getByRole("heading", { level: 3, name: c.t("integrity.evidence") })).toBeVisible();
    },
  },
  {
    name: "drift",
    open: async (p, c) => {
      await go(p, c, "conflicts", "drift");
      await p.getByRole("row").nth(1).click();
    },
  },
  {
    name: "capa",
    open: async (p, c) => {
      await go(p, c, "conflicts", "capa");
      await p.getByRole("button", { name: /^CA-/ }).first().click();
      await expect(p.getByRole("tablist", { name: c.t("capa.steps") })).toBeVisible();
    },
  },
  { name: "affected", open: (p, c) => go(p, c, "decisions", "affected") },
  {
    name: "rerequests",
    open: async (p, c) => {
      await go(p, c, "decisions", "rerequests");
      await p.getByRole("region", { name: c.t("rr.drafts"), exact: true }).getByRole("row").nth(1).click();
      await expect(p.locator("#rr-h")).toBeVisible();
    },
  },
  { name: "reports", open: (p, c) => go(p, c, "reports") },
  { name: "users", open: (p, c) => go(p, c, "admin", "users") },
  { name: "integrations", open: (p, c) => go(p, c, "admin", "integrations") },
  {
    name: "security",
    open: async (p, c) => {
      await go(p, c, "admin", "security");
      const first = p.getByRole("table", { name: c.t("sec.incidents"), exact: true }).getByRole("row").nth(1);
      if (await first.count()) await first.click();
    },
  },
  {
    name: "account",
    open: async (p, c) => {
      await p.locator("header button.who").click();
      await expect(p.getByRole("heading", { level: 1 })).toHaveText(c.t("nav.account"));
    },
  },
  {
    name: "palette",
    open: async (p, c) => {
      await go(p, c, "overview");
      await p.keyboard.press("Control+k");
      await expect(p.getByRole("dialog")).toBeVisible();
    },
  },
];

test("every page state in EN/JA at 1440/390 in dark/light passes axe without horizontal scroll", async ({ page }, info) => {
  test.setTimeout(1_800_000);
  const base = ctx(info);
  const failures: string[] = [];
  // Login page in every combination first (signed out).
  for (const lang of ["en", "ja"] as const)
    for (const vp of VIEWPORTS)
      for (const theme of ["dark", "light"] as const) {
        await page.setViewportSize(vp);
        await page.goto("/");
        const c = variant(base, lang, theme);
        if ((await page.locator("html").getAttribute("lang")) !== lang) await page.getByRole("button", { name: translate(lang === "en" ? "ja" : "en", "prefs.language") }).click();
        await applyTheme(page, c);
        const label = `login ${lang} ${vp.name} ${theme}`;
        await axe(page, label).catch((e) => failures.push(String(e.message).split("\n").slice(0, 6).join(" ")));
        await noHorizontalScroll(page, label).catch((e) => failures.push(String(e.message).split("\n")[0]));
        await shot(page, info, `login-${lang}-${vp.name}-${theme}`);
        // The supplier portal's sign-in page (a separate app on /portal).
        await page.goto("/portal");
        if ((await page.locator("html").getAttribute("lang")) !== lang) await page.getByRole("button", { name: translate(lang === "en" ? "ja" : "en", "prefs.language") }).click();
        await applyTheme(page, c);
        await expect(page.getByRole("heading", { level: 1 })).toHaveText(translate(lang, "portal.signIn"));
        const plabel = `portal sign-in ${lang} ${vp.name} ${theme}`;
        await axe(page, plabel).catch((e) => failures.push(String(e.message).split("\n").slice(0, 6).join(" ")));
        await noHorizontalScroll(page, plabel).catch((e) => failures.push(String(e.message).split("\n")[0]));
        await shot(page, info, `portal-${lang}-${vp.name}-${theme}`);
      }
  await page.setViewportSize(VIEWPORTS[0]);
  await login(page, base, "admin");
  let current: Lang = "en";
  for (const lang of ["en", "ja"] as const) {
    if (lang !== current) {
      await page.getByRole("button", { name: translate(current, "prefs.language") }).first().click();
      await expect(page.locator("html")).toHaveAttribute("lang", lang);
      current = lang;
    }
    for (const vp of VIEWPORTS)
      for (const theme of ["dark", "light"] as const) {
        await page.setViewportSize(vp);
        const c = variant(base, lang, theme);
        await applyTheme(page, c);
        for (const s of STATES) {
          const label = `${s.name} ${lang} ${vp.name} ${theme}`;
          try {
            await s.open(page, c);
            if (s.name === "palette") {
              await axe(page, label).catch((e) => failures.push(String(e.message).split("\n").slice(0, 6).join(" ")));
              await page.keyboard.press("Escape");
              continue;
            }
            await page.waitForTimeout(250); // reveal animation
            await axe(page, label).catch((e) => failures.push(String(e.message).split("\n").slice(0, 6).join(" ")));
            await noHorizontalScroll(page, label).catch((e) => failures.push(String(e.message).split("\n")[0]));
            await shot(page, info, `${s.name}-${lang}-${vp.name}-${theme}`);
          } catch (e) {
            failures.push(`${label}: could not open: ${String((e as Error).message).split("\n")[0]}`);
          }
        }
      }
  }
  expect(failures, failures.join("\n")).toEqual([]);
});

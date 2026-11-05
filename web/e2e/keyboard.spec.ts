// Keyboard-only run of the main journey: sign in → upload → extract → open a conflict → record a review →
// sign out. No mouse; every step is reached with Tab / Shift+Tab / Enter / Space, and focus is always visible.
import { expect, test, type Page } from "@playwright/test";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { applyTheme, axe, ctx, email, hasTotp, PASSWORD, totp } from "./helpers";

async function focusedName(page: Page) {
  return page.evaluate(() => {
    const el = document.activeElement as HTMLElement | null;
    if (!el) return "";
    const lab = el.getAttribute("aria-label") || (el.id && document.querySelector(`label[for="${el.id}"]`)?.textContent) || el.closest("label")?.textContent || el.textContent || "";
    return lab.trim();
  });
}

/** Presses Tab until the focused element's accessible name matches; asserts a visible focus indicator. */
async function tabTo(page: Page, name: string | RegExp, max = 250) {
  for (let i = 0; i < max; i++) {
    await page.keyboard.press("Tab");
    const n = await focusedName(page);
    if (typeof name === "string" ? n === name || n.startsWith(name) : name.test(n)) {
      const visible = await page.evaluate(() => {
        const el = document.activeElement as HTMLElement;
        const s = getComputedStyle(el);
        return el.matches(":focus-visible") && (s.outlineStyle !== "none" || s.boxShadow !== "none");
      });
      expect(visible, `visible focus on ${n}`).toBe(true);
      return;
    }
  }
  throw new Error(`could not reach ${name} with Tab`);
}

test("keyboard only: sign in, upload, extract, review a conflict, sign out", async ({ page }, info) => {
  test.setTimeout(240_000);
  const c = ctx(info);
  const csv = join(process.env.E2E_WORK!, "kb_inspection.csv");
  writeFileSync(csv, "lot,part number,revision,length (mm),result\nL-KB1,KB-777,A,50.0,pass\n");
  await page.goto("/");
  await applyTheme(page, c);
  await tabTo(page, c.t("login.org"));
  await page.keyboard.type(c.tenant);
  await tabTo(page, c.t("login.email"));
  await page.keyboard.type(email("reviewer"));
  await tabTo(page, c.t("login.password"));
  await page.keyboard.type(PASSWORD);
  await page.keyboard.press("Enter");
  if (hasTotp(c, "reviewer")) {
    // Reviewers have an authenticator (review decisions are privileged): the code field appears next.
    await expect(page.getByLabel(c.t("login.totp"))).toBeVisible();
    await page.keyboard.press("Shift+Tab"); // back from the submit button region to the new field, then forward
    await tabTo(page, c.t("login.totp"));
    await page.keyboard.type(await totp(c, "reviewer"));
    await page.keyboard.press("Enter");
  }
  await expect(page.getByRole("button", { name: c.t("shell.signOut") })).toBeVisible();

  // Skip link first, then the primary navigation.
  await page.keyboard.press("Tab");
  expect(await focusedName(page)).toBe(c.t("shell.skip"));
  await tabTo(page, c.t("section.evidence"));
  await page.keyboard.press("Enter");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(c.t("nav.documents"));
  await tabTo(page, c.t("docs.kind"));
  for (let i = 0; i < 3; i++) await page.keyboard.press("ArrowDown"); // spec → drawing → PO → inspection report
  await expect(page.getByLabel(c.t("docs.kind"))).toHaveValue("inspection_report");
  await tabTo(page, c.t("docs.file"));
  const chooser = page.waitForEvent("filechooser");
  await page.keyboard.press("Space");
  await (await chooser).setFiles(csv);
  await tabTo(page, c.t("docs.uploadBtn"));
  await page.keyboard.press("Enter");
  await expect(page.getByRole("status")).toContainText(c.tv("artifact.status", "accepted"));
  await tabTo(page, c.t("docs.extractFor", { name: "kb_inspection.csv" }));
  await page.keyboard.press("Enter");
  await expect(page.getByRole("status")).toContainText(c.t("docs.extracted", { n: 1, labels: "x" }).split("1")[1].slice(0, 8));

  await tabTo(page, c.t("section.conflicts"));
  await page.keyboard.press("Enter");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(c.t("nav.findings"));
  await tabTo(page, "VLV-1950");
  await page.keyboard.press("Enter");
  await expect(page.locator("article.finding")).toContainText("VLV-1950");
  await tabTo(page, c.t("finding.requestEvidence"));
  await page.keyboard.press("Enter");
  await expect(page.getByRole("status")).toHaveText(c.t("findings.reviewed"));
  await axe(page, "keyboard journey end state");
  await tabTo(page, c.t("shell.signOut"));
  await page.keyboard.press("Enter");
  await expect(page.getByRole("heading", { name: c.t("login.title") })).toBeVisible();
});

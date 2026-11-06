// 8D corrective action: a reviewer opens a case from the rejected material finding, works D1–D5 (team, is / is-not,
// containment, root cause, corrective action), and D6 refuses evidence that predates the implemented action.
import { expect, test, type Page } from "@playwright/test";
import { axe, ctx, go, login, noHorizontalScroll, shot, status, type Ctx } from "../helpers";

test.describe.configure({ mode: "serial" });

const tab = (page: Page, c: Ctx, n: number) => page.getByRole("tab", { name: new RegExp(`^${c.t(`capa.d.${n}`)}`) });
const panel = (page: Page) => page.getByRole("tabpanel");

async function complete(page: Page, c: Ctx, n: number) {
  await expect(panel(page).getByText(c.t("capa.ready"))).toBeVisible();
  await page.getByRole("button", { name: c.t("capa.complete"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("capa.completed"));
  await expect(tab(page, c, n)).toContainText(c.t("capa.stepDone"));
  await expect(panel(page).getByText(c.t("capa.locked"))).toBeVisible();
}

async function save(page: Page, c: Ctx) {
  await page.getByRole("button", { name: c.t("capa.save"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("capa.saved"));
}

test("reviewer opens an 8D case from the material finding and works D1–D5; D6 needs evidence newer than the fix", async ({ page }, info) => {
  test.setTimeout(300_000);
  const c = ctx(info);
  await login(page, c, "reviewer");
  await go(page, c, "conflicts", "findings");
  await page.getByLabel(c.t("findings.status")).selectOption("resolved_rejected");
  const list = page.getByRole("region", { name: c.t("findings.list"), exact: true });
  await list.getByRole("row").filter({ hasText: c.tv("conflict.kind", "value_mismatch") }).filter({ hasText: "JA-BRK-1001" }).first().getByRole("button", { name: "JA-BRK-1001" }).click();
  await page.getByRole("button", { name: c.t("capa.fromFindingBtn"), exact: true }).click();
  const opened = page.getByRole("status").filter({ hasText: /CA-[0-9A-F]{8}/ });
  await expect(opened).toBeVisible();
  const ref = (await opened.innerText()).match(/CA-[0-9A-F]{8}/)![0];
  // A second case for the same finding is refused while the first is open.
  await page.getByRole("button", { name: c.t("capa.fromFindingBtn"), exact: true }).click();
  await expect(page.getByText(c.t("err.case_already_open"))).toBeVisible();

  await go(page, c, "conflicts", "capa");
  await page.getByRole("button", { name: ref, exact: true }).click();
  await expect(page.getByRole("heading", { level: 2, name: new RegExp(ref) })).toBeVisible();
  // The finding's two claims (SUS304 spec, SUS316 mill sheet) are carried over as evidence.
  const src = page.getByRole("region", { name: c.t("capa.sourceEvidence"), exact: true });
  await expect(src).toContainText("SUS316");
  await expect(tab(page, c, 1)).toHaveAttribute("aria-selected", "true");

  // D1 team: the step lists what is missing until two people and a leader are named.
  await expect(panel(page).getByText(c.t("capa.missing.team_needs_two_people"))).toBeVisible();
  await page.getByRole("button", { name: c.t("capa.addMember"), exact: true }).click();
  await page.getByRole("button", { name: c.t("capa.addMember"), exact: true }).click();
  const members = panel(page).getByRole("combobox", { name: c.t("capa.member"), exact: true });
  await members.nth(0).selectOption({ label: "E2E reviewer" });
  await members.nth(1).selectOption({ label: "E2E reviewer2" });
  await save(page, c);
  await complete(page, c, 1);
  await axe(page, "8D case D1");

  // D2 problem: statement + is / is-not; the finding's evidence is already cited.
  await tab(page, c, 2).click();
  await panel(page).getByLabel(c.t("capa.statement.2")).fill("ミルシートの材質がSUS316、図面・仕様書はSUS304。");
  await page.getByRole("button", { name: c.t("capa.addRow"), exact: true }).click();
  await panel(page).getByLabel(`${c.t("capa.is")} 1`, { exact: true }).fill("JA-BRK-1001 ロット L-2401");
  await panel(page).getByLabel(`${c.t("capa.isNot")} 1`, { exact: true }).fill("JA-SFT-2002");
  // The mill sheet claim was rejected as faulty in the finding review: it is flagged and cannot carry the step.
  const cite2 = panel(page).getByRole("group", { name: c.t("capa.cite"), exact: true });
  await expect(cite2.getByRole("checkbox", { name: /SUS316/ })).toBeChecked();
  await expect(cite2.getByRole("listitem").filter({ hasText: "SUS316" })).toContainText(c.t("capa.notUsable"));
  await save(page, c);
  await expect(panel(page).getByText(c.t("capa.missing.problem_evidence_required"))).toBeVisible();
  await cite2.getByRole("checkbox", { name: /SUS316/ }).uncheck();
  await expect(cite2.getByRole("checkbox", { name: /^mfg\.material SUS304/ }).first()).toBeChecked();
  await save(page, c);
  await complete(page, c, 2);

  // D3 containment: one action citing the specified material.
  await tab(page, c, 3).click();
  await panel(page).getByRole("button", { name: c.t("capa.addItem"), exact: true }).click();
  await panel(page).getByLabel(c.t("capa.description"), { exact: true }).fill("在庫ロット L-2401 を隔離、全数材質確認。");
  await panel(page).getByRole("group", { name: `${c.t("capa.cite")} — 1` }).getByRole("checkbox", { name: /^mfg\.material SUS304/ }).first().check();
  await save(page, c);
  await complete(page, c, 3);

  // D4 root cause: an occurrence cause citing evidence.
  await tab(page, c, 4).click();
  await panel(page).getByRole("button", { name: c.t("capa.addItem"), exact: true }).click();
  await panel(page).getByRole("combobox", { name: c.t("capa.kind"), exact: true }).selectOption("occurrence");
  await panel(page).getByLabel(c.t("capa.description"), { exact: true }).fill("受入時にミルシートの材質照合をしていなかった。");
  await panel(page).getByRole("group", { name: `${c.t("capa.cite")} — ${c.t("capa.cause", { n: 1 })}` }).getByRole("checkbox", { name: /^mfg\.material SUS304/ }).first().check();
  await save(page, c);
  await complete(page, c, 4);

  // D5: every cause needs an action; then its implementation is recorded.
  await tab(page, c, 5).click();
  await expect(panel(page).getByText(c.t("capa.missing.corrective_action_required"))).toBeVisible();
  const add = page.getByRole("form", { name: c.t("capa.addAction"), exact: true });
  await add.getByLabel(c.t("capa.description"), { exact: true }).fill("受入検査でミルシート材質を図面と自動照合する。");
  await add.getByRole("combobox", { name: c.t("capa.owner"), exact: true }).selectOption({ label: "E2E reviewer2" });
  await add.getByLabel(c.t("capa.due")).fill("2031-01-31");
  await add.getByRole("checkbox").first().check();
  await add.getByRole("button", { name: c.t("capa.addAction"), exact: true }).click();
  await expect(status(page)).toHaveText(c.t("capa.actionAdded"));
  await panel(page).getByLabel(c.t("capa.statement.5")).fill("流出原因と発生原因の双方に対策。");
  await save(page, c);
  await complete(page, c, 5);
  await panel(page).getByRole("button", { name: c.t("capa.markDone"), exact: true }).click();
  // Implementation is recorded with its evidence; without any the server refuses it.
  await panel(page).getByRole("button", { name: c.t("capa.markDone"), exact: true }).last().click();
  await expect(page.locator("p[role=alert]").first()).toBeVisible();
  await panel(page).getByRole("group", { name: c.t("capa.cite"), exact: true }).getByRole("checkbox", { name: /^mfg\.material SUS304/ }).first().check();
  await panel(page).getByLabel(c.t("capa.doneComment")).fill("受入検査手順書 改訂3 で運用開始。");
  await panel(page).getByRole("button", { name: c.t("capa.markDone"), exact: true }).last().click();
  await expect(status(page)).toHaveText(c.t("capa.actionDone"));
  await expect(panel(page).getByRole("row").filter({ hasText: "受入検査" })).toContainText(c.tv("capa.actionStatus", "done"));

  // D6: citing the old (pre-fix) evidence does not prove effectiveness.
  await tab(page, c, 6).click();
  await panel(page).getByLabel(c.t("capa.statement.6")).fill("次ロットで材質照合済み。");
  await panel(page).getByRole("group", { name: c.t("capa.cite"), exact: true }).getByRole("checkbox", { name: /^mfg\.material SUS304/ }).first().check();
  await save(page, c);
  await expect(panel(page).getByText(c.t("capa.missing.verification_evidence_not_new"))).toBeVisible();
  await expect(page.getByRole("button", { name: c.t("capa.complete"), exact: true })).toBeVisible();

  // Keyboard: arrow keys move between the step tabs.
  await tab(page, c, 6).focus();
  await page.keyboard.press("ArrowLeft");
  await expect(tab(page, c, 5)).toBeFocused();
  await expect(tab(page, c, 5)).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Home");
  await expect(tab(page, c, 1)).toBeFocused();

  await expect(page.getByRole("row").filter({ hasText: ref })).toContainText(c.t("capa.progressOf", { n: 5 }));
  await axe(page, "8D case");
  await noHorizontalScroll(page, "8D case");
  await shot(page, info, "capa");
});

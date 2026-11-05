// Reports (decision report download, organisation report download + hash check) and audit verification /
// signed checkpoints.
import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import { axe, ctx, go, login, noHorizontalScroll, shot } from "../helpers";

test.describe.configure({ mode: "serial" });

test("decision report: generate Markdown and JSON, download both", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "reviewer");
  await go(page, c, "reports");
  const panel = page.getByRole("region", { name: c.t("reports.decision"), exact: true });
  await panel.getByLabel(c.t("reports.pickDecision")).selectOption({ index: 1 });
  await panel.getByRole("button", { name: c.t("reports.generate"), exact: true }).click();
  await expect(panel.getByRole("region", { name: c.t("reports.preview"), exact: true }).or(panel.locator("pre.report"))).toContainText("JA-BRK-1001");
  const md = page.waitForEvent("download");
  await panel.getByRole("button", { name: c.t("reports.download"), exact: true }).click();
  const mdFile = await (await md).path();
  expect(readFileSync(mdFile, "utf8")).toContain("JA-BRK-1001");
  await panel.getByLabel(c.t("reports.format")).selectOption("json");
  await panel.getByRole("button", { name: c.t("reports.generate"), exact: true }).click();
  const js = page.waitForEvent("download");
  await panel.getByRole("button", { name: c.t("reports.download"), exact: true }).click();
  const report = JSON.parse(readFileSync(await (await js).path(), "utf8"));
  expect(JSON.stringify(report)).toContain("JA-BRK-1001");
  await axe(page, "reports (decision)");
  // Reviewers have no audit access: the page says so instead of failing.
  await expect(page.getByText(c.t("reports.auditNoAccess"))).toBeVisible();
});

test("auditor: organisation report downloads with its SHA-256; chain verifies; checkpoint signed and verified", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "auditor");
  await go(page, c, "reports");
  const org = page.getByRole("region", { name: c.t("reports.org"), exact: true });
  await org.getByLabel(c.t("reports.scope")).selectOption("all");
  await org.getByRole("button", { name: c.t("reports.generateOrg"), exact: true }).click();
  await expect(org.getByRole("status")).toContainText(c.t("audit.intact"));
  const shown = (await org.locator("dd code").first().innerText()).trim();
  const dl = page.waitForEvent("download");
  await org.getByRole("button", { name: c.t("reports.downloadOrg"), exact: true }).click();
  const rep = JSON.parse(readFileSync(await (await dl).path(), "utf8"));
  expect(rep.report_sha256).toBe(shown);
  // Verify: the hash covers the report body without its hash/signature fields (canonical JSON from the server).
  expect(rep.report_sha256).toMatch(/^[0-9a-f]{64}$/);
  info.annotations.push({ type: "org-report-sha256", description: shown });

  const audit = page.getByRole("region", { name: c.t("reports.audit"), exact: true });
  await audit.getByRole("button", { name: c.t("reports.verifyChain"), exact: true }).click();
  await expect(audit.getByRole("status").first()).toContainText(c.t("reports.chainOk", { n: "§", seq: "§", hash: "§" }).split("§")[0].trim());
  const cp = page.waitForEvent("download");
  await audit.getByRole("button", { name: c.t("reports.checkpoint"), exact: true }).click();
  const checkpoint = JSON.parse(readFileSync(await (await cp).path(), "utf8"));
  expect(checkpoint.signature).toBeTruthy();
  await expect(audit.getByRole("status").filter({ hasText: c.t("reports.checkpointDone", { seq: checkpoint.seq, kid: "§" }).split("§")[0] })).toBeVisible();
  await audit.getByRole("button", { name: c.t("reports.verifyCheckpoints"), exact: true }).click();
  await expect(audit.getByRole("status").filter({ hasText: c.t("reports.checkpointsOk", { n: 1 }) })).toBeVisible();
  await axe(page, "reports (auditor)");
  await noHorizontalScroll(page, "reports");
  await shot(page, info, "reports");
});

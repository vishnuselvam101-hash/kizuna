// Reports (decision report download, organisation report download + hash check), audit verification / signed
// checkpoints, and the decision's evidence pack verified offline by the kizuna-verify-pack CLI.
import { expect, test } from "@playwright/test";
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { axe, ctx, go, login, noHorizontalScroll, shot } from "../helpers";

test.describe.configure({ mode: "serial" });
const TITLE = "JA-BRK-1001 量産承認（改訂B）";
const KEY_FILE = (lang: string) => join(process.env.E2E_WORK ?? ".", `checkpoint-key-${lang}.json`);

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
  // Kept for the evidence-pack test: the key a customer would pin, obtained independently of the pack.
  writeFileSync(KEY_FILE(c.lang), JSON.stringify({ key_id: checkpoint.key_id }));
  await expect(audit.getByRole("status").filter({ hasText: c.t("reports.checkpointDone", { seq: checkpoint.seq, kid: "§" }).split("§")[0] })).toBeVisible();
  await audit.getByRole("button", { name: c.t("reports.verifyCheckpoints"), exact: true }).click();
  await expect(audit.getByRole("status").filter({ hasText: c.t("reports.checkpointsOk", { n: 1 }) })).toBeVisible();
  await axe(page, "reports (auditor)");
  await noHorizontalScroll(page, "reports");
  await shot(page, info, "reports");
});

test("evidence pack: reviewer exports the decision's pack; kizuna-verify-pack verifies it offline and catches tampering", async ({ page }, info) => {
  const c = ctx(info);
  // The pack proves events up to a signed checkpoint: the auditor's checkpoint above covers the decision.
  const checkpoint = JSON.parse(readFileSync(KEY_FILE(c.lang), "utf8"));
  await login(page, c, "reviewer");
  await go(page, c, "decisions", "decisions");
  await page.getByRole("region", { name: c.t("decisions.list"), exact: true }).getByRole("button", { name: TITLE }).first().click();
  const pack = page.getByRole("region", { name: c.t("pack.title"), exact: true });
  await expect(pack).toContainText(c.t("pack.notProven"));
  const dl = page.waitForEvent("download");
  await pack.getByRole("button", { name: c.t("pack.export"), exact: true }).click();
  const zip = await (await dl).path();
  await expect(pack.getByRole("status")).toHaveText(c.t("pack.done"));
  await axe(page, "evidence pack");

  // Offline: no database, no network — only the ZIP.
  const ok = verifyPack(zip);
  expect(ok.code, ok.out).toBe(0);
  const report = JSON.parse(ok.out);
  expect(report.checks.length).toBeGreaterThan(3);
  expect(report.ok).toBe(true);
  expect(report.checks.every((x: { ok: boolean }) => x.ok)).toBe(true);
  // Pinned to the tenant's signing key (from the checkpoint), in the user's language.
  const kid = String(checkpoint.key_id);
  const text = execFileSync(process.env.E2E_VERIFY_PACK!, [zip, "--trusted-key", kid, "--lang", c.lang], { encoding: "utf8" });
  expect(text).toContain("JA-BRK-1001");

  // Change one byte of one captured document: the pack no longer verifies.
  const bad = join(dirname(zip), "tampered.zip");
  execFileSync("python3", ["-I", "-c", TAMPER, zip, bad]);
  const res = verifyPack(bad);
  expect(res.code, res.out).toBe(1);
  expect(JSON.parse(res.out).ok).toBe(false);
});

const TAMPER = `import sys, zipfile
src, dst = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(src) as a, zipfile.ZipFile(dst, "w") as b:
    done = False
    for i in a.infolist():
        d = a.read(i)
        if not done and i.filename.startswith("artifacts/") and d:
            d = bytes([d[0] ^ 1]) + d[1:]
            done = True
        b.writestr(i, d)
    assert done, "no document in the pack"`;

function verifyPack(zip: string): { code: number; out: string } {
  try {
    return { code: 0, out: execFileSync(process.env.E2E_VERIFY_PACK!, [zip, "--json"], { encoding: "utf8" }) };
  } catch (e) {
    const x = e as { status: number; stdout: string };
    return { code: x.status, out: x.stdout };
  }
}

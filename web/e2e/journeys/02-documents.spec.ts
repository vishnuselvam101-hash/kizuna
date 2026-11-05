// Upload every document type (Japanese originals + the synthetic English set), quarantine of a hostile PDF,
// duplicate capture, integrity verification, extraction (incl. OCR of a scanned PDF and a DXF title block).
import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { axe, ctx, DATA, go, ja, login, noHorizontalScroll, row, shot, status, upload} from "../helpers";

test.describe.configure({ mode: "serial" });

export const JA_DOCS: [string, string][] = [
  ["検査成績書_JA-BRK-1001.pdf", "inspection_report"],
  ["仕様書_JA-BRK-1001.xlsx", "spec_document"],
  ["発注一覧_SJIS.csv", "purchase_order"],
  ["図面_JA-BRK-1001.dxf", "drawing"],
  ["スキャン図面_JA-BRK-1001.pdf", "drawing"],
];

test("analyst uploads each Japanese document type; a hostile certificate is quarantined, never processed", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "analyst");
  await go(page, c, "evidence", "documents");
  for (const [f, kind] of JA_DOCS) {
    await upload(page, c, ja(f), kind);
    await expect(status(page)).toHaveText(c.t("docs.stored", { status: c.tv("artifact.status", "accepted") }));
    await expect(row(page, f)).toContainText(c.tv("artifact.status", "accepted"));
  }
  await upload(page, c, ja("検査成績書_改竄.pdf"), "inspection_report");
  await expect(status(page)).toHaveText(c.t("docs.stored", { status: c.tv("artifact.status", "quarantined") }));
  const bad = row(page, "検査成績書_改竄.pdf");
  await expect(bad).toContainText(c.tv("artifact.status", "quarantined"));
  await expect(bad).toContainText("pdf_active_content");
  await expect(bad.getByRole("button", { name: c.t("docs.extract") })).toHaveCount(0);
  // The same bytes again: recorded as a new capture of the stored file, not a second document.
  await upload(page, c, ja("検査成績書_JA-BRK-1001.pdf"), "inspection_report");
  await expect(status(page)).toHaveText(c.t("docs.duplicate", { status: c.tv("artifact.status", "accepted") }));
  await expect(row(page, "検査成績書_JA-BRK-1001.pdf")).toHaveCount(1);
  await page.getByRole("button", { name: c.t("docs.verifyFor", { name: "仕様書_JA-BRK-1001.xlsx" }) }).click();
  await expect(status(page)).toHaveText(c.t("docs.intact"));
  await axe(page, "documents");
  await noHorizontalScroll(page, "documents");
  await shot(page, info, "documents");
});

test("reviewer2 uploads the supplier mill sheet (they may not review conflicts built on it later)", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "reviewer2");
  await go(page, c, "evidence", "documents");
  await upload(page, c, ja("ミルシート_JA-BRK-1001.pdf"), "inspection_report");
  await expect(status(page)).toHaveText(c.t("docs.stored", { status: c.tv("artifact.status", "accepted") }));
});

test("English tenant also ingests the synthetic supplier set (specs, drawings, POs, inspection lots)", async ({ page }, info) => {
  const c = ctx(info);
  test.skip(c.lang !== "en", "bulk English set is loaded once, in the English tenant");
  const manifest = JSON.parse(readFileSync(join(DATA, "manifest.json"), "utf8")) as { file: string; kind: string; expect: string }[];
  await login(page, c, "analyst");
  await go(page, c, "evidence", "documents");
  for (const m of manifest.filter((x) => x.expect === "accepted")) {
    await upload(page, c, join(DATA, "files", m.file), m.kind);
    await expect(status(page)).toContainText(c.tv("artifact.status", "accepted"));
  }
});

test("analyst extracts claims from every accepted document, including OCR of the scanned drawing and the DXF", async ({ page }, info) => {
  const c = ctx(info);
  await login(page, c, "analyst");
  await go(page, c, "evidence", "documents");
  await expect(row(page, JA_DOCS[0][0])).toBeVisible();
  const [pre, post] = c.t("docs.extractFor", { name: "§" }).split("§");
  const labels = (await page.locator("button[aria-label]").evaluateAll((bs) => bs.map((b) => b.getAttribute("aria-label")!))).filter(
    (l) => l.startsWith(pre) && l.endsWith(post),
  );
  expect(labels.length).toBeGreaterThanOrEqual(JA_DOCS.length + 1);
  const counts: Record<string, number> = {};
  const unmapped: Record<string, string[]> = {};
  for (const l of labels) {
    const name = l.slice(pre.length, l.length - post.length);
    const resp = page.waitForResponse((r) => r.url().endsWith("/extract") && r.request().method() === "POST");
    await page.getByRole("button", { name: l, exact: true }).click();
    const r = await (await resp).json();
    expect(r.claims_created, `extract ${name}: ${JSON.stringify(r)}`).toBeDefined();
    const nf = new Intl.NumberFormat(c.lang === "ja" ? "ja-JP" : "en-GB");
    await expect(status(page)).toHaveText(c.t("docs.extracted", { n: nf.format(r.claims_created), labels: r.unmapped_labels.join(", ") || c.t("common.none") }));
    counts[name] = r.claims_created;
    unmapped[name] = r.unmapped_labels;
  }
  info.annotations.push({ type: "claims", description: JSON.stringify({ counts, unmapped }) });
  console.log(JSON.stringify({ counts, unmapped }));
  for (const [f] of [...JA_DOCS, ["ミルシート_JA-BRK-1001.pdf"]]) {
    const key = Object.keys(counts).find((k) => k.includes(f))!;
    expect(counts[key], `claims from ${f}`).toBeGreaterThan(0);
  }
  // Japanese labels (長さ, 引張強さ, 硬さ …) are understood; only the maker column is left for the AI-assisted mapper.
  expect(unmapped["仕様書_JA-BRK-1001.xlsx"]).toEqual(["製造者"]);
  for (const f of ["検査成績書_JA-BRK-1001.pdf", "ミルシート_JA-BRK-1001.pdf"])
    expect(unmapped[f].filter((l) => /[\u3040-\u9fff]/.test(l)), `Japanese labels left unmapped in ${f}`).toEqual([]);
});

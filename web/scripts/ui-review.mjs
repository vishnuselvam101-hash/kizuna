// Browser review of the built UI against a mocked API (synthetic data only).
// Usage: pnpm build && pnpm exec vite preview --port 4317 &
//        PW_CORE=/path/to/playwright-core OUT=/tmp/shots node scripts/ui-review.mjs
// Produces screenshots (1440/390 × dark/light), in-browser axe results (incl. colour contrast),
// LCP/CLS under Fast-3G + 4× CPU throttling, and rAF frame rate of the WebGL views.
import { createRequire } from "node:module";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PW_CORE ?? "playwright-core");
const BASE = process.env.BASE ?? "http://127.0.0.1:4317";
const OUT = process.env.OUT ?? "ui-review-out";
mkdirSync(OUT, { recursive: true });
const axeSrc = readFileSync(new URL("../node_modules/axe-core/axe.min.js", import.meta.url), "utf8");

const sha = (c) => c.repeat(64).slice(0, 64);
const now = "2026-11-04T09:00:00Z";
const artifacts = [
  ["a1", "BRK-100 specification rev C.pdf", "spec_document", "accepted"],
  ["a2", "PO-5521 Supplier A.csv", "purchase_order", "accepted"],
  ["a3", "BRK-100 drawing rev D.pdf", "drawing", "accepted"],
  ["a4", "Inspection lot 7781.xlsx", "inspection_report", "accepted"],
  ["a5", "unknown-macro.xlsm", "other", "quarantined"],
].map(([id, display_name, kind, status], i) => ({ id, display_name, kind, status, sha256: sha("0123456789abcdef"[i]), size_bytes: 20480 + i * 7000, media_type: "application/pdf", created_at: now, scan_result: status === "quarantined" ? { findings: ["macro_content"] } : {} }));
const claims = [];
const preds = ["mfg.material", "mfg.finish", "mfg.thickness", "mfg.revision", "mfg.tolerance"];
["BRK-100", "BRK-101", "BRK100", "HSG-220", "SUP-ACME"].forEach((key, k) =>
  preds.forEach((p, j) =>
    claims.push({ id: `c${k}${j}`, subject_type: key.startsWith("SUP") ? "supplier" : "part", subject_key: key, predicate: p, value_text: j === 0 ? (k ? "S275" : "S355") : `v${j}`, value_num: null, value_unit: null, revision: k % 2 ? "D" : "C", artifact_id: `a${(j % 4) + 1}`, artifact_display_name: artifacts[j % 4].display_name, source_location: { page: j + 1 }, extraction_method: "manual", confidence_bps: 9000 - j * 300, review_status: j === 2 ? "accepted" : "unreviewed", recorded_at: now, superseded_at: null }),
  ),
);
const conflicts = [
  { id: "f1", kind: "value_mismatch", subject_key: "BRK-100", status: "open", explanation: { why: "Specification rev C states material S355; purchase order PO-5521 states S275.", rule: "value_mismatch@1" }, detected_at: now },
  { id: "f2", kind: "revision_mismatch", subject_key: "BRK-101", status: "open", explanation: { why: "Purchase order references drawing rev C; the latest released drawing is rev D." }, detected_at: now },
  { id: "f3", kind: "missing_approval", subject_key: "HSG-220", status: "open", explanation: { why: "No first-article approval is recorded for HSG-220." }, detected_at: now },
];
const decisions = [
  { id: "d1", kind: "supplier_part_release", title: "Release BRK-100 from Supplier A", subject_key: "BRK-100", outcome: "approved", status: "potentially_affected", decided_at: now, derivation_sha256: sha("e") },
  { id: "d2", kind: "first_article_approval", title: "First article BRK-101", subject_key: "BRK-101", outcome: "approved", status: "valid", decided_at: now, derivation_sha256: sha("f") },
];
const trace = {
  decision: { ...decisions[0], rationale: "Material and finish match drawing rev C." },
  derivation_reproducible: true,
  evidence: [
    { claim_id: "c00", predicate: "mfg.material", snapshot_at_decision: { text: "S355" }, current_value: { text: "S275" }, changed: true },
    { claim_id: "c01", predicate: "mfg.finish", snapshot_at_decision: { text: "zinc" }, current_value: { text: "zinc" }, changed: false },
    { claim_id: "c02", predicate: "mfg.thickness", snapshot_at_decision: { num: "4", unit: "mm" }, current_value: { num: "4", unit: "mm" }, changed: false },
  ],
  invalidations: [{ reason: "mfg.material superseded by PO-5521", material: true, recorded_at: now, cause_kind: "claim_superseded" }],
  changed_since_decision: [{ claim_id: "c10", predicate: "mfg.material", value: { text: "S275" }, recorded_at: now, event: "recorded_after_decision" }],
};
const detail = (c) => ({
  conflict: c,
  claim_a: claims[0],
  claim_b: c.kind === "missing_approval" ? null : claims[5],
  sources: [{ claim_id: "c00", display_name: artifacts[0].display_name, sha256: sha("0"), location: { page: 1 } }, { claim_id: "c10", display_name: artifacts[1].display_name, sha256: sha("1"), location: { row: 14, col: 3 } }],
  reviews: [],
  requires_human_review: true,
  limitations: "Deterministic rule value_mismatch@1; extracted values may contain extraction errors (lowest source confidence 9000 bps); a matching hash proves only that a document is unchanged since capture.",
  potentially_affected_decisions: [{ id: "d1", title: decisions[0].title, status: "potentially_affected" }],
});
const users = [
  { id: "u1", email: "aiko.admin@example.test", display_name: "Aiko Admin", role: "tenant_admin", status: "active", totp_enabled: true, locked_until: null },
  { id: "u2", email: "ren.reviewer@example.test", display_name: "Ren Reviewer", role: "reviewer", status: "active", totp_enabled: true, locked_until: null },
  { id: "u3", email: "sam.analyst@example.test", display_name: "Sam Analyst", role: "analyst", status: "disabled", totp_enabled: false, locked_until: null },
];
const events = Array.from({ length: 30 }, (_, i) => ({ seq: i + 1, occurred_at: now, actor_user_id: "u1", actor_kind: "user", action: ["artifact.uploaded", "claim.extracted", "conflict.reviewed", "decision.created"][i % 4], target_type: "artifact", target_id: "a1", details: {}, hash: sha("9") }));

function respond(path) {
  const p = path.replace(/^\/api\/v1/, "");
  const [base, q = ""] = p.split("?");
  if (base === "/auth/login") return { access_token: "t", refresh_token: "r", mfa_verified: true };
  if (base === "/auth/me") return { user_id: "u1", tenant_id: "t", email: users[0].email, display_name: "Aiko Admin", role: "tenant_admin", mfa_enabled: true, session_mfa_verified: true };
  if (base === "/artifacts") return artifacts;
  if (base === "/dashboard/summary") return { artifacts: { accepted: 4, quarantined: 1 }, claims: { unreviewed: 20, accepted: 5 }, conflicts: { open: 3, resolved: 2 }, conflict_kinds_open: { value_mismatch: 1, revision_mismatch: 1, missing_approval: 1 }, decisions: { valid: 1, potentially_affected: 1 }, subjects: { part: 4, supplier: 1 } };
  if (base === "/subjects") {
    const type = new URLSearchParams(q).get("type");
    return [...new Set(claims.filter((c) => c.subject_type === type).map((c) => c.subject_key))].map((k) => ({ subject_type: type, subject_key: k, claims: 5, predicates: 5 }));
  }
  if (base === "/integrations/import-settings") return { default_kind: "spec_document", max_upload_mb: 25, csv: { delimiter: ",", encoding: "shift_jis", header_row: 1, max_records: 20000 }, xlsx: { sheet: "", header_row: 1, max_sheets: 50, max_records: 20000 }, pdf: { ocr: true, ocr_language: "jpn+eng", max_pages: 500 }, mapping: { subject_key: "part_number", predicate: "attribute", value: "value", unit: "unit", revision: "revision" } };
  if (base === "/security/summary") return { open_incidents: 1, actions_awaiting_approval: 1, events_24h: { "auth.login_failed": { severity: "medium", count: 14 }, "upload.rejected": { severity: "low", count: 2 } }, active_ip_blocks: [{ ip: "203.0.113.7", until: 1793000000 }] };
  if (base === "/security/incidents") return [{ id: "i1", rule: "brute_force_account", severity: "high", subject: "203.0.113.7", summary: "14 failed sign-ins for aiko.admin@example.test in 10 minutes.", status: "open", last_seen: now }];
  if (base.startsWith("/security/incidents/")) return { incident: { id: "i1", rule: "brute_force_account", severity: "high", subject: "203.0.113.7", summary: "14 failed sign-ins for aiko.admin@example.test in 10 minutes.", status: "open", last_seen: now }, actions: [{ id: "x1", kind: "block_ip", target: "203.0.113.7", policy: "approval_required", status: "proposed", detail: null }], events: [] };
  if (base === "/claims") return claims.filter((c) => !q.includes("subject_type=") || q.includes(`subject_type=${c.subject_type}`));
  if (base.startsWith("/evidence/")) {
    const key = decodeURIComponent(base.split("/")[3]);
    const cs = claims.filter((c) => c.subject_key === key);
    return { predicates: preds.map((pr) => ({ predicate: pr, current: cs.filter((c) => c.predicate === pr), superseded_count: 0, disagreement: false })) };
  }
  if (base === "/conflicts") return q.includes("status=open") ? conflicts : [];
  if (base.startsWith("/conflicts/")) return detail(conflicts.find((c) => c.id === base.split("/")[2]) ?? conflicts[0]);
  if (base === "/decisions") return q.includes("potentially_affected") ? decisions.filter((d) => d.status === "potentially_affected") : decisions;
  if (base.endsWith("/trace")) return trace;
  if (base === "/users") return users;
  if (base === "/audit/events") return events;
  if (base === "/audit/verify") return { intact: true, events_checked: 30, problem: null, head_seq: 30, head_hash: sha("9") };
  if (base === "/entities/candidates") return { candidates: [{ left: "BRK-100", right: "BRK100", outcome: "possible_match", reasons: ["separator_difference"] }] };
  if (base.startsWith("/compare/")) return { differences: [{ predicate: "mfg.material", from: "S355", to: "S275", change: "changed" }, { predicate: "mfg.coating", from: null, to: "zinc", change: "added" }] };
  return { ok: true };
}

async function mockApi(page) {
  await page.route("**/api/v1/**", (r) => r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(respond(new URL(r.request().url()).pathname + new URL(r.request().url()).search)) }));
}

async function login(page) {
  await page.getByLabel(/Organisation|組織/).fill("acme");
  await page.getByLabel(/^Email|メールアドレス/).fill("aiko.admin@example.test");
  await page.getByLabel(/Password|パスワード/).fill("correct horse battery");
  await page.getByRole("button", { name: /^Sign in$|^サインイン$/ }).click();
  await page.getByRole("heading", { level: 1, name: "Overview" }).waitFor();
}

const VIEWS = [
  ["overview", null, null],
  ["documents", "Evidence", "Documents"],
  ["records", "Evidence", "Suppliers & components"],
  ["revisions", "Evidence", "Revision comparison"],
  ["conflicts", "Conflicts", null],
  ["decisions", "Decisions", "Decision trace"],
  ["affected", "Decisions", "Affected decisions"],
  ["reports", "Reports", null],
  ["users", "Admin", "Users & access"],
  ["integrations", "Admin", "Import settings"],
  ["security", "Admin", "Security monitoring"],
];

async function prepare(page, name) {
  if (name === "records") await page.getByRole("button", { name: /BRK-100/ }).first().click();
  if (name === "revisions") {
    await page.getByLabel("Identifier").fill("BRK-100");
    await page.getByLabel("From revision").fill("C");
    await page.getByLabel("To revision").fill("D");
    await page.getByRole("button", { name: "Compare", exact: true }).click();
  }
  if (name === "conflicts") await page.getByRole("button", { name: "BRK-100" }).first().click();
  if (name === "decisions" || name === "affected") await page.getByRole("button", { name: "Release BRK-100 from Supplier A" }).click();
  await page.waitForTimeout(400);
}

const results = { axe: {}, errors: [], perf: {}, fps: {} };
const browser = await chromium.launch({ args: ["--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader"] });

for (const theme of ["dark", "light"]) {
  for (const width of [1440, 390]) {
    const page = await browser.newPage({ viewport: { width, height: width > 500 ? 900 : 844 }, deviceScaleFactor: 1 });
    await mockApi(page);
    page.on("pageerror", (e) => results.errors.push(`${theme}-${width}: ${e.message}`));
    page.on("console", (m) => m.type() === "error" && results.errors.push(`${theme}-${width}: ${m.text().slice(0, 200)}`));
    await page.goto(BASE);
    if (theme === "light") await page.getByRole("button", { name: "Light theme" }).click();
    await page.waitForTimeout(600);
    await page.screenshot({ path: `${OUT}/login-${theme}-${width}.png` });
    await login(page);
    for (const [name, section, sub] of VIEWS) {
      if (section) await page.getByRole("navigation", { name: "Primary" }).getByRole("button", { name: section }).click();
      if (sub) await page.getByRole("navigation", { name: "Section" }).getByRole("button", { name: sub }).click();
      await prepare(page, name);
      await page.screenshot({ path: `${OUT}/${name}-${theme}-${width}.png`, fullPage: true });
      if (width === 1440) {
        await page.addScriptTag({ content: axeSrc });
        const v = await page.evaluate(async () => (await window.axe.run(document)).violations.map((x) => `${x.id}(${x.nodes.length}): ${x.nodes.slice(0, 3).map((n) => n.target.join(" ")).join(" | ")}`));
        results.axe[`${name}-${theme}`] = v;
      }
    }
    await page.close();
  }
}

// LCP / CLS on Fast 3G (1.6 Mbps down, 150 ms RTT) + 4× CPU slowdown, cold cache.
async function vitals(path, after) {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await ctx.newPage();
  await mockApi(page);
  const cdp = await ctx.newCDPSession(page);
  await cdp.send("Network.enable");
  await cdp.send("Network.emulateNetworkConditions", { offline: false, latency: 150, downloadThroughput: (1.6 * 1024 * 1024) / 8, uploadThroughput: (750 * 1024) / 8 });
  await cdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
  await page.addInitScript(() => {
    window.__v = { lcp: 0, cls: 0 };
    new PerformanceObserver((l) => l.getEntries().forEach((e) => (window.__v.lcp = e.startTime))).observe({ type: "largest-contentful-paint", buffered: true });
    new PerformanceObserver((l) => l.getEntries().forEach((e) => !e.hadRecentInput && (window.__v.cls += e.value))).observe({ type: "layout-shift", buffered: true });
  });
  const t0 = Date.now();
  await page.goto(BASE + path, { waitUntil: "load" });
  if (after) await after(page);
  await page.waitForTimeout(2500);
  const v = await page.evaluate(() => window.__v);
  const js = await page.evaluate(() => performance.getEntriesByType("resource").filter((r) => r.name.endsWith(".js")).map((r) => [r.name.split("/").pop(), r.transferSize]));
  await ctx.close();
  return { lcp_ms: Math.round(v.lcp), cls: Number(v.cls.toFixed(4)), wall_ms: Date.now() - t0, js };
}
results.perf.login = await vitals("/");
results.perf.overview = await vitals("/", async (p) => login(p));

// Frame rate of WebGL scenes (software GL in headless Chromium: a lower bound, not a GPU figure).
async function fps(setup) {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  await mockApi(page);
  await page.goto(BASE);
  await setup(page);
  await page.waitForTimeout(1500);
  const f = await page.evaluate(() => new Promise((res) => { let n = 0; const s = performance.now(); const tick = () => { n++; performance.now() - s < 3000 ? requestAnimationFrame(tick) : res(n / 3); }; requestAnimationFrame(tick); }));
  const gl = await page.evaluate(() => !!document.querySelector("canvas"));
  await page.close();
  return { fps: Math.round(f), webgl_canvas: gl };
}
results.fps.login = await fps(async () => {});
results.fps.overview = await fps(login);
await browser.close();
writeFileSync(`${OUT}/results.json`, JSON.stringify(results, null, 2));
console.log(JSON.stringify(results, null, 2));

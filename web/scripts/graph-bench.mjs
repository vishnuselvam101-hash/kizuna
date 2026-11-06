// Measures the scalable evidence graph in headless Chromium against a mocked, paged API
// (synthetic graphs of 10k and 100k nodes with ~3 links per node). Results feed web/PERF_GRAPH.md.
// Usage: pnpm build && pnpm exec vite preview --port 4318 &
//        PW_CORE=/path/to/playwright-core BASE=http://127.0.0.1:4318 node scripts/graph-bench.mjs [sizes...]
import { createRequire } from "node:module";
import { readFileSync, writeFileSync } from "node:fs";
import { cpus, loadavg } from "node:os";

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PW_CORE ?? "playwright-core");
const BASE = process.env.BASE ?? "http://127.0.0.1:4318";
const OUT = process.env.OUT ?? "graph-bench.json";
const SIZES = process.argv.slice(2).map(Number).filter(Boolean);
const sizes = SIZES.length ? SIZES : [10000, 100000];
const PAGE = 5000;
const axeSrc = readFileSync(new URL("../node_modules/axe-core/axe.min.js", import.meta.url), "utf8");

/** Deterministic synthetic tenant with about `n` nodes and 3n links, served in claim pages. */
function synth(n) {
  const D = Math.max(5, Math.round(n * 0.01)), Dec = Math.max(5, Math.round(n * 0.02)), F = Math.max(2, Math.round(n * 0.005));
  const S = Math.max(5, Math.round(n * 0.1));
  const C = n - D - Dec - F - S;
  const subjOf = (c) => (c * 2654435761) % S;
  const page = (after, limit) => {
    const nodes = [], edges = [];
    const start = after === null ? 0 : Number(after) + 1;
    if (after === null) {
      for (let d = 0; d < D; d++) nodes.push({ id: `doc:${d}`, kind: "document", label: `Spec ${d}.pdf`, tone: d % 17 ? "base" : "warn" });
      for (let d = 0; d < Dec; d++) nodes.push({ id: `dec:${d}`, kind: "decision", label: `Release lot ${d}`, tone: d % 9 ? "ok" : "crit" });
      for (let f = 0; f < F; f++) {
        nodes.push({ id: `fnd:${f}`, kind: "finding", label: `P-${(f * 37) % S}`, tone: "crit" });
        edges.push([`subj:part:P-${(f * 37) % S}`, `fnd:${f}`]);
      }
    }
    const end = Math.min(C, start + limit);
    const seen = new Set();
    for (let c = start; c < end; c++) {
      const s = subjOf(c);
      const sid = `subj:part:P-${s}`;
      nodes.push({ id: `claim:${c}`, kind: "claim", label: `P-${s} mfg.p${c % 23}`, tone: c % 41 ? (c % 5 ? "base" : "ok") : "crit" });
      if (!seen.has(sid)) (seen.add(sid), nodes.push({ id: sid, kind: "subject", label: `P-${s}`, tone: s % 37 === 0 ? "crit" : "info" }));
      edges.push([`doc:${c % D}`, `claim:${c}`], [`claim:${c}`, sid], [`claim:${c}`, `dec:${c % Dec}`]);
      if (c % 4 === 0) edges.push([`claim:${c}`, `dec:${(c * 7 + 3) % Dec}`]);
    }
    return { nodes, edges, next: end < C ? String(end - 1) : null, claims_total: after === null ? C : null };
  };
  return { page, total: { claims: C, subjects: S, docs: D, decisions: Dec, findings: F } };
}

/** Decision trace with `n` evidence items (half changed since the decision). */
function trace(n) {
  const decision = { id: "d1", kind: "supplier_part_release", title: "Release BRK-100", subject_key: "BRK-100", outcome: "approved", status: "potentially_affected", decided_at: "2026-11-04T09:00:00Z", derivation_sha256: "e".repeat(64), rationale: "r" };
  const evidence = Array.from({ length: n }, (_, i) => ({ claim_id: `c${i}`, predicate: `mfg.p${i}`, snapshot_at_decision: { text: "S355" }, current_value: { text: i % 2 ? "S355" : "S275" }, changed: i % 2 === 0 }));
  const changed = Array.from({ length: Math.floor(n / 4) }, (_, i) => ({ claim_id: `x${i}`, predicate: `mfg.p${i * 2}`, value: { text: "S275" }, recorded_at: "2026-11-04T09:00:00Z", event: "recorded_after_decision" }));
  return { decision, derivation_reproducible: true, evidence, invalidations: [], changed_since_decision: changed };
}

async function mock(page, g, tr) {
  await page.route("**/api/v1/**", (r) => {
    const u = new URL(r.request().url());
    const p = u.pathname.replace(/^\/api\/v1/, "");
    let body = [];
    if (p === "/auth/login") body = { access_token: "t", refresh_token: "r", mfa_verified: true };
    else if (p === "/auth/me") body = { user_id: "u1", tenant_id: "t", email: "a@example.test", display_name: "Aiko Admin", role: "tenant_admin", mfa_enabled: true, session_mfa_verified: true };
    else if (p === "/dashboard/summary") body = { artifacts: { accepted: g?.total.docs ?? 1 }, claims: { unreviewed: g?.total.claims ?? 1 }, conflicts: { open: g?.total.findings ?? 0 }, conflict_kinds_open: {}, decisions: { valid: g?.total.decisions ?? 1 }, subjects: { part: g?.total.subjects ?? 1 } };
    else if (p === "/graph/overview") body = g ? g.page(u.searchParams.get("after"), Number(u.searchParams.get("limit") ?? PAGE)) : { nodes: [], edges: [], next: null, claims_total: 0 };
    else if (p === "/decisions") body = tr ? [tr.decision] : [];
    else if (p.endsWith("/trace")) body = tr;
    else if (p === "/audit/verify") body = { intact: true, events_checked: 1, problem: null, head_seq: 1, head_hash: "a".repeat(64) };
    else if (p === "/conflicts") body = [];
    r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
  });
}

async function login(page) {
  await page.getByLabel(/Organisation/).fill("acme");
  await page.getByLabel(/^Email/).fill("a@example.test");
  await page.getByLabel(/Password/).fill("correct horse battery");
  return page.evaluate(() => performance.now());
}

const stats = (page) => page.evaluate(() => {
  const c = document.querySelector(".graph-gl canvas");
  return c?.kizunaGraph ? { ...c.kizunaGraph.stats } : null;
});

/** Frame intervals while the camera orbits every frame (forces a render per frame). */
const frames = (page, ms = 3000) =>
  page.evaluate((ms) => new Promise((res) => {
    const v = document.querySelector(".graph-gl canvas").kizunaGraph;
    const d = [];
    let last = performance.now();
    const t0 = last;
    const tick = (t) => {
      d.push(t - last);
      last = t;
      v.orbit(0.004, 0);
      if (t - t0 < ms) requestAnimationFrame(tick);
      else {
        d.shift();
        d.sort((a, b) => a - b);
        const q = (x) => +d[Math.min(d.length - 1, Math.floor(d.length * x))].toFixed(1);
        res({ frames: d.length, median_ms: q(0.5), p95_ms: q(0.95), fps: +(1000 / (d.reduce((a, b) => a + b, 0) / d.length)).toFixed(1), lod: v.stats.lod, draw_calls: v.stats.drawCalls });
      }
    };
    requestAnimationFrame(tick);
  }), ms);

async function heap(page) {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("HeapProfiler.collectGarbage");
  await cdp.send("Performance.enable");
  const { metrics } = await cdp.send("Performance.getMetrics");
  const m = Object.fromEntries(metrics.map((x) => [x.name, x.value]));
  return { js_heap_used_mb: +(m.JSHeapUsedSize / 1048576).toFixed(1), js_heap_total_mb: +(m.JSHeapTotalSize / 1048576).toFixed(1), dom_nodes: m.Nodes };
}

async function axe(page) {
  await page.addScriptTag({ content: axeSrc });
  return page.evaluate(async () => (await window.axe.run(document)).violations.map((v) => `${v.id} (${v.nodes.length})`));
}

const browser = await chromium.launch({ args: ["--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader"] });
const gpu = await (async () => {
  const p = await browser.newPage();
  const r = await p.evaluate(() => {
    const gl = document.createElement("canvas").getContext("webgl2");
    const ext = gl?.getExtension("WEBGL_debug_renderer_info");
    return gl ? gl.getParameter(ext ? ext.UNMASKED_RENDERER_WEBGL : gl.RENDERER) : "no webgl2";
  });
  await p.close();
  return r;
})();
const results = { date: new Date().toISOString(), host: { cpus: cpus().length, loadavg_start: loadavg().map((x) => +x.toFixed(1)) }, renderer: gpu, viewport: "1440x900 @1x", page_size: PAGE, overview: {}, trace: {}, fallback: {} };

for (const n of sizes) {
  const g = synth(n);
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await ctx.newPage();
  await mock(page, g, null);
  await page.goto(BASE);
  const t0 = await login(page);
  await page.getByRole("button", { name: /^Sign in$/ }).click();
  await page.waitForFunction(() => {
    const s = document.querySelector(".graph-gl canvas")?.kizunaGraph?.stats;
    return s && s.lastDrawAt > 0;
  }, null, { timeout: 600000 });
  const s = await stats(page);
  console.log(n, 'loaded', JSON.stringify(s));
  await page.locator(".graph-gl").screenshot({ path: `graph-${n}-far.png`, timeout: 120000 }).catch(() => {});
  const far = await frames(page);
  console.log(n, 'far', JSON.stringify(far));
  // Zoom onto one record: the near view (individual claims) with chunk frustum culling.
  await page.evaluate(() => {
    const c = document.querySelector(".graph-gl canvas");
    c.kizunaGraph.select(c.kizunaGraph.stats.nodes - 1, true);
  });
  const nearF = await frames(page);
  console.log(n, 'near', JSON.stringify(nearF));
  const mem = await heap(page);
  // Keyboard: focus the canvas, step to the first conflict, the details panel names it.
  await page.locator(".graph-gl canvas").focus();
  await page.keyboard.press("n");
  const detail = await page.locator(".graph-detail b").first().textContent();
  const violations = await axe(page);
  await page.locator(".graph-gl").screenshot({ path: `graph-${n}-near.png`, timeout: 120000 }).catch((e) => console.log('screenshot failed', e.message.split('\n')[0]));
  results.overview[n] = {
    nodes: s.nodes,
    edges: s.edges,
    first_draw_ms: Math.round(s.firstDrawAt - t0),
    all_pages_drawn_ms: Math.round(s.lastDrawAt - t0),
    layout_worker_ms_total: Math.round(s.layoutMs),
    scene_rebuild_ms_total: Math.round(s.rebuildMs),
    frame_far: far,
    frame_near_zoomed: nearF,
    memory: mem,
    keyboard_detail: detail,
    axe: violations,
  };
  console.log(n, JSON.stringify(results.overview[n]));
  await ctx.close();
}

// Decision trace graph (same renderer, static graph) at 1k and 10k evidence items.
for (const n of process.env.SKIP_TRACE ? [] : [1000, 10000]) {
  const tr = trace(n);
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await ctx.newPage();
  await mock(page, null, tr);
  await page.goto(BASE);
  await login(page);
  await page.getByRole("button", { name: /^Sign in$/ }).click();
  await page.getByRole("navigation", { name: "Primary" }).getByRole("button", { name: /Decisions/ }).first().click();
  const t0 = await page.evaluate(() => performance.now());
  await page.getByRole("button", { name: "Release BRK-100" }).first().click();
  await page.waitForFunction(() => document.querySelector(".trace .graph-gl canvas")?.kizunaGraph?.stats.lastDrawAt > 0, null, { timeout: 300000 });
  const s = await page.evaluate(() => ({ ...document.querySelector(".trace .graph-gl canvas").kizunaGraph.stats }));
  await page.locator(".trace .graph-gl canvas").scrollIntoViewIfNeeded();
  const f = await page.evaluate(() => new Promise((res) => {
    const v = document.querySelector(".trace .graph-gl canvas").kizunaGraph;
    const d = [];
    let last = performance.now();
    const t0 = last;
    const tick = (t) => {
      d.push(t - last);
      last = t;
      v.orbit(0.004, 0);
      if (t - t0 < 3000) requestAnimationFrame(tick);
      else (d.shift(), d.sort((a, b) => a - b), res({ median_ms: +d[Math.floor(d.length / 2)].toFixed(1), p95_ms: +d[Math.floor(d.length * 0.95)].toFixed(1) }));
    };
    requestAnimationFrame(tick);
  }));
  results.trace[n] = { nodes: s.nodes, edges: s.edges, graph_drawn_ms: Math.round(s.lastDrawAt - t0), frame: f, memory: await heap(page) };
  console.log("trace", n, JSON.stringify(results.trace[n]));
  await ctx.close();
}

// Fallbacks: reduced motion → no WebGL; the 100k overview becomes the paged table.
{
  const n = sizes[sizes.length - 1];
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, reducedMotion: "reduce" });
  const page = await ctx.newPage();
  await mock(page, synth(n), null);
  await page.goto(BASE);
  const t0 = await login(page);
  await page.getByRole("button", { name: /^Sign in$/ }).click();
  await page.waitForFunction(() => !document.querySelector(".graph-progress") && document.querySelector(".graph-table tbody tr"), null, { timeout: 600000 });
  const t1 = await page.evaluate(() => performance.now());
  results.fallback.reduced_motion = { nodes: n, table_complete_ms: Math.round(t1 - t0), rows_shown: await page.locator(".graph-table tbody tr").count(), memory: await heap(page), axe: await axe(page) };
  console.log("fallback", JSON.stringify(results.fallback.reduced_motion));
  await ctx.close();
}

results.host.loadavg_end = loadavg().map((x) => +x.toFixed(1));
writeFileSync(OUT, JSON.stringify(results, null, 2));
await browser.close();

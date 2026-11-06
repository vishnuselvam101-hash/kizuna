import { fireEvent, render, screen } from "@testing-library/react";
import { GraphTable, SVG_MAX } from "./graph";
import { Layout, SpatialGrid } from "./graphlayout";
import { GraphModel, type GraphPage } from "./graphmodel";
import { I18nProvider } from "./i18n";
import { axeViolations } from "./test-utils";

/** Synthetic overview graph: `parts` records with `per` claims each, docs, decisions citing claims. */
function synth(parts: number, per: number): GraphPage {
  const nodes: GraphPage["nodes"] = [];
  const edges: GraphPage["edges"] = [];
  for (let d = 0; d < 10; d++) nodes.push({ id: `doc:${d}`, kind: "document", label: `doc ${d}`, tone: "base" });
  for (let d = 0; d < 20; d++) nodes.push({ id: `dec:${d}`, kind: "decision", label: `dec ${d}`, tone: d % 3 ? "ok" : "crit" });
  for (let p = 0; p < parts; p++) {
    nodes.push({ id: `subj:part:P${p}`, kind: "subject", label: `P${p}`, tone: "info" });
    for (let c = 0; c < per; c++) {
      const id = `claim:${p}-${c}`;
      nodes.push({ id, kind: "claim", label: `P${p} m${c}`, tone: c === 0 && p % 7 === 0 ? "crit" : "base" });
      edges.push([`doc:${(p + c) % 10}`, id], [id, `subj:part:P${p}`]);
      if (c === 0) edges.push([id, `dec:${p % 20}`]);
    }
  }
  return { nodes, edges };
}

function runLayout(m: GraphModel, l = new Layout()) {
  let out = null as ReturnType<Layout["add"]> | null;
  m.subscribe((b) => (out = l.add(m.kinds, m.tones, b.from, b.to, b.edges, m.expected)));
  return out!;
}

test("layout is deterministic and keeps earlier nodes in place as later pages arrive", () => {
  const g = synth(60, 5);
  const a = runLayout(GraphModel.of(g));
  const b = runLayout(GraphModel.of(g));
  expect(Array.from(a.pos)).toEqual(Array.from(b.pos));
  // Same graph streamed in two pages: claims/records/docs keep their first-page positions.
  const half = Math.floor(g.nodes.length / 2);
  const m = new GraphModel();
  m.expected = g.nodes.length;
  const l = new Layout();
  const first = l.add(m.kinds, m.tones, 0, m.add(g.nodes.slice(0, half), g.edges).to, m.edges.slice(0, m.edgeCount * 2), m.expected);
  const bt = m.add(g.nodes.slice(half), []);
  const second = l.add(m.kinds, m.tones, bt.from, bt.to, bt.edges, m.expected);
  for (let i = 0; i < first.n; i++) {
    if (m.kind(i) === "decision" || m.kind(i) === "finding") continue;
    expect([second.pos[i * 3], second.pos[i * 3 + 1], second.pos[i * 3 + 2]]).toEqual([first.pos[i * 3], first.pos[i * 3 + 1], first.pos[i * 3 + 2]]);
  }
});

test("claims sit beside their record; far view has one cluster per record with record-level links", () => {
  const m = GraphModel.of(synth(40, 6));
  const r = runLayout(m);
  expect(r.cRep.length).toBe(40);
  const p0 = m.indexOf("subj:part:P0")!;
  const c0 = m.indexOf("claim:0-3")!;
  expect(Math.hypot(r.pos[c0 * 3 + 1] - r.pos[p0 * 3 + 1], r.pos[c0 * 3 + 2] - r.pos[p0 * 3 + 2])).toBeLessThan(1.5);
  // Worst member tone wins (P0 has a conflicting claim).
  expect(r.cTone[Array.from(r.cRep).indexOf(p0)]).toBe(0);
  // Far edges: doc→cluster deduplicated, cluster→record, cluster→decision; far fewer than all edges.
  expect(r.farEdges.length / 2).toBeLessThan(m.edgeCount);
  const n = r.n;
  const ends = Array.from(r.farEdges);
  expect(ends.some((x) => x >= n)).toBe(true);
  expect(Math.max(...ends)).toBeLessThan(n + r.cRep.length);
  expect(r.chunks).toBeGreaterThanOrEqual(4);
});

test("grid picking matches a brute-force ray test", () => {
  const m = GraphModel.of(synth(80, 8));
  const r = runLayout(m);
  const rad = new Float32Array(r.n).map((_, i) => 0.5 * r.scale[i]);
  const grid = new SpatialGrid(r.pos, rad, r.n);
  let seed = 7;
  const rnd = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648);
  let hits = 0;
  for (let k = 0; k < 200; k++) {
    // Aim at a random node from a random far point so most rays hit something.
    const target = Math.floor(rnd() * r.n);
    const o: [number, number, number] = [rnd() * 200 - 100, rnd() * 200 - 100, 150];
    const t = [r.pos[target * 3] - o[0], r.pos[target * 3 + 1] - o[1], r.pos[target * 3 + 2] - o[2]];
    const len = Math.hypot(...t);
    const d: [number, number, number] = [t[0] / len, t[1] / len, t[2] / len];
    let best = -1, bestT = Infinity;
    for (let i = 0; i < r.n; i++) {
      const ox = r.pos[i * 3] - o[0], oy = r.pos[i * 3 + 1] - o[1], oz = r.pos[i * 3 + 2] - o[2];
      const tt = ox * d[0] + oy * d[1] + oz * d[2];
      if (tt >= 0 && ox * ox + oy * oy + oz * oz - tt * tt <= rad[i] * rad[i] && tt < bestT) (bestT = tt), (best = i);
    }
    expect(grid.pick(o, d)).toBe(best);
    if (best >= 0) hits++;
  }
  expect(hits).toBeGreaterThan(150);
});

test("large graphs without WebGL fall back to a paged, filterable table (axe clean)", async () => {
  const m = GraphModel.of(synth(120, 4));
  expect(m.size).toBeGreaterThan(SVG_MAX);
  const picked: string[] = [];
  const { container } = render(
    <I18nProvider initial="en">
      <GraphTable m={m} label="Evidence graph" onPick={(id) => picked.push(id)} />
    </I18nProvider>,
  );
  expect(screen.getByText(`Rows 1–50 of ${m.size}`)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Next rows" }));
  expect(screen.getByText(`Rows 51–100 of ${m.size}`)).toBeTruthy();
  fireEvent.change(screen.getByLabelText("Kind"), { target: { value: "subject" } });
  expect(screen.getByText("Rows 1–50 of 120")).toBeTruthy();
  fireEvent.change(screen.getByLabelText("Search labels"), { target: { value: "P119" } });
  expect(screen.getByText("Rows 1–1 of 1")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Open" }));
  expect(picked).toEqual(["subj:part:P119"]);
  expect(await axeViolations(container)).toEqual([]);
});

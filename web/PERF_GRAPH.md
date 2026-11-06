# Evidence graph at scale: measurements

Branch `webgl-scale`, measured 2026-11-06 on the production build (`pnpm build`, `vite preview`)
with `scripts/graph-bench.mjs`. Every number below comes from that run (`final.json`); nothing is
extrapolated.

## What changed

- **Data**: `GET /api/v1/graph/overview?after&limit` (new, `crates/kizuna-api/src/graph.rs`, tests in
  `tests/graph_overview.rs`): keyset-paged by claim id, 5000 claims per page by default. The
  Overview streams pages until `next` is absent; the old cap of 300 claims is gone. Edges whose
  far end has not arrived yet wait in the client model (`src/graphmodel.ts`).
- **Layout** (`src/graphlayout.ts`, run in `src/graph.worker.ts`): a layered DAG (documents →
  claims → records → decisions and findings) with each layer on a seeded sunflower spiral. It is
  deterministic and incremental: nodes already placed keep their position when later pages arrive.
  Decisions and findings follow the barycentre of their evidence. Claims sit around their
  record's slot, so clustering by record (part or supplier) is spatial.
- **Rendering** (`src/scene3d.ts`, lazy three.js chunk): one `InstancedMesh` per spatial chunk (a
  16×16 y–z grid per layer, frustum-culled per chunk). Links are merged `LineSegments` per chunk,
  and their opacity scales with density.
  - **Far zoom** (when a node would be under 1.5 px, graphs above 4000 nodes): claims collapse
    into one marker per record, sized by claim count and coloured by its worst tone. Links collapse
    to record level, deduplicated and capped at 20,000 by even sampling.
  - **Near zoom** on big graphs: only links leaving chunks near the camera target are drawn. The
    far plane closes in with fog, so chunks behind the view are culled.
  - **Rendering loop**: frames are drawn on demand, with no loop while idle. The gentle drift is
    kept only for graphs of 3000 nodes or fewer when motion is allowed. Rendering pauses when the
    tab is hidden or the canvas is off-screen.
  - **Big graphs** (> 4000 nodes) render without MSAA and with Lambert shading; small graphs keep
    the original MSAA and standard material.
- **Picking**: a uniform-grid spatial index with a 3D DDA ray walk (`SpatialGrid`), so hovering
  and clicking cost O(cells crossed), not O(n). A unit test checks it against a brute-force ray
  test on 200 rays. The far view picks record clusters.
- **Access**:
  - **Keyboard**: the canvas is focusable (`role="application"`, with the key help linked by
    `aria-describedby`). Arrow keys rotate, +/− zoom, 0 resets, N/P step through nodes (conflicts
    first), Enter opens and Escape clears.
  - **Details panel**: a live region names the selected node, its kind, status and link count, with
    an Open button and the linked nodes as buttons, so the whole graph can be walked by keyboard.
  - **No WebGL or reduced motion**: graphs of 400 nodes or fewer keep the SVG drawing. Larger
    ones become a paged (50 rows), filterable table with Open buttons.

## Environment

The host is a Linux VPS with 8 vCPUs and **no GPU**, shared with other build jobs: the load
average was **16–18** for the whole run (more than two runnable threads per core). Chromium was
headless (Playwright 1.64.1, build 1248) with WebGL2 on
`ANGLE (Vulkan 1.3.0 (SwiftShader Device (Subzero)))`, i.e. **software rendering on the CPU**. The
viewport was 1440×900 at 1× and the canvas 1358×360 CSS px.

The API was mocked in the browser with a synthetic tenant. Per n nodes it has 1% documents, 2%
decisions, 0.5% open findings, 10% records and the rest claims. Each claim has links to a document,
its record and a decision, plus a second decision for every fourth claim, which gives about 2.8
links per node. The paged endpoint itself is covered by the Rust integration tests against
PostgreSQL. It was not load-tested with 100k real claims.

```
pnpm build && pnpm exec vite preview --port 4318 &
PW_CORE=<playwright-core path> BASE=http://127.0.0.1:4318 node scripts/graph-bench.mjs 300 10000 100000
```

## Overview graph

| | 300 nodes | 10k nodes | 100k nodes |
|---|---|---|---|
| Links | 838 | 28,163 | 281,625 |
| Pages (5000 claims each) | 1 | 2 | 18 |
| First nodes drawn (from Sign in) | 2278 ms¹ | 580 ms | 2785 ms |
| All pages drawn | 2278 ms¹ | 664 ms | 6536 ms |
| Layout worker time (sum) | 14 ms | 58 ms | 529 ms |
| Main-thread scene rebuilds (sum) | 15 ms | 92 ms | 999 ms |
| Frame time, whole graph (mean fps · median / p95 ms) | 29 fps · 33 / 67 | 23 fps · 50 / 83 (far view) | 11 fps · 17 / 550 (far view) |
| Frame time, zoomed onto one record | 31 fps · 33 / 50 | 11 fps · 67 / 250 (near) | **1.3 fps** · 817 / 1033 (near) |
| Draw calls (whole / zoomed) | 26 / 24 | 216 / 263 | 445 / 71 |
| JS heap after load (after GC) | 5.3 MB | 8.1 MB | 26.0 MB |
| In-browser axe-core 4.10.2 (whole page) | 0 violations | 0 | 0 |
| Keyboard check (focus canvas, N) | details panel names `P-0` | same | same |

¹ The 300-node run was the browser's first, so it includes the cold load of the 128 KB three.js
chunk on a loaded host. The 10k run came after it.

Frame times were measured with the camera orbiting every animation frame, which forces a render
on each one. The p95 values are spiky because SwiftShader shares the CPU with the page and the
other jobs, so mean fps is the steadier figure.

**Honest reading:**

- Software rendering at 100k is usable only zoomed out (about 11 fps). Zoomed in, the near view
  draws tens of thousands of instanced octahedra on the CPU, at about 1 fps.
- On a real GPU this workload is small: 100k instances × 24 vertices plus ≤ 280k line segments in
  ≤ 445 draw calls. But **GPU frame times were not measured here**, and neither the 60 fps target
  nor any GPU memory figure has been confirmed.
- Loading, layout and picking (CPU work) are the measured parts.

## Decision trace graph

The trace graph uses the same renderer, fed a static graph. The figures below are the trace screen
with n evidence items, half of them changed, plus n/4 later changes.

| | 1k evidence (1251 nodes) | 10k evidence (12,501 nodes) |
|---|---|---|
| Graph drawn after selecting the decision | 546 ms | 5951 ms |
| Frame time (median / p95) | 33 / 83 ms | 17 / 267 ms |
| JS heap | 9.1 MB | 38.9 MB |
| DOM nodes | 15,836 | 155,336 |

At 10k evidence the time and memory go into the trace screen's evidence table, which renders every
row (155k DOM nodes), not into the graph. Paging that table is a follow-up outside this branch.

## Fallbacks

- **Reduced motion** (no WebGL scene), 100k-node Overview: the table is complete after all 18
  pages in 4130 ms. It shows 50 rows, the page has 745 DOM nodes and the JS heap is 20.1 MB.
  axe-core reports 0 violations.
- **Small graphs without WebGL** (≤ 400 nodes): the SVG drawing, unchanged. The unit tests cover
  the what-if highlight.
- **jsdom tests** (`src/graph.test.tsx`, `src/workflow.test.tsx`):
  - deterministic and stable layout across pages;
  - clusters and far links;
  - grid picking matches brute force;
  - table paging, filter, search and Open, with axe clean;
  - page streaming with late edges and deduplication.

## Bundle (gzip, `vite build`)

| Chunk | Before (main 3c3df2a) | After | Budget |
|---|---|---|---|
| `index.js` (app shell) | 133.96 KB | 137.30 KB | ≤ 180 KB |
| `scene3d.js` (three.js, lazy) | 123.7 KB | 127.9 KB | lazy only |
| `graph.worker.js` (layout, lazy) | — | 4.9 KB raw, 2.0 KB gzip | lazy only |

## Not measured

- GPU frame rate and GPU memory, and laptop or mobile GPUs.
- The real API serving 100k claims: the endpoint was only tested functionally.
- Network latency.
- Lighthouse.

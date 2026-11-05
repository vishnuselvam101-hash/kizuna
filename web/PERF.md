# Web UI performance and accessibility measurements

Measured 2026-11-05 on the production build (`pnpm build`, `vite preview`) with the API mocked by
`scripts/ui-review.mjs` (synthetic data: 5 documents, 25 claims, 3 conflicts, 2 decisions, 3 users).

Environment: Linux VPS, 8 vCPU, no GPU; Node 24.22.2; headless Chromium (Playwright build 1248)
with SwiftShader software WebGL. Command:

```
pnpm build && pnpm exec vite preview --port 4317 &
PW_CORE=<playwright-core path> OUT=<dir> node scripts/ui-review.mjs
```

## Bundle (gzip, from `vite build`)

| Chunk | Size | Loaded on |
|---|---|---|
| `index.js` (app shell, all screens) | 76.4 KB | every route (budget 180 KB) |
| `index.css` | 4.8 KB | every route |
| `scene3d.js` (three.js 0.170.0, MIT) | 122.7 KB | dynamic import only by the login hero, Overview graph and decision trace graph, and only when WebGL2 is available, motion is allowed and the device is not low-power |
| Fonts (woff2, latin subsets; Japanese via unicode-range slices) | ~13–21 KB each | on first use; `font-display: swap` |

## Web vitals — Fast 3G (1.6 Mbps, 150 ms RTT) + 4× CPU throttling, cold cache, 1440×900

| Page | LCP | CLS | JS transferred |
|---|---|---|---|
| Login (hero) | 1044 ms | 0.0065 | 76.0 KB shell + 122.1 KB scene (lazy, after first paint) |
| Overview after sign-in | 1036 ms | 0 | same |

Budget: LCP < 2.0 s, CLS < 0.05 — met in this run.

## Frame rate (requestAnimationFrame over 3 s)

| Scene | fps |
|---|---|
| Login knot (full viewport) | 34 |
| Overview evidence graph | 60 |

These are software-rendered (SwiftShader, no GPU) and are a lower bound. 60 fps on a mid-range
laptop GPU is **not measured** here. Rendering stops when the tab is hidden or the canvas is
off-screen; device pixel ratio is capped at 1.5; motion is transform-only.

## Accessibility

- In-browser axe-core 4.10.2 (all rules, including colour contrast): **0 violations** on all 10
  screens in dark and light themes at 1440 px (20 runs).
- jsdom axe checks (`src/a11y.test.tsx`, colour contrast excluded there) on login (en/ja) and
  seven screens: 0 violations.
- Screens screenshot-reviewed at 1440 and 390 px in dark and light (44 screenshots).

## Not measured

- Lighthouse score (not installed in this environment).
- Real GPU frame rate; real network latency to the API.

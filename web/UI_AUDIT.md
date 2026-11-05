# KIZUNA web UI audit (ui-polish, from de315db)

Method: `pnpm build && vite preview`, then `scripts/ui-review.mjs` (mocked API, synthetic data) with
Playwright/Chromium at 1440 and 390 px, dark and light, plus Japanese captures of Overview, Conflicts,
Security and Decisions. All screenshots were viewed. Axe runs in the page on every screen at 1440.

## Review harness (fixed first)

| # | Finding | Fix |
|---|---|---|
| H1 | The review script was out of date: no `/dashboard/summary`, `/subjects`, import-settings or security mocks, so Overview crashed to a **blank page** after sign-in (`Cannot read properties of undefined (reading 'accepted')`) and the run aborted. | Mocks added for every endpoint the UI calls, Security screen added, page errors and console errors are now recorded in `results.json`, stale locators fixed. |
| H2 | Mock claims had no `artifact_display_name`, which the real API returns, so sources showed as "Document a1…". | Mock now matches the API. |

## Findings and fixes

| # | Screen | Finding (before) | After |
|---|---|---|---|
| 1 | All | Status colours clashed. `warn` amber (#e7a33e) and `ok` gold (#d8b26a) looked the same, so "Unreviewed", "Possible match", "Not enrolled", "Changed" and "Added" read as **verified**. | Gold now means verified only. `warn` is a dashed amber outline for anything that needs a person. A new `neutral` tone covers pending or plain states (unreviewed, active, pending scan). "Removed" is now crit and "Added" is info. |
| 2 | Overview | KPI value colours were off. "Documents 5" was amber while the only warning was the sub-line, and the audit-chain "Intact" was not shown as verified. | Only the sub-line is amber ("1 not accepted"). "Intact" is gold. |
| 3 | Overview / trace | The WebGL graph was framed by hand. It sat left of centre with a third of the panel empty, nodes were clipped at the top and bottom (1440) and at the sides (390), and the trace node was clipped while swinging. | The structure is centred on its bounding box. The camera distance is solved each frame from aspect, FOV and the y-swing, so nothing clips at any width. |
| 4 | Overview | The attention lists wrapped the "why" text under the badge with no hanging indent. | Badge and ID sit on the first line and the explanation on a muted second line. |
| 5 | All tables | Cells were top-aligned while buttons and badges were centred, so rows looked misaligned. The left "keyboard row" bar showed on row 1 of every table even without focus, which looked like a stray selection. Action buttons floated mid-row. | Cells are middle-aligned. The active-row bar shows only when the table has focus. Actions are right-aligned. The last row has no double border. |
| 6 | Decisions | The title column collapsed to one word per line ("Release / BRK- / 100 / from …") and dates wrapped onto 3 lines. | The title has a minimum width. Dates, IDs and hashes never wrap. The redundant Outcome column is dropped from the list (it is still in the detail panel). |
| 7 | Records / Conflicts | "(not a probability of truth)" was repeated in every row, which made rows wrap to 2 lines. | Cells read "Manual · 90%". The caveat appears once as a note under the table, and the full sentence stays in the conflict's "Confidence and limitations" section. |
| 8 | Documents | Sizes were raw ("20,480") under a "Bytes" header. | Sizes read "20.0 KB" under a "Size" header, with exact bytes in the tooltip. |
| 9 | Documents | The native "Choose File / No file chosen" button was unstyled and looked foreign in the dark UI. | `::file-selector-button` is styled to match. |
| 10 | Documents / Reports | A disabled primary button looked like a dim but active teal button. | A disabled primary is an outline with muted text. |
| 11 | Reports @390 | The page scrolled horizontally (448 px wide) because the decision `<select>` took its width from the longest option. | Labels and controls in a form can shrink (`minmax(0,1fr)`, `min-width:0`). No 390 screen overflows now. |
| 12 | Shell | Changing screens focused `<main>` with scrolling allowed. The browser scrolled the page so the H1 sat under the sticky top bar. This was visible on Import settings and on every Evidence sub-page at 390. | `focus({preventScroll})` plus a scroll to the top. |
| 13 | Shell | The user chip was cut off ("Aiko Admin · Administra…") at 1440. | The width allows the full name and role. |
| 14 | Shell | The "Light theme" toggle looked the same in both themes. Only `aria-pressed` changed, so in light mode it read as an instruction. | The toggle has a visible state dot (filled teal when on). |
| 15 | Shell @390 | Primary and section navs scroll horizontally with no affordance (Reports and Admin were hidden). | A fade mask on the right edge shows there is more. |
| 16 | Security | Raw enum values were shown: "high", "open", "brute force", lowercase and not translated (JA also showed English). | EN and JA labels added for every severity, rule, incident status, action kind and action status (from `security-detection` and `0005_defensive.sql`). Only i18n was changed. |
| 17 | Security (JA) | KPI labels were count fragments ("件の未解決インシデント") shown above the number. | Now proper labels: 未解決のインシデント / 承認待ちの対応 / 有効なアドレス遮断. |
| 18 | Records / Conflicts | Source locations were English in JA ("page 1", "row 14, col 3"). | Localised: 1ページ目 / 14行目・3列目 (EN: page 1, row 14, col 3). |
| 19 | JA copy | Some strings read as translationese. "発火します" ("fires") became 出典が1つでも検出されます. "記録済み {n}件中" became 全{n}件中. "承認（記録付き特例）" became 特例として承認. "該当しなくなった" became 対象外. "バイト" became サイズ. | Fixed. The rest of the JA dictionary was read and reads naturally. |
| 20 | Tables (JA) | Mono IDs broke at the hyphen ("BRK- / 100") in narrow columns. | IDs and code in cells do not wrap. |

## Left as is (deliberate)
- The login hero, knot mark, grid canvas and type scale are on-identity. No generic gradients, no emoji and no glow spam were found.
- Wide tables scroll inside `.table-wrap` at 390. This is the intended pattern, and the page itself never scrolls sideways.
- Native date inputs show the browser-locale format (`mm/dd/yyyy` in en-US Chromium). A custom picker is not worth the JS.

## For the admin AI opt-in / detection-threshold agent (not touched here)
- **Import settings (`IntegrationsView`)**: numeric ranges show as a bare "1–200" hint under the field. Prefer "1–200 MB" with the unit, and set `aria-describedby` on the input. The column-mapping inputs are mono while the others are sans. Use one style.
- **Import settings**: "Save settings" (primary), "Export settings (JSON)" and "Reset to defaults" sit in one row. Reset should be visually separated or confirm, because it is destructive to unsaved edits.
- **Security (`SecurityView`)**: the "Last seen" cell lacks `className: "num"` (sans and wrapping, while every other date is mono and nowrap). In JA the Rule column wraps to 4 lines at 390. Give it `nowrap` or a min width.
- **Security**: the incident table "Summary" text comes from the server in English only. If thresholds or rules gain new names, add `sec.rule.<name>` in both dictionaries (the fallback shows snake_case with spaces).
- Use the shared tones: `Badge tone="warn"` (dashed) for "needs a person", `ok` (gold) only for verified, and `neutral` for plain states. Do not colour a KPI number amber when only its sub-line is the warning.
- **Reports audit trail**: Actor and Target show short IDs ("user u1", "artifact a1"). Resolving names needs an API join (see API_GAPS.md).

## Measured (scripts/ui-review.mjs, headless Chromium, software GL)
- Axe: 0 violations on all 11 screens × dark/light (before: 0 on the screens that rendered; Overview previously crashed under the review mocks).
- Page errors / console errors: 0.
- App JS: index 80.6 KB gzip (budget 180 KB; was 79.5 KB). scene3d is lazy, 123.6 KB gzip.
- Fast-3G + 4× CPU: LCP login 1088 ms, overview 1152 ms. CLS 0.0065 / 0.
- No 390 px screen is wider than the viewport (before: Reports was 448 px).

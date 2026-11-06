# Japanese extraction v3: layout readers, measured honestly (2026-11-06)

Branch `ja-v3` (from main 3c3df2a). Follows `JA_EXTRACTION_V2_2026-11-05.md`. Everything here is SIMULATED:
synthetic documents with ground truth. No real supplier documents were used.

## Headline

The independent holdout barely moved. Text recall went from 0.174 to **0.180**, and scan recall went from 0.057 to
**0.044**. The new readers handle the layouts I generated for dev (dev recall 0.47 → 0.90), but they do not handle
the holdout author's versions of those layouts. Tolerance and measured values on the holdout are still 0.00. The
dev gain measures tuning, not generalisation.

## Splits and procedure

| split | source | role |
|---|---|---|
| train | `tests/ja_corpus.py` (30 docs) | unchanged |
| dev | old dev (10) + **`tests/ja_dev3.py` (28 new docs)** | tuning |
| dev_scan | old scan set (8) | OCR tuning |
| holdout | `tests/ja_holdout.py` (40) | report only |
| holdout_scan | `tests/ja_holdout.py` (12 scans) | report only |

`ja_dev3.py` was written from the layout families that the v2 report names: horizontal tables, labels above
values, values above labels and several parts in one document. `ja_holdout.py` was not opened. No label from the
v2 report's list of holdout labels was added to any vocabulary. The dev3 families are:

- `htable`: ruled and unruled tables, tight right-aligned ruled cells, a vertically merged `測定値` label, a `単位`
  row and English row labels.
- `above`: boxed strips, label/value stacks and value-above-caption boxes.
- `multipart`: 2–3 parts per document. Truth carries `subject`.
- `nearmiss`: traps, namely a customer/old part number, an empty box, a range row, a missing measurement,
  bracketed item notes and footer numbers.
- `sandwich`: 4 page images with a hidden text layer. About 18 % of the numeric strings in each layer are one
  character wrong, and the truth is the printed value.

The holdout was run exactly once, after all code, the dev set and the calibration fit were final. Nothing was
changed after that run. "Before" is main 3c3df2a's worker and mapper, scored with the same scorer and corpus.

Scorer change: when a truth entry carries `subject` (multipart dev docs), a claim must be about that part.

Reproduce with:

    uv run python tests/ja_eval.py --holdout --rust target/debug/examples/map_records --out r.json
    uv run python tests/ja_calibrate.py r.json

Per-field JSON for all splits is in `reports/ja/ja_v3_eval.json`.

## Results (strict, micro precision / recall, Rust production path)

| split | before (3c3df2a, mfg-map@5) | after (ja-v3, mfg-map@6) |
|---|---|---|
| train | 1.000 / 0.959 | 1.000 / 0.959 (every field unchanged) |
| dev | 0.598 / 0.468 | 0.924 / 0.899 |
| dev_scan | 0.955 / 0.530 | 0.955 / 0.530 |
| **holdout** | 0.709 / 0.174 | **0.699 / 0.180** |
| **holdout_scan** | 0.619 / 0.057 | **0.556 / 0.044** |

Worker claims: holdout 0.533 / 0.220 → 0.535 / 0.226, and holdout_scan 0.413 / 0.079 → 0.381 / 0.067.

Most of the 50 dev false positives come from the 4 `sandwich` documents (49). In two of them the hidden layer misspells
the part number, so every value lands on the wrong subject. All 49 are flagged for review (see Calibration).

### Per field (Rust, strict)

Train is identical for every field before and after, so it is not repeated here.

#### dev

| field | n | before P / R | after P / R |
|---|---|---|---|
| approval_date | 2 | 1.00 / 1.00 | 1.00 / 1.00 |
| approval_status | 2 | 1.00 / 1.00 | 1.00 / 1.00 |
| approved_by | 10 | 1.00 / 0.40 | 1.00 / 0.90 |
| checked_by | 2 | 1.00 / 1.00 | 1.00 / 1.00 |
| delivery_date | 5 | 1.00 / 1.00 | 1.00 / 1.00 |
| dimension.height | 15 | 0.25 / 0.13 | 1.00 / 0.93 |
| dimension.inner_diameter | 11 | 0.83 / 0.46 | 1.00 / 0.91 |
| dimension.length | 21 | 0.78 / 0.33 | 1.00 / 0.91 |
| dimension.outer_diameter | 15 | 0.83 / 0.33 | 1.00 / 0.87 |
| dimension.thickness | 22 | 0.67 / 0.36 | 1.00 / 0.95 |
| dimension.width | 17 | 0.60 / 0.35 | 1.00 / 0.94 |
| drawing_number | 12 | 1.00 / 0.83 | 1.00 / 1.00 |
| drawn_by | 10 | 1.00 / 0.40 | 1.00 / 0.90 |
| inspection_date | 16 | 1.00 / 0.88 | 1.00 / 0.88 |
| issue_date | 28 | 1.00 / 0.46 | 1.00 / 0.93 |
| material | 29 | 0.54 / 0.52 | 0.96 / 0.93 |
| measured.height | 16 | 0.18 / 0.19 | 1.00 / 0.88 |
| measured.inner_diameter | 9 | 0.27 / 0.33 | 1.00 / 0.78 |
| measured.length | 30 | 0.26 / 0.30 | 1.00 / 0.87 |
| measured.outer_diameter | 21 | 0.11 / 0.14 | 1.00 / 0.81 |
| measured.thickness | 30 | 0.35 / 0.40 | 1.00 / 0.93 |
| measured.width | 15 | 0.19 / 0.20 | 1.00 / 0.87 |
| part_name | 50 | 0.80 / 0.70 | 1.00 / 0.92 |
| property.chem_c | 3 | 1.00 / 1.00 | 1.00 / 1.00 |
| property.chem_mn | 3 | 1.00 / 1.00 | 1.00 / 1.00 |
| property.chem_p | 3 | 1.00 / 1.00 | 1.00 / 1.00 |
| property.chem_s | 3 | 1.00 / 1.00 | 1.00 / 1.00 |
| property.chem_si | 3 | 1.00 / 1.00 | 1.00 / 1.00 |
| property.elongation | 3 | 1.00 / 1.00 | 1.00 / 1.00 |
| property.hardness | 3 | 1.00 / 1.00 | 1.00 / 1.00 |
| property.surface_roughness | 2 | 1.00 / 1.00 | 1.00 / 1.00 |
| property.tensile_strength | 3 | 1.00 / 1.00 | 1.00 / 1.00 |
| property.yield_strength | 3 | 1.00 / 1.00 | 1.00 / 1.00 |
| quantity | 7 | 1.00 / 1.00 | 1.00 / 1.00 |
| result | 56 | 1.00 / 0.38 | 1.00 / 0.80 |
| revision | 14 | 1.00 / 0.50 | 1.00 / 0.79 |
| scale | 2 | 1.00 / 1.00 | 1.00 / 1.00 |
| stamp.approved | 10 | 1.00 / 0.40 | 1.00 / 0.90 |
| stamp.checked | 2 | 1.00 / 1.00 | 1.00 / 1.00 |
| stamp.drawn | 10 | 1.00 / 0.40 | 1.00 / 0.90 |
| standard | 7 | 1.00 / 0.29 | 1.00 / 0.71 |
| supplier | 32 | 1.00 / 0.62 | 1.00 / 0.91 |
| surface_treatment | 10 | 1.00 / 0.80 | 1.00 / 0.80 |
| tolerance | 101 | 0.65 / 0.33 | 1.00 / 0.92 |
| unit_price | 5 | 0.80 / 0.80 | 0.80 / 0.80 |

#### holdout

| field | n | before P / R | after P / R |
|---|---|---|---|
| approval_date | 8 | 0.00 / 0.00 | 0.00 / 0.00 |
| approval_status | 8 | 0.00 / 0.00 | 0.00 / 0.00 |
| approved_by | 8 | 1.00 / 0.12 | 1.00 / 0.12 |
| checked_by | 8 | 0.50 / 0.12 | 0.50 / 0.12 |
| delivery_date | 29 | 1.00 / 0.31 | 1.00 / 0.31 |
| dimension.height | 8 | 0.00 / 0.00 | 0.00 / 0.00 |
| dimension.inner_diameter | 8 | 0.00 / 0.00 | 0.00 / 0.00 |
| dimension.length | 11 | 0.00 / 0.00 | 0.00 / 0.00 |
| dimension.outer_diameter | 13 | 0.00 / 0.00 | 0.00 / 0.00 |
| dimension.thickness | 12 | 0.00 / 0.00 | 0.00 / 0.00 |
| dimension.width | 9 | 0.00 / 0.00 | 0.00 / 0.00 |
| drawing_number | 16 | 1.00 / 0.12 | 1.00 / 0.12 |
| drawn_by | 8 | 1.00 / 0.12 | 1.00 / 0.12 |
| inspection_date | 8 | 1.00 / 0.12 | 1.00 / 0.12 |
| issue_date | 30 | 1.00 / 0.20 | 1.00 / 0.20 |
| material | 24 | 0.40 / 0.08 | 0.50 / 0.12 |
| measured.height | 18 | 0.00 / 0.00 | 0.00 / 0.00 |
| measured.inner_diameter | 21 | 0.00 / 0.00 | 0.00 / 0.00 |
| measured.length | 8 | 0.00 / 0.00 | 0.00 / 0.00 |
| measured.outer_diameter | 17 | 0.00 / 0.00 | 0.00 / 0.00 |
| measured.thickness | 20 | 0.00 / 0.00 | 0.00 / 0.00 |
| measured.width | 14 | 0.00 / 0.00 | 0.00 / 0.00 |
| part_name | 61 | 1.00 / 0.36 | 0.92 / 0.38 |
| property.chem_c | 8 | 1.00 / 0.12 | 1.00 / 0.12 |
| property.chem_mn | 8 | 1.00 / 0.12 | 1.00 / 0.12 |
| property.chem_p | 8 | 1.00 / 0.12 | 1.00 / 0.12 |
| property.chem_s | 8 | 1.00 / 0.12 | 1.00 / 0.12 |
| property.chem_si | 8 | 1.00 / 0.12 | 1.00 / 0.12 |
| property.elongation | 8 | 0.00 / 0.00 | 0.00 / 0.00 |
| property.hardness | 8 | 1.00 / 0.25 | 1.00 / 0.25 |
| property.surface_roughness | 8 | 0.00 / 0.00 | 0.00 / 0.00 |
| property.tensile_strength | 8 | 0.00 / 0.00 | 0.00 / 0.00 |
| property.yield_strength | 8 | 0.00 / 0.00 | 0.00 / 0.00 |
| quantity | 37 | 0.96 / 0.59 | 0.96 / 0.59 |
| result | 16 | 1.00 / 0.06 | 0.40 / 0.12 |
| revision | 24 | 1.00 / 0.12 | 1.00 / 0.12 |
| scale | 8 | 1.00 / 0.25 | 1.00 / 0.25 |
| standard | 8 | 0.17 / 0.12 | 0.17 / 0.12 |
| supplier | 36 | 1.00 / 0.36 | 0.88 / 0.39 |
| surface_treatment | 16 | 1.00 / 0.06 | 1.00 / 0.06 |
| tolerance | 51 | 0.00 / 0.00 | 0.00 / 0.00 |
| unit_price | 29 | 1.00 / 0.76 | 1.00 / 0.76 |

#### holdout_scan

| field | n | before P / R | after P / R |
|---|---|---|---|
| approval_date | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| approval_status | 3 | 1.00 / 0.33 | 1.00 / 0.33 |
| approved_by | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| checked_by | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| dimension.height | 4 | 0.00 / 0.00 | 0.00 / 0.00 |
| dimension.inner_diameter | 2 | 0.00 / 0.00 | 0.00 / 0.00 |
| dimension.length | 5 | 0.00 / 0.00 | 0.00 / 0.00 |
| dimension.outer_diameter | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| dimension.thickness | 4 | 0.00 / 0.00 | 0.00 / 0.00 |
| dimension.width | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| drawing_number | 6 | 0.00 / 0.00 | 0.00 / 0.00 |
| drawn_by | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| fax.number | 9 | 1.00 / 0.11 | 1.00 / 0.11 |
| fax.sender | 9 | 0.50 / 0.11 | 0.50 / 0.11 |
| fax.sent_at | 9 | 1.00 / 0.22 | 1.00 / 0.22 |
| inspection_date | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| issue_date | 6 | 1.00 / 0.17 | 0.00 / 0.00 |
| material | 9 | 0.33 / 0.11 | 0.33 / 0.11 |
| measured.height | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| measured.inner_diameter | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| measured.length | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| measured.outer_diameter | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| measured.thickness | 10 | 0.00 / 0.00 | 0.00 / 0.00 |
| measured.width | 10 | 0.00 / 0.00 | 0.00 / 0.00 |
| part_name | 12 | 0.50 / 0.08 | 0.00 / 0.00 |
| property.chem_c | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| property.chem_mn | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| property.chem_p | 3 | 1.00 / 0.33 | 1.00 / 0.33 |
| property.chem_s | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| property.chem_si | 3 | 1.00 / 0.33 | 1.00 / 0.33 |
| property.elongation | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| property.hardness | 5 | 0.00 / 0.00 | 0.00 / 0.00 |
| property.surface_roughness | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| property.tensile_strength | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| property.yield_strength | 3 | 0.50 / 0.33 | 0.50 / 0.33 |
| quantity | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| result | 6 | 0.00 / 0.00 | 0.00 / 0.00 |
| revision | 9 | 1.00 / 0.22 | 1.00 / 0.11 |
| scale | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| stamp.approved | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| stamp.checked | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| stamp.drawn | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| standard | 3 | 0.00 / 0.00 | 0.00 / 0.00 |
| supplier | 6 | 0.00 / 0.00 | 0.00 / 0.00 |
| surface_treatment | 6 | 0.00 / 0.00 | 0.00 / 0.00 |
| tolerance | 19 | 0.00 / 0.00 | 0.00 / 0.00 |

On the holdout, material, part_name, supplier and result gained a little. Result is the only field that moved
noticeably (0.06 → 0.12), and its precision fell to 0.40. Holdout scans lost three true values.

## What changed

1. **Horizontal tables** (`geometry.horizontal_tables`). Items are the columns, and there is one row per role
   (規格値/公差/測定値/判定, `Nominal`, `Tol.`, `X1`, `Judge`).
   - Header: either `検査項目 | 外径 | …`, or the item names alone above a row whose role label sits left of them.
   - Columns: the vertical ruling lines around each item cell when the PDF draws them, else halfway between the
     item cells.
   - Merged cells: a value spanning several columns applies to each of them. A vertically merged `測定値` label is
     followed by `2`, `3` or no label.
   - Units: one other row (規格範囲, limits) is skipped, a `単位` row gives units, and a bracketed note that is not
     a unit is dropped from the item name.
2. **Ruling lines** (`documents.PageGraphics`). Stroked horizontal/vertical segments and thin rectangles are read
   from the content stream, capped at 4000 per page. A vertical rule between two fragments always starts a new
   cell.
3. **Label/value strips** (`geometry.strip`). A line of labels with a line of values directly below or above it,
   paired by horizontal overlap. A single label must be in the vocabulary.
4. **Title blocks** (`drawing.title_block`). Any vocabulary label standing alone in a cell takes its value from
   the cell to the right, from below, or (new) from above. Lines already read as tables or strips are skipped,
   and so are values that are themselves labels. This removed garbage pairs such as `図番 = 品名`.
5. **Part sections** (Rust `mfg-map@6`). When the `label: value` fields name two or more different parts, every
   positioned value is about the nearest part header above it.
   - Header fields before the first part apply to every part.
   - Claims carry `section`.
   - A guessed (fallback) part label still opens a section, but stays flagged.
6. **Hidden text layers** (`worker.verify_hidden_text`). A page whose only text is invisible (render mode 3) over
   an image is treated as someone else's OCR, and each value is compared with our OCR of the same line, using
   letters and digits only. The resulting `ocr_confidence`:

   | outcome | ocr_confidence |
   |---|---|
   | agree | the OCR line confidence, capped at 0.95 |
   | differ | 0.40 |
   | no OCR available | 0.70 |

   Differing and unverified values are below the 0.80 review floor.
7. **Subject confidence.** An OCR-read part/drawing/heat number's confidence (`subject_conf_bps`) bounds the
   calibration of every value about it. Below 0.80 it forces review. A document cannot set this key.
8. **CSV/XLSX preamble.** `label,value,label,value` rows, also with empty cells, are read.
9. **Calibration `cal@2`.**
   - Groups are also split by layout (`/geom` for strips, title blocks and horizontal tables; `location.layout`
     is set by the worker).
   - The table was refitted on train + dev + dev_scan, and dev now contains the sandwich documents' wrong values.

Versions (ADR 0024 addendum):

| component | version |
|---|---|
| worker | `pdf@3`, `ocr-tesseract@3`, `csv@3`, `xlsx@3` (0.4.0) |
| mapper | `mfg-map@6` |
| calibration | `cal@2` |

New location keys: `layout` and `subject_conf_bps`.

## Calibration and review gate

The gate fitted to 0 bps: the fixed flags alone catch ≥ 90 % of the dev wrong values, so the threshold adds
nothing. The fixed flags are OCR or subject confidence below 0.80 and guessed labels or subjects.

| split | wrong values caught | review load |
|---|---|---|
| dev | 50/50 | 49 % |
| sandwich docs only | 49/49 (no right value flagged) | 80 % (49 of 61) |
| dev_scan | 4/5 | 12 % |
| **holdout** | **12/52 (23 %)** | 38 % |
| **holdout_scan** | **8/8** | 100 % |

Wrong hidden-layer values are caught because they are checked against the page image, not because of the fitted
table. The text holdout's wrong values are still mostly rule-labelled values about the wrong subject. Their
calibration group (`text/rule`, p = 0.9986) has no comparable errors in the fit set. The `nearmiss` dev traps
produced no errors after tuning, so they add no signal either. v2 caught 10/48 (21 %).

## Not done / next

- **Holdout tolerance and measured values (0/51 and 0/98).** The holdout's tables differ from the layouts in
  `ja_dev3.py` in ways this report cannot diagnose without opening the holdout, which was not done. The next step
  needs a new, independently written dev set of measurement tables (another author, another generator). Holdout
  documents cannot be used for this.
- **Wrong-subject values on the text holdout.** They are still not caught (23 %). A per-document consistency
  check (two identifier-shaped values next to subject labels) could flag them. It is untested here.
- **Sandwich checks run OCR on every hidden-layer page.** That costs one OCR pass per page. In the bwrap sandbox it
  uses the same tools as the scanned-PDF path; the worker isolation tests pass.

## Checks

`scripts/check.sh` was run with its own test database (`KIZUNA_ENV_FILE=.env.jav3` → `kizuna_test_jav3`).
Everything passes except `kizuna-api` `bughunt2_regressions::expiry_uses_injected_clock_and_tenant_calendar`. That
test fails the same way on main 3c3df2a when run at 00:xx UTC: the clock is moved forward by hours, so the TOTP code
is rejected. It depends on the time of day and has nothing to do with this branch. The rest of the workspace passes
with it skipped. The Python suite passes (132), including the bwrap+seccomp isolation tests.

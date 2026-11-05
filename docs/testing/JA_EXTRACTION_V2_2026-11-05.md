# Japanese extraction v2: generalisation, measured honestly (2026-11-05)

Branch `ja-v2` (from main c215db0). Follows `JA_EXTRACTION_2026-11-05.md`. Everything here is SIMULATED: synthetic
documents with ground truth. No real supplier documents were used.

## Splits

| split | source | role |
|---|---|---|
| train | `tests/ja_corpus.py` (30 docs) | the conventions the rules were written against |
| dev | the old corpus "holdout" (10 docs) | **tuning**. It was renamed because v2 tuned on it |
| dev_scan | the old scan set (8 scans, 200 dpi) | OCR tuning |
| holdout | `tests/ja_holdout.py` (40 text PDFs/CSVs) | **report only** |
| holdout_scan | `tests/ja_holdout.py` (12 scans, 150–300 dpi) | **report only** |

`tests/ja_holdout.py` was written by a separate agent. That agent was not allowed to read the extraction rules
(`src/`, `crates/`), the evaluator or the old corpus's vocabularies. It chose its own labels, for example 品番(Part No.),
P/N, 製番, Q'ty, 材質記号, Commodity, 承認区分, DWG.NO., APPD and 適合. It also chose its own layouts: labels above values,
values two cells to the right, title blocks with the value before the label, horizontal measurement tables and
numbered spec sections. It added distractors (TEL, 御中, 規格範囲 ranges, AQL, C0.5), its own scan renderer
(Noto Serif/Mono, 150–300 dpi, ±2°, impulse noise, blur, square seals) and FAX headers.

The holdout was committed (f9dfdcd) before any result on it was computed, and nothing was changed after seeing
those results. One convention was mapped in the scorer before any holdout run: the holdout writes the tolerance
minus as a signed number, while this corpus uses its magnitude.

The scorer is now stricter: a Rust claim about the wrong subject counts as wrong. Reproduce with:

    uv run python tests/ja_eval.py --holdout --rust target/debug/examples/map_records --out r.json
    uv run python tests/ja_calibrate.py r.json      # calibration fit + catch rate / review load

## Results (strict, micro precision / recall, Rust production path)

| split | main c215db0 (mfg-map@3) | ja-v2 (mfg-map@4 on its branch, @5 on main) |
|---|---|---|
| train | 1.000 / 0.959 | 1.000 / 0.959 |
| dev | 0.000 / 0.000 | 0.995 / 0.942 |
| dev_scan | 0.748 / 0.445 | 0.955 / 0.530 |
| **holdout** | 0.603 / 0.057 | **0.709 / 0.174** |
| **holdout_scan** | 0.400 / 0.026 | **0.619 / 0.057** |

The main c215db0 column was scored with today's scorer and holdout, using c215db0's worker and mapper. On scans,
v2 trades worker recall for precision:

| worker claims | main c215db0 | ja-v2 |
|---|---|---|
| holdout | 0.423 / 0.144 | 0.533 / 0.220 |
| holdout_scan | 0.471 / 0.134 | 0.413 / 0.079 |

Binarisation is the likely cause of the drop on holdout_scan: on dev_scan it lowered worker recall as the price of
precision. This is not measured on the holdout, because no A/B variants are run on it.

The headline is that **the rules do not generalise to independently written documents**. Recall on the holdout is
0.17, and 0.06 on its scans. The dev numbers (0.94) measure tuning, not generalisation.

Holdout per-field recall:

| fields | recall |
|---|---|
| unit_price | 0.76 |
| quantity | 0.59 |
| part_name, supplier | 0.36 |
| delivery_date | 0.31 |
| issue_date | 0.20 |
| drawing_number, revision, inspection_date, drawn_by, approved_by | 0.12 |
| material | 0.08 |
| result | 0.06 |
| tolerance | 0.00 |
| measured.* | 0.00 |

Full per-field tables are in `reports/ja/ja_v2_eval.json`.

Holdout precision fell mainly for three reasons:

- **Wrong subjects (claims attached to the wrong part).** A title block with a known label (REV.) next to an
  identifier that is not a part number.
- **`standard` read from spec-range text.**
- **material = a grade family.**

The remaining recall gaps are structural:

- horizontal measurement tables are not read, so there are no tolerance or measured values;
- label-above-value layouts are not read;
- values printed before their labels are not read.

## What changed

1. **Fallback mapping (Rust `infer.rs`).**
   - **Unknown labels.** When the vocabulary does not know a label, cues in the label (Japanese substrings; Latin
     whole words) and the value's shape map it. Examples: 製品番号 has 品 + 番号 and maps to part number;
     a date value under a label ending in 日 maps to a date; `1:2` maps to scale; `JIS G …` maps to standard;
     `板厚 規格値` maps to the nominal thickness.
   - **Subject without a recognised label.** When no subject label is recognised, one identifier-shaped header
     value is chosen. Scoring uses its shape, its position (header, top lines, first column) and how close it is to
     recognised fields. The value must be the unique best and score at least 4.
   - **Lower confidence.** Every guessed value, and every claim about a guessed subject, carries
     `label_basis`/`subject_basis` = `fallback`, has confidence capped at 0.60 and is `needs_confirmation`.
   - **Worker.** Title-block grids and CSV headers now accept label-like words (`claims.label_like`), so unknown
     labels reach the mapper as records.
2. **Synonym learning (migration 0025 on main `label_synonyms`, `kizuna-api/src/label_synonyms.rs`).**
   - **Learning.** When a reviewer approves a fallback-mapped worker claim, the tenant learns `label → mapped_as`,
     and `subject_label → subject_as` if the subject was guessed. The `label_synonym.learn` event is audited.
   - **Next extraction.** The next extraction for that tenant maps the label with basis `tenant_synonym`.
   - **Scoping.** Synonyms are tenant-scoped by RLS and are never shared across tenants. A synonym never
     overrides a rule label, and targets come from a fixed list. Hand-written (`manual`) and AI claims cannot plant
     a synonym, and a document cannot pre-set the provenance keys.
   - **Reversible.** `POST /label-synonyms/{id}/revoke` keeps the row and records who revoked it and when, and the
     revocation is audited. `GET /label-synonyms` lists synonyms.
   - **Test.** `tests/label_synonyms.rs` covers the whole cycle end to end, including the other tenant.
3. **OCR preparation.**
   - **Rasterisation.** Scanned PDFs are rendered at 300 dpi when the page is A4 or Letter sized. Larger pages are
     rendered to about the pixel size of an A4 page at 300 dpi, never below 200 dpi. Without that cap, the
     existing API test (a 72-dpi image wrapped as a 19-inch page) was upsampled 4× and read SHF-77 as "SHF-/77".
     Image scans are upsampled to about 300 dpi for an A4 page.
   - **Masks.** Detected 印影 regions are painted white before OCR.
   - **Grey, deskew, binarise.** ImageMagick 7 converts the page to grey, deskews it (rotating about the centre)
     and binarises it with Otsu.
   - **Fixed input coder.** The input coder is always `png:`/`jpeg:`, so ImageMagick never auto-detects a format.
   - **Coordinates.** Word boxes are returned in the 200-dpi base space that the seal detector uses.
   - **No ImageMagick.** Without ImageMagick the worker warns `ocr_preprocess_unavailable` and uses the raw raster.
   - **Licence.** ImageMagick is a system tool under the ImageMagick License (Apache-2.0-style), invoked as a
     program and not linked. No Python dependency was added.
   - **Sandbox.** In the bwrap sandbox `/usr/bin/magick` is a symlink into the unmounted `/etc/alternatives`, so
     the worker looks for `magick-im7.q16` first.

   Measured on dev_scan with Rust P/R (same mapper, one variant changed at a time):

   | variant | P / R |
   |---|---|
   | 200 dpi raw (old) | 0.659 / 0.445 |
   | 300 dpi raw | 0.636 / 0.420 |
   | no binarise | 0.705 / 0.515 |
   | no deskew | 0.861 / 0.525 |
   | no seal mask | 0.862 / 0.500 |
   | **all (chosen)** | **0.943 / 0.500** |

   The size-capped dpi rule only changes non-A4 pages. Final run on dev_scan: 0.955 / 0.530.

   On holdout_scan the result is poor (0.619 / 0.057). The main loss is not OCR quality but the layouts the rules
   cannot read (above).
4. **Confidence calibration (`calibration.rs` + `calibration.json`, `cal@1`).**
   - **Groups.** Each value falls in one group: text or OCR, crossed with rule, tenant synonym or fallback.
   - **Fit.** Within each group, isotonic regression (PAV) on the OCR confidence, fitted on train + dev + dev_scan.
     Blocks hold at least 10 values. Their estimates are smoothed to (k+1)/(n+2) and re-pooled so they stay
     monotone. The fit is deterministic and
     the table is committed.
   - **Claims.** Each claim carries `p_correct_bps`. Its confidence is at most that value.
   - **Gate.** Values with p < `gate_bps` need confirmation, as do values below the old OCR floor of 0.80 and all
     guessed values. The gate was chosen as the smallest threshold that catches at least 90 % of wrong values on dev
     + dev_scan, which gave 0.9232.

   Results:

   | split | wrong values caught | review load (share of values flagged) |
   |---|---|---|
   | dev | 1/1 | 100 % (all fallback) |
   | dev_scan | 5/5 | 20 % |
   | **holdout** | **10/48 (21 %)** | 38 % |
   | **holdout_scan** | **6/8 (75 %)** | 71 % |

   The gate does not transfer to the text holdout. Most of its wrong values are rule-label values about the wrong
   subject. The fit set has no wrong text/rule values, so the calibration rates that group at 0.998. On holdout
   scans the gate catches 75 % (target 90 %) and flags 71 % of values. Tesseract's word confidence separates
   right from wrong only weakly, and the fit set has just 6 wrong values.

## Not done / next

- Readers for horizontal measurement tables, label-above-value layouts and value-before-label title blocks (the
  holdout's largest recall gaps).
- A subject-consistency check: a title-block identifier next to a known part number should not become the
  subject.
- Recalibrate once real reviewed documents exist. A synthetic fit set has too few wrong values (6) to calibrate
  well.
- Run the ImageMagick step with the worker's seccomp filter on the pilot host. With the worker's bwrap mount layout
  alone (no /etc/ImageMagick-7), `magick-im7.q16` was checked to run and produce a PNG. The corpus measurements
  were taken outside the sandbox.

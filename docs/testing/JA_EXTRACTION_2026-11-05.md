# Japanese document extraction — measured before/after (2026-11-05)

Branch `ja-extraction` (from main c9d58a4). Deterministic rules only, no model, no network.

## Corpus

`services/extraction/tests/ja_corpus.py` is a seeded generator (seed 20261105) with ground truth. It produced 48 documents
and 1,193 labelled values:

| split | docs | what |
|---|---|---|
| train | 30 | 検査成績書, ミルシート, 仕様書 and 図面 as CJK text PDFs (Type0/Identity-H + ToUnicode; every 3rd one has one fragment per glyph, like Excel exports), plus 注文書 as Shift_JIS CSV with a preamble |
| holdout | 10 | same layouts, but with label synonyms the rules were **not** written for (製品番号, 図面No., 使用材料, 合否, 長さ, 板厚…) |
| scan | 8 | 4 types rendered by ImageMagick at 200 dpi with noise, skew and red 印影 seals. Half are PNG; half are JPEG-in-PDF with a FAX transmission header |

The documents include full-width digits, 和暦 (令和/平成, 元年, R7.4.1), ㎜ / N/mm² / MPa / ％ / 個 / ￥, ± and +a/−b
tolerances, inline `120.5㎜ ±0.1`, item/role inspection tables, two-row mill-sheet headers, tables that continue on
page 2 without a repeated header, title-block key-value grids, and 設計/検図/承認 approval boxes.

The evaluator is `tests/ja_eval.py`:

- **strict**: the value must match in canonical form: ISO date, number plus canonical unit, plus/minus, pass/fail enum, or NFKC text.
- **detect**: the printed text is also accepted.

It scores two pipelines:

- **worker**: the claims the worker itself produces.
- **rust**: the production path, worker records → `map_records` (run with `cargo run --example map_records`).

Reproduce with: `uv run python tests/ja_eval.py --rust target/debug/examples/map_records --out r.json`

## Results (strict, micro precision / recall)

| pipeline / split | baseline c9d58a4 | after |
|---|---|---|
| worker / train | 0.303 / 0.027 | **1.000 / 1.000** |
| worker / holdout | 0.000 / 0.000 | 1.000 / 0.539 |
| worker / scan (OCR) | 0.050 / 0.005 | 0.620 / 0.505 |
| worker / all | 0.219 / 0.018 | 0.938 / 0.824 |
| rust / train | 0.619 / 0.019 | **1.000 / 0.959** |
| rust / scan | 0.000 / 0.000 | 0.630 / 0.375 |
| rust / holdout | 0.000 / 0.000 | 0.000 / 0.000 (see limits) |
| rust / all | 0.619 / 0.012 | 0.942 / 0.666 |

At the detect level, worker/all went from 0.771 / 0.062 to 0.975 / 0.857.

The train figures measure coverage of conventions the rules were written against. The holdout and scan rows are the honest
generalisation numbers. Worker time for the whole corpus was 70 s, almost all of it OCR. Each scanned JPEG-PDF takes about
8–17 s because of the 200 dpi rasterisation.

## Japanese evidence fidelity (I5)

| item | scan-set result |
|---|---|
| FAX header: sender / number / sent_at | 4/4 each, ISO timestamp |
| 印影 regions (pixel detection, assigned to 設計/検図/承認) | 5 of 6 seals assigned correctly. 1 is `unassigned` because OCR read 検図 as "RY". 0 spurious |
| approval date from the 承認 box | 2/2 |
| OCR values below 0.80 confidence flagged `needs_confirmation` | 23 of 149 claims |
| …of which wrong | the flag caught **7 of 60** wrong OCR values. Tesseract's confidence is poorly calibrated, so the flag is a floor, not a safety net |

The rules behind these results:

- Claims keep the printed text in `value` and add the normalised form next to it (`norm`, `date`, `number`/`unit`, `plus`/`minus`). In Rust, `source_location.original` holds the 全角/和暦 text.
- A seal is the claim `mfg.approval.mark.<role>` = present. It is never "approved".
- `capture.fax.*` claims record the FAX header.
- A claim whose `source_location.review` is `needs_confirmation` cannot back a decision until a person accepts it. The API returns `claim_needs_confirmation`, and the trigger in migration `0017_ja_evidence.sql` enforces the same rule for any writer. This is tested in `crates/kizuna-api/tests/ja_evidence.rs`, and a property test checks that every claim below the OCR floor is flagged.

## Rules added

Worker:

- `ja.py`: NFKC plus look-alike folding, 和暦 with era bounds, units, thousands separators, tolerances, inline tolerances, result and approval words, FAX header.
- Layout: CJK glyph widths, per-glyph joining, CJK OCR word rejoining, OCR adjacency from pixel boxes.
- Records:
  - leader-dot and 【】 key-value forms;
  - title-block key-value grids;
  - two-row headers with super-header units;
  - item/role inspection tables (規格値/公差/測定値/判定);
  - tables that continue on the next page;
  - the `n=1` header fix.
- CSV: header detection and preamble records.
- Approval boxes: a name gives a stamp/name claim, 承認 with a date gives the approval date, and a status word gives the approval status.
- Element symbols are read as chemistry only when the row has at least 3 of them.
- A row's 単位 column applies to its quantity.
- `stamps.py`: red-ink seal detection. PNG/JPEG data is wrapped unchanged in a PDF and pdftoppm renders it to PPM.
- JA instruction-like text is flagged.

Rust (`mfg-map@3`): mirrors the vocabulary and adds:

- `ja.rs`;
- part inheritance for groups without one, when the document has a single subject;
- heat number as the subject of mill sheets;
- nominal dimensions as `mfg.dimension` in every kind;
- source locations rounded to fit the claim location contract. Before this change, float PDF bboxes failed `validate`, so positioned PDF claims were dropped;
- new `LOCATION_KEYS`.

## Known limits

- **Rust holdout is 0.** When the part-number label is unknown (製品番号), the document has no subject, so the mapper emits nothing. That is safe, but recall is zero.
- **Multi-heat mill sheets.** Document-level fields (鋼種, 規格, 製造者) are not attached to any heat.
- **OCR at 200 dpi.**
  - Small kanji (図面番号→"HMMS", 外径→"ME") and ± (read as +, and refused as one-sided) are misread.
  - `N/mm²` comes out as "N/mm?".
  - Names inside red seals are not read, so `approved_by` and the other sign-off names are missed on scans; only the mark is recorded.
  - The PDF `/Title` written by the scanner software becomes a `title` claim (4 false positives).
- **Seal detection formats.** Only 8-bit non-interlaced RGB/grey/palette PNG, 3-component JPEG and scanned PDFs are supported. RGBA PNG gets the warning `stamp_detection_skipped:image_format`.
- **Units.** `φ25` with no unit stays unitless; there is no implicit mm. A plain `単価` without ￥/円 has no currency.

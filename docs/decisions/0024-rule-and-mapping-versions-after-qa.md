# ADR 0024: Rule and mapping version bumps for the QA acceptance fixes

Status: accepted (2026-11-05).

## Context
The QA acceptance lane (`qa`, report `docs/testing/ACCEPTANCE_2026-11-05.md`) extended the conflict rules and
label vocabularies without changing their version tags. Every claim and finding records the version that produced
it (`extraction_method`, `provenance`, finding explanations), so a behaviour change under an unchanged tag would make
two runs with the same tag disagree.

## Decision
- `mfg-rules@2`: adds `certified_material` (a mill sheet / inspection certificate certifying another grade than the
  specification, e.g. SUS316 vs SUS304 → `value_mismatch`). Test `regression_qa_mill_sheet_material_differs_from_specification`.
- `mfg-map@4`: Japanese tokens 長さ, 板厚 and `<dimension>公差` (長さ公差, 外径公差, …) map like their English labels;
  the same table is mirrored in the worker (`claims.py JA_TOKENS`). Test
  `regression_qa_japanese_dimension_property_and_tolerance_labels`.
- `ai-map@3`: the plausibility lexicon accepts 製造者 / 製造元 for `supplier`.
- Re-extraction under a new mapping version is a new run (`extraction_runs` are unique per mapping version); existing
  claims keep their recorded version.

## Consequences
Version tags change whenever behaviour changes; reviews should reject rule/vocabulary edits without a bump.

## Addendum: `mfg-map@5` (ja-v2)
The ja-v2 lane also used `mfg-map@4` on its branch for its fallback label inference (`infer.rs`) and tenant label
synonyms (migration 0025 on main). Merged after the QA bump, it becomes `mfg-map@5`.

## Addendum: `mfg-map@6`, `cal@2` and worker extractors `@3` (ja-v3, 2026-11-06)
Report: `docs/testing/JA_EXTRACTION_V3_2026-11-06.md`.
- Worker `pdf@3`, `ocr-tesseract@3` (worker 0.4.0). New layout readers (`geometry.py`) read horizontal
  measurement tables, label/value strips (label above or below its value) and, in title blocks, any vocabulary label
  with its value to the right, below or above. Ruling lines split cells. Pages whose only text is invisible over an
  image (a scanner's hidden OCR layer) are checked against our own OCR of the page. Each value gets an
  `ocr_confidence`, below the review floor when the two readings differ. Records carry `location.layout`.
- Worker `csv@3`, `xlsx@3`: preamble rows with several `label,value` pairs are read.
- `mfg-map@6`: part sections. When the `label: value` fields name two or more different parts, each positioned value
  is about the nearest part header above it. Header fields before the first part are about every part. Each claim
  records `section`. An OCR-read subject's confidence (`subject_conf_bps`) bounds its values' calibration and flags
  them below the review floor.
- `cal@2`: groups also split by layout (`/geom` for strips, title blocks and horizontal tables). The fit set
  includes dev documents with deliberate wrong values in a hidden text layer.
- Location keys `subject_conf_bps` (derived; documents cannot set it) and `layout` (set by the worker) were added to
  `evidence_core::claim::LOCATION_KEYS`.

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

# Integrity signals and revision drift: precision / recall (synthetic)

Command: `cargo run --release -p integrity-signals --example evaluate 8` (2026-11-05, this host: 8 vCPU, rustc 1.95.0,
5.1 s wall incl. build check). Default settings. Generator: `crates/integrity-signals/src/synth.rs` (seeds 1–8).

Certificates: per seed 24 honest suppliers + 3 per injected kind, 40 certificates each (80 for the digit-preference
supplier), 17,496 certificates in total, 7,780 on honest-only suppliers. Honest data includes re-issued certificates
for the same lot, very capable processes (σ = tol/10), coarse integer hardness, old stock shipped a week late.

| Signal | Injected | Detected | Signals | True pos. | Precision | Recall |
|---|---|---|---|---|---|---|
| table_copy | 24 | 24 | 24 | 24 | 1.000 | 1.000 |
| identical_values | 24 | 24 | 26 | 24 | 0.923 | 1.000 |
| narrow_band | 24 | 24 | 24 | 24 | 1.000 | 1.000 |
| at_limit | 24 | 24 | 24 | 24 | 1.000 | 1.000 |
| excess_precision | 24 | 24 | 24 | 24 | 1.000 | 1.000 |
| chemistry_sum | 24 | 23 | 23 | 23 | 1.000 | 0.958 |
| date_order | 24 | 24 | 24 | 24 | 1.000 | 1.000 |
| lot_sequence | 24 | 24 | 24 | 24 | 1.000 | 1.000 |
| digit_preference | 24 | 23 | 23 | 23 | 1.000 | 0.958 |

False signals on honest-only suppliers: 2 in 7,780 certificates (0.26 per 1,000), both `identical_values`
coincidences on coarse values. Misses: one injected lost decimal point gave Mn 95 % (sum < 100 %, physically possible,
correctly not flagged); one digit-preference case above the p threshold.

Drawings: per seed 60 drawings × 8 events (honest revisions with changes, re-scans, single-field OCR gaps,
"120.0" vs "120.00"; injected silent changes, revision bumps without change, regressions), 3,327 versions.

| Finding | Injected | Detected | Findings | True pos. | Precision | Recall |
|---|---|---|---|---|---|---|
| silent_change | 600 | 600 | 600 | 600 | 1.000 | 1.000 |
| revision_without_change | 419 | 419 | 420 | 419 | 0.998 | 1.000 |
| revision_regression | 163 | 163 | 163 | 163 | 1.000 | 1.000 |

The one false `revision_without_change`: an honest revision changed a field that OCR had missed in the copy before.

Acceptance bar (tests/evaluation.rs, seeds 1–3): per-kind precision and recall ≥ 0.6, overall ≥ 0.9, honest false
signals ≤ 2 per 1,000; drift per kind ≥ 0.9. **Synthetic only**: an upper bound for real documents.

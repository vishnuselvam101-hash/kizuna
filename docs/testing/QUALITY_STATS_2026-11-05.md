# quality-stats calibration and benchmark — 2026-11-05

Evidence level: **SIMULATED** (deterministic synthetic data, no real certificates). Crate `crates/quality-stats`
(ADR 0019). Reproduce exactly:

```
cargo run --release -p quality-stats --example calibrate   # seed 20261105, 1000 honest + 1000 manipulated per row
cargo run --release -p quality-stats --example bench       # 100 000 values, median of 7 runs
```

Machine: AMD EPYC 9354P (8 vCPU), Rust 1.95, release profile, single-threaded.

## Method
- Each row generates 1000 honest and 1000 manipulated datasets with the SplitMix64 generator (`synth.rs`), renders
  values to decimal strings and parses them back exactly (same path as real input).
- Score = −log10(p). FPR = share of honest datasets with p < 0.01; TPR = share of manipulated datasets with
  p < 0.01; ROC AUC = Mann–Whitney P(score_manip > score_honest) + ½ P(tie). Datasets where the test refused
  (`NotApplicable`) are counted in the n/a columns and excluded from the rates.
- Monte Carlo standard error of a rate near 0.01 with 1000 datasets is ≈ 0.003; of a rate near 0.5 ≈ 0.016.

## Integrity tests (real run output)
| Test | Manipulation | n/a honest | n/a manip. | FPR @ p<0.01 | TPR @ p<0.01 | ROC AUC |
|---|---|---|---|---|---|---|
| terminal digit χ² | 15% of last digits tidied to 0/5 (n=60, sd=80 units) | 0 | 0 | 0.014 | 0.157 | 0.744 |
| heaping index | 15% of last digits tidied to 0/5 (n=60, sd=80 units) | 0 | 0 | 0.004 | 0.340 | 0.934 |
| terminal digit χ² | 30% of last digits tidied to 0/5 (n=60, sd=80 units) | 0 | 0 | 0.013 | 0.746 | 0.974 |
| heaping index | 30% of last digits tidied to 0/5 (n=60, sd=80 units) | 0 | 0 | 0.008 | 0.932 | 0.998 |
| Benford χ² | invented: uniform first digit over 4 decades (n=200) | 33 | 0 | 0.003 | 1.000 | 1.000 |
| Benford guard | controlled process N(520,10) honest vs same (both must be n/a) | 1000 | 1000 | 0.000 | 0.000 | n/a |
| variance too low | repeats with sd = 0.5·σ0 (n=10) | 0 | 0 | 0.015 | 0.492 | 0.973 |
| variance too low | repeats with sd = 0.7·σ0 (n=10) | 0 | 0 | 0.008 | 0.118 | 0.854 |
| exact at limit | values > USL clamped to USL (n=50, N(9.5,0.4), USL 10.00) | 0 | 0 | 0.005 | 0.932 | 0.996 |

Reading:
- **Terminal digit / heaping**: heaping on 0/5 is the more powerful of the two for the "tidying" pattern it targets
  (AUC 0.93 vs 0.74 at 15% tidied, n = 60); χ² over all 10 digits catches other digit preferences. Honest FPR
  0.013–0.014 for χ² is slightly above nominal (χ² approximation with 6 expected per cell; within ~1.3 SE).
- **Benford** detects invented values with uniform first digits perfectly here, and the guard refused **all** 1000
  controlled-process datasets (520 ± 10 MPa) — exactly the false alarm it exists to prevent. 33/1000 honest lognormal
  datasets were refused because their central 90% spanned < 2 decades.
- **Variance too low** with n = 10 repeats has limited power: 49% at half the stated σ, 12% at 0.7σ (AUC 0.97 /
  0.85). Honest FPR 0.015 (≈ 1.6 SE above 0.01). Ask for more repeats before relying on it.
- **Exact at limit** finds clamping of out-of-spec values onto USL in 93% of 50-value datasets (AUC 0.996) with
  FPR 0.005.

## Near-duplicate tables (MinHash k = 128 + LSH)
200 independent tables (20 rows × 5 columns, N(50, 2) at 0.01) plus an edited copy of each of the first 100
(edited cells shifted by 0.01–0.05). 44 750 non-copy pairs exist among the 300 tables.

| Cells edited in copy | mean J est. (copies) | max J est. (independent) | LSH b×r, J cut | copies found | other pairs flagged | ROC AUC |
|---|---|---|---|---|---|---|
| 10% | 0.740 | 0.094 | 32×4, 0.5 | 100/100 | 0 of 44750 | 1.000 |
| 30% | 0.445 | 0.078 | 32×4, 0.5 | 16/100 | 0 of 44750 | 1.000 |
| 30% | 0.445 | 0.078 | 64×2, 0.2 | 100/100 | 0 of 44750 | 1.000 |
| 50% | 0.277 | 0.086 | 64×2, 0.2 | 91/100 | 0 of 44750 | 1.000 |

Reading: estimated J of independent tables never exceeded 0.094, copies stay ≥ 0.28 on average even with half the
cells edited, so ranking is perfect (AUC 1.000) in this setting. The default 32×4 banding with J ≥ 0.5 is tuned for
light editing (≤ 10%); 64×2 with J ≥ 0.2 recovers 100% of 30%-edited and 91% of 50%-edited copies with no false
pairs here. Real certificates share fixed columns (nominal, limits, units) — exclude those columns from the tokens
or expect higher J between unrelated tables.

## SPC run rules on in-control data
- WesternElectric: 0.215 of 1000 in-control 25-point series raise ≥ 1 signal
- Nelson: 0.241 of 1000 in-control 25-point series raise ≥ 1 signal

Roughly consistent with the theoretical in-control ARL of about 92 points for the four Western Electric rules combined
(1 − (1 − 1/92)^25 ≈ 0.24): rule hits on short series are context, not findings.

## Unit-test reference values
Textbook examples asserted in tests: Montgomery Table 9.1 CUSUM (C⁺₂₉ = 5.28, first signal at 29) and EWMA
(λ = 0.1, L = 2.7: z₁ = 9.945, UCL₁ = 10.27, first signal at 29); control-chart factors for n = 2, 5, 10 (A2, D3,
D4, A3, B3, B4, c4); d2 table re-derived by numerical integration for n = 2..25; normal/χ² values from standard
tables; FNV-1a test vector; Bissell and χ² capability intervals hand-computed. 53 tests incl. 13 proptest
properties (`cargo test -p quality-stats`).

## Benchmark (100 000 values, release, median of 7)
| Operation on 100000 values | median ms |
|---|---|
| parse decimal strings | 9.13 |
| terminal digit χ² | 8.66 |
| heaping index | 5.92 |
| Benford (guard refuses) | 19.92 |
| variance too low | 0.66 |
| exact at limit (median/MAD) | 37.68 |
| capability (I-MR sigma, CIs) | 1.22 |
| X̄-R chart (20 000 subgroups of 5) | 0.86 |
| I-MR chart | 0.30 |
| EWMA | 3.45 |
| CUSUM | 0.53 |
| Nelson rules | 2.97 |
| MinHash k=128 + LSH (1000 tables × 100 values) | 123.27 |

All single-pass statistics run in ≤ 10 ms per 100k values; median/MAD-based steps (exact-at-limit) sort twice
(~38 ms). MinHash cost is k × tokens (1000 tables × 120 tokens × 128 hashes ≈ 15 M mixes, 123 ms).

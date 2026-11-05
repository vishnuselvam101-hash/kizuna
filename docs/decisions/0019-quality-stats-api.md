# ADR 0019: `quality-stats` — pure SPC, capability and data-integrity statistics

Status: accepted (2026-11-05). Not yet wired into the API; integrator/feature lanes will call it.

## Decision
A new library crate `crates/quality-stats` (no I/O, no DB, no async, no global state, `unsafe` forbidden) holds all
statistics over measured values. Every public function is a pure function of its inputs; results are deterministic
across machines (MinHash uses a fixed seed and portable hashes, no `RandomState`).

### Input and numerics
- Values enter as text and are parsed exactly by `Measurement::parse` (`rust_decimal`, which keeps the written scale:
  `"12.30"` has scale 2 and terminal digit 0). Plain decimals only — exponents, separators, `NaN`/`inf` are refused;
  locale normalisation happens upstream.
- Anything depending on written digits (terminal digit, heaping, Benford first digit, equality with a limit, table
  tokens) uses the exact decimal. `f64` is used only inside statistics; accuracy of each special function and of
  Welford mean/variance is documented in `special.rs` / `describe.rs` and tested against published values.
- Errors are `StatsError` (`TooFewValues`, `ZeroVariance`, `NonFinite`, `InvalidParameter`, `Parse`); no panics on
  user data. A test whose assumptions fail returns `Applicability::NotApplicable(reason)`, never a misleading number.

### API (module → functions)
| Module | Functions | Output |
|---|---|---|
| `spc` | `xbar_r`, `xbar_s`, `individuals_moving_range` | `SubgroupChart` / `IndividualsChart` (points, `ChartLimits`, σ̂ within) |
| `spc` | `ewma(xs, target, σ, λ, L)`, `cusum(xs, target, σ, k, h)` | statistic series, limits, signal indices |
| `spc` | `check_rules(points, centre, σ, RuleSet::{WesternElectric, Nelson})` | sorted `Violation { index, rule }` |
| `capability` | `capability(values, Spec { lsl, usl }, σ_within?, confidence)` | Cp/Cpk/Pp/Ppk (+ Cpu/Cpl) with CIs and caveat notes; one-sided specs → Cp/Pp `None` |
| `integrity` | `terminal_digit_preference`, `rounding_heaping`, `benford_first_digit` (`&[Measurement]`) | `Applicability<TestResult>` |
| `integrity` | `variance_too_low(values, stated σ, resolution)` | `StatsResult<Applicability<TestResult>>` |
| `integrity` | `exact_at_limit(values, limit, LimitSide, resolution?)` | `Applicability<TestResult>` |
| `integrity::minhash` | `table_tokens`, `MinHasher::new(k, seed).signature`, `find_near_duplicates(sigs, bands, rows, J cut)` | `Signature`, `NearDuplicate { a, b, jaccard_estimate }` |

`TestResult { statistic, df, p_value, n, explanation }` — the explanation is reviewer-facing text (counts, expected
values, p). Integrators store the whole result next to the finding.

### Usage rules for integrators
- A p-value is a prompt for review, never an automatic rejection. With many certificates, control the false
  discovery rate (e.g. Benjamini–Hochberg) before raising findings; the calibration report shows realistic power.
- Run rules on 25 in-control points already raise ≥ 1 signal in ~22–24% of series (calibration run); show rule
  hits as context, not as findings.
- Signatures are stable only for a fixed `(k, seed)`; store them with those parameters (default `k = 128`,
  `DEFAULT_SEED`). For copies with ≤ 10% edited cells use 32×4 bands, J ≥ 0.5; to catch heavier editing use 64×2,
  J ≥ 0.2 (see `docs/testing/QUALITY_STATS_2026-11-05.md`).

## Consequences
Deterministic, explainable statistics that can be unit-tested against textbook examples. Normality is assumed by
capability indices, the variance test and the limit model; the module docs state each assumption and guard. Power
figures are SIMULATED on synthetic data only; precision on real certificates is unmeasured (research register).

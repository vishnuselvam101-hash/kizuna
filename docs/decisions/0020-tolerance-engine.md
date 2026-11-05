# ADR 0020: `tolerance` — dimensional tolerance engine (spec model, ISO 286, ISO 14253-1, stack-up)

> **Verification required before any customer use:** the ISO 286 tolerance/deviation tables and the ISO 965 thread
> (6g/6h) values in `crates/tolerance` were typed from memory, not copied from the standards. Every table value must
> be checked against the purchased ISO 286-1/-2 and ISO 965 texts (and the check recorded in
> `docs/testing/TOLERANCE_2026-11-05.md`) before results are relied on.

Status: accepted (2026-11-05). Not yet wired into the API; integrator lanes will call it. (Numbered 0019 because 0018
is already used twice on other branches: `ai-assisted-extraction` and `quality-stats-api`.)

## Context
Verification compares numbers on supplier certificates and inspection reports with drawing tolerances written in
Japanese or English (`φ10 +0.1/−0`, `10±0.05`, `25 h7`, `M8×1.25 6g`, `400MPa以上`, full-width digits). The answer
must be exact, must account for measurement uncertainty, and must be explainable to a quality engineer. Assembly
reviews also need a 1-D stack-up with supplier capability.

## Decision
A new library crate `crates/tolerance` (pure: no I/O, DB, async or global state; `unsafe` forbidden; deps only
`rust_decimal` + `thiserror`, both already in the workspace).

- **Exact numbers.** Every dimensional value is a `rust_decimal::Decimal` held in the canonical unit of its
  dimension (mm for length, MPa = N/mm² for stress). Factors into canonical are exact (1 in = 25.4 mm, 1 µm =
  0.001 mm, 1 GPa = 1000 MPa, 1 kPa = 0.001 MPa). A conversion that rust_decimal would round (scale > 28) is an
  error, not a silent rounding, so unit → canonical → unit is identity or `Err`. mm → in has no exact decimal for most
  values; decisions never need it, explanations display it rounded to 9 dp.
- **Spec model.** `Spec { nominal?, lower?: Bound, upper?: Bound, display_unit, fit? }`; a `Bound` can be exclusive
  (未満 / 超 / `<` / `>`). Constructors refuse contradictions (lower > upper, negative ±, mixed dimensions).
- **ISO 286-1:2010.** Tables for IT4–IT13 and fundamental deviations d, e, f, g, h, js, k, m, n, p (holes D–P by the
  general rule EI = −es and the special rule ES = −ei + Δ for K/M/N ≤ IT8 and P ≤ IT7), 0 < D ≤ 500 mm, transcribed
  from the standard's published values (= JIS B 0401-1). js/JS rounds odd IT7–IT11 values down to even as the
  ISO 286-2 tables do. Anything outside the tables (> 500 mm, IT1–3/IT14+, r–zc) is `Unsupported` — never
  extrapolated. A test checks every IT entry against the standard tolerance factor i = 0.45∛D + 0.001D (±25 %) to
  catch transcription errors; 25 published fits are checked exactly.
- **Threads.** ISO 261 coarse pitch, basic pitch diameter (ISO 724), major-diameter limits for 6g/6h external threads
  (ISO 965-1 es and Td(6)). Other classes parse but return `Unsupported` for limits.
- **Parser.** Hand-written cursor parser after a normalisation pass (full-width → ASCII, U+2212/dashes → `-`,
  〜 → `~`, µ/μ → `u`, diameter marks dropped). A unit written only after the tolerance also applies to a unit-less
  nominal (`0.5 ± 0.002 in`). Ambiguous inputs (`10 +0.1` with one deviation, decimal comma `1,5`) are refused.
  Never panics (property-tested on arbitrary Unicode and mutated notations).
- **Conformance (ISO 14253-1:2017).** Measurement = [y − U, y + U] with expanded uncertainty U. Pass iff the
  interval is inside the spec zone (y in the conformance zone, spec shrunk by U); Fail iff the interval is entirely
  outside; otherwise Indeterminate. 2U > T ⇒ the conformance zone is empty and the explanation says to reduce U.
  The verdict carries the conformance-zone limits and an English one-line explanation with the numbers.
- **Stack-up.** Worst case exact in decimal; RSS √Σtᵢ² (Cp = 1 assumption, configurable `default_cp`); modified
  RSS = Bender factor (1.5) × RSS, capped at the worst case with a finding. Supplier data per link: σ, or Cpk with an
  optional mean (σ = min(USL − μ, μ − LSL)/(3·Cpk)). Assembly modelled as a normal (independent links), predicted
  yield and ppm, variance contribution per link, findings `WorstCaseOutside`, `RssOutside`, `MrssCappedAtWorstCase`,
  `YieldBelowTarget{predicted, target, top_contributor}` (default target 0.9973). Statistics are f64; erfc via series
  + continued fraction (A&S 7.1.14), ≤ 1e-11 relative error vs CPython `math.erfc`.

## Consequences
- Decisions on limits are exact and reproducible; only yield/RSS numbers are floating point.
- Coverage is honest but partial (ISO 286 letters d–p, 6g/6h threads). Extending means adding table rows with a
  citation and a published-value test, not code paths.
- Normal approximation underestimates tails for skewed or truncated (sorted) supplier distributions; the yield is a
  prediction, labelled as such, never a verification result.

## Alternatives rejected
- f64 everywhere: `0.1 + 0.2` errors at the limit would flip pass/fail on exact boundary values.
- Monte Carlo stack-up: needs RNG and distributions per link; RSS/normal is the industry baseline and deterministic.
- A parser generator crate: a new dependency for ~300 lines of grammar.

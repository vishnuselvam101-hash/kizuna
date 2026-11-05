# `tolerance` crate — test and benchmark results (2026-11-05)

Branch `algo-tolerance` from main 45d4ef0; rustc 1.95.0; AMD EPYC 9354P (shared host, other jobs running).
Design: ADR 0020. Commands: `cargo test -p tolerance`, `cargo clippy -p tolerance --all-targets`,
`cargo run --release -p tolerance --example bench`. Target dir after debug + release build of this crate: 343 MB.

## Results
- `cargo test -p tolerance`: **30 passed, 0 failed** (24 unit tests, 6 property tests × 2000 cases each).
- `cargo clippy -p tolerance --all-targets`: 0 warnings.

| area | what is checked (expected values are hand-computed or published) |
|---|---|
| units | 1 in = 25.4 mm exactly; 25.4 mm → 1 in / 25 400 µm; 0.21 GPa = 210 N/mm² = 210 000 kPa; mm → in without an exact decimal is refused; length ↔ stress is a dimension error |
| ISO 286 | 25 mm: H7 0/+21, h7 −21/0, g6 −20/−7, f7 −41/−20, k6 +2/+15, m6 +8/+21, n6 +15/+28, p6 +22/+35, K7 −15/+6, K6 −11/+2, M7 −21/0, N7 −28/−7, P7 −35/−14, F8 +20/+53, js7 ±10, D10 +65/+149 µm; 50 mm H7/g6; 10 mm N7 −19/−4; range boundary 18 vs 18.001; D ≤ 3 Δ = 0 (K7 −10/0); 100 mm h9; js6 ±6.5; > 500 mm, IT14, letter x, `Js7` refused; all 117 IT4–IT13 entries within ±25 % of i = 0.45∛D + 0.001D |
| threads | M8×1.25-6g major Ø 7.972/7.760; M10 (coarse 1.5) 6g 9.968/9.732; d₂(M8×1.25) = 7.18810125; 6H limits and M7 coarse pitch → `Unsupported` |
| parser | 8 symmetric forms incl. full-width `１０±０．０５` and `10mm ± 50um`; 6 deviation forms incl. `φ１０　＋０．１／－０`, `⌀10 0/+0.1`; `25 h7`, `φ25H7`, `２５ｇ６`; `9.9〜10.1mm`; `400MPa以上`, `≥ 400 N/mm²`, `0.8 max`, `MAX 0.8`, `10以上10.2以下`, `0.5未満` (exclusive); inch nominal from trailing unit; 4 thread spellings; 17 malformed inputs rejected (`10`, `10 +0.1`, `1,5±0.1`, `10.1.2±0.1`, mixed units …) |
| ISO 14253-1 | spec 10 ± 0.05, U = 10 µm: conformance zone 9.96…10.04; 9 values across pass / indeterminate / fail incl. the exact zone edges; 2U = T → single-point zone; 2U > T → empty zone, Indeterminate with "reduce U"; one-sided 400 MPa min with U = 8 MPa (412 pass, 0.405 GPa indeterminate, 391 N/mm² fail); exclusive `< 0.5` at 0.5 fails; dimension mismatch and negative U refused |
| stack-up | 10 ±0.1 − 4 ±0.05 − 5 ±0.05, gap 1 ±0.15: WC 0.8…1.2 (finding), RSS ±0.1224745, MRSS ±0.1837117, yield 0.9997614365 (= 2Φ(3.674)−1, CPython), housing 66.7 % of variance; housing Cpk 0.5 with mean 10.03 → `YieldBelowTarget` naming housing; single link → MRSS capped; empty chain, Cpk 0, mean outside limits, one-sided link refused |
| normal | Φ at 0, ±1, −3, −6, 2.5 within 1e-11 relative of CPython `math.erfc`; sf(6) to 1e-20 absolute |

Property tests (2000 cases each): unit → canonical → unit is exact for every unit (|mantissa| < 10¹⁸, scale ≤ 12);
WC ≥ MRSS ≥ RSS and 0 ≤ yield ≤ 1 for random 1–19-link chains with asymmetric tolerances; a smaller U never changes
a Pass/Fail decided with a larger U, Pass ⇒ inside spec, Fail ⇒ outside; `describe()` output parses back to identical
limits; parser never panics on arbitrary Unicode (≤ 40 chars) or on corpus notations mutated by insert/delete/
replace/duplicate (≤ 400 chars), and any parsed spec has lower ≤ upper.

## Benchmark (release, median of 7 runs, 10 000 items each)

| operation | median ms | µs / item |
|---|---|---|
| stack-up `analyze`, 8 links/chain (half with Cpk) | 69.11 | 6.91 |
| `parse` (8-notation JA/EN corpus, cycled) | 26.76 | 2.68 |
| ISO 14253-1 `judge` (incl. explanation string) | 20.21 | 2.02 |

10 000 chains take ~70 ms on one core; formatting the explanation dominates `judge`. No optimisation was attempted.

## Known limits
ISO 286 letters d–p / D–P only, IT4–IT13, ≤ 500 mm; thread limits only 6g/6h major diameter; yield is a normal
approximation for independent links (a prediction, not a verification result).

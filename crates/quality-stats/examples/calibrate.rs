//! Calibration of the integrity tests on deterministic synthetic honest vs manipulated datasets.
//! Run: `cargo run --release -p quality-stats --example calibrate` (prints Markdown).
//! Score per dataset = −log10(p) (or the Jaccard estimate for MinHash); a flag means p < ALPHA.
//! AUC = P(score_manipulated > score_honest) + ½·P(tie) (Mann–Whitney U / (n_h·n_m)).

use quality_stats::integrity::{
    LimitSide, MinHasher, benford_first_digit, exact_at_limit, find_near_duplicates, minhash::DEFAULT_SEED,
    rounding_heaping, table_tokens, terminal_digit_preference, variance_too_low,
};
use quality_stats::spc::{RuleSet, check_rules};
use quality_stats::synth::{Rng, honest, record};
use quality_stats::{Applicability, Measurement, TestResult, decimal::to_f64s};
use rust_decimal::Decimal;

const SEED: u64 = 20_261_105;
const REPS: usize = 1000;
const ALPHA: f64 = 0.01;

fn score(r: &Applicability<TestResult>) -> Option<f64> {
    r.applicable().map(|t| -t.p_value.max(1e-300).log10())
}

fn auc(h: &[f64], m: &[f64]) -> f64 {
    let mut s = 0.0;
    for a in m {
        for b in h {
            s += if a > b {
                1.0
            } else if a == b {
                0.5
            } else {
                0.0
            };
        }
    }
    s / (h.len() * m.len()) as f64
}

struct Row {
    test: &'static str,
    scenario: String,
    na_h: usize,
    na_m: usize,
    h: Vec<f64>,
    m: Vec<f64>,
    cut: f64,
}

fn print(rows: &[Row]) {
    println!("| Test | Manipulation | n/a honest | n/a manip. | FPR @ p<{ALPHA} | TPR @ p<{ALPHA} | ROC AUC |");
    println!("|---|---|---|---|---|---|---|");
    for r in rows {
        let rate = |v: &[f64]| v.iter().filter(|s| **s > r.cut).count() as f64 / v.len().max(1) as f64;
        let a = if r.h.is_empty() || r.m.is_empty() {
            "n/a".to_string()
        } else {
            format!("{:.3}", auc(&r.h, &r.m))
        };
        println!(
            "| {} | {} | {} | {} | {:.3} | {:.3} | {a} |",
            r.test,
            r.scenario,
            r.na_h,
            r.na_m,
            rate(&r.h),
            rate(&r.m),
        );
    }
}

fn collect(
    test: &'static str,
    scenario: &str,
    mut gen_pair: impl FnMut(&mut Rng) -> (Applicability<TestResult>, Applicability<TestResult>),
) -> Row {
    let mut rng = Rng::new(SEED ^ quality_stats::integrity::minhash::fnv1a64(format!("{test}{scenario}").as_bytes()));
    let mut row = Row {
        test,
        scenario: scenario.into(),
        na_h: 0,
        na_m: 0,
        h: vec![],
        m: vec![],
        cut: -ALPHA.log10(),
    };
    for _ in 0..REPS {
        let (a, b) = gen_pair(&mut rng);
        match score(&a) {
            Some(s) => row.h.push(s),
            None => row.na_h += 1,
        }
        match score(&b) {
            Some(s) => row.m.push(s),
            None => row.na_m += 1,
        }
    }
    row
}

/// Round the last digit of a fraction `share` of values to the nearest 0 or 5 (hand "tidying").
fn tidy(rng: &mut Rng, ms: &[Measurement], share: f64) -> Vec<Measurement> {
    ms.iter()
        .map(|m| {
            if rng.uniform() < share {
                let u = m.units();
                let r = (u as f64 / 5.0).round() as i64 * 5;
                Measurement::from_decimal(Decimal::new(r, m.scale()))
            } else {
                *m
            }
        })
        .collect()
}

fn main() {
    let mut rows = vec![];
    for share in [0.15, 0.3] {
        let scen = format!("{:.0}% of last digits tidied to 0/5 (n=60, sd=80 units)", share * 100.0);
        rows.push(collect("terminal digit χ²", &scen, |r| {
            let h = honest(r, 60, 25.0, 0.8, 2);
            let base = honest(r, 60, 25.0, 0.8, 2);
            (
                terminal_digit_preference(&h),
                terminal_digit_preference(&tidy(r, &base, share)),
            )
        }));
        rows.push(collect("heaping index", &scen, |r| {
            let h = honest(r, 60, 25.0, 0.8, 2);
            let base = honest(r, 60, 25.0, 0.8, 2);
            (rounding_heaping(&h), rounding_heaping(&tidy(r, &base, share)))
        }));
    }
    rows.push(collect(
        "Benford χ²",
        "invented: uniform first digit over 4 decades (n=200)",
        |r| {
            let h: Vec<Measurement> = (0..200)
                .map(|_| record((50f64.ln() + 1.6 * r.normal()).exp(), 2))
                .collect();
            let m: Vec<Measurement> = (0..200)
                .map(|_| {
                    let d = 1.0 + r.below(9) as f64 + r.uniform();
                    record(d * 10f64.powi(r.below(4) as i32 - 1), 2)
                })
                .collect();
            (benford_first_digit(&h), benford_first_digit(&m))
        },
    ));
    rows.push(collect(
        "Benford guard",
        "controlled process N(520,10) honest vs same (both must be n/a)",
        |r| {
            (
                benford_first_digit(&honest(r, 200, 520.0, 10.0, 1)),
                benford_first_digit(&honest(r, 200, 520.0, 10.0, 1)),
            )
        },
    ));
    for f in [0.5, 0.7] {
        rows.push(collect(
            "variance too low",
            &format!("repeats with sd = {f}·σ0 (n=10)"),
            |r| {
                let h = to_f64s(&honest(r, 10, 10.0, 0.05, 3));
                let m = to_f64s(&honest(r, 10, 10.0, 0.05 * f, 3));
                let t = |v: &[f64]| variance_too_low(v, 0.05, 0.001).expect("n >= 2");
                (t(&h), t(&m))
            },
        ));
    }
    let usl = Measurement::parse("10.00").expect("valid literal");
    rows.push(collect(
        "exact at limit",
        "values > USL clamped to USL (n=50, N(9.5,0.4), USL 10.00)",
        |r| {
            let h = honest(r, 50, 9.5, 0.4, 2);
            let m: Vec<Measurement> = honest(r, 50, 9.5, 0.4, 2)
                .into_iter()
                .map(|v| if v.decimal() > usl.decimal() { usl } else { v })
                .collect();
            (
                exact_at_limit(&h, usl, LimitSide::Upper, None),
                exact_at_limit(&m, usl, LimitSide::Upper, None),
            )
        },
    ));
    print(&rows);

    // MinHash + LSH: 200 independent tables (20×5, N(50,2) at 0.01) plus edited copies of the first 100.
    println!();
    println!(
        "| Cells edited in copy | mean J est. (copies) | max J est. (independent) | LSH b×r, J cut | copies found | other pairs flagged | ROC AUC |"
    );
    println!("|---|---|---|---|---|---|---|");
    for (edit, bands, rows, cut) in [
        (0.1, 32, 4, 0.5),
        (0.3, 32, 4, 0.5),
        (0.3, 64, 2, 0.2),
        (0.5, 64, 2, 0.2),
    ] {
        let mut rng = Rng::new(SEED + (edit * 100.0) as u64);
        let mut tables: Vec<Vec<Vec<Measurement>>> = (0..200)
            .map(|_| (0..20).map(|_| honest(&mut rng, 5, 50.0, 2.0, 2)).collect())
            .collect();
        for t in 0..100 {
            let mut c = tables[t].clone();
            for row in c.iter_mut() {
                for cell in row.iter_mut() {
                    if rng.uniform() < edit {
                        *cell = record(cell.to_f64() + 0.01 * (1 + rng.below(5)) as f64, 2);
                    }
                }
            }
            tables.push(c);
        }
        let h = MinHasher::new(128, DEFAULT_SEED).expect("k > 0");
        let sigs: Vec<_> = tables.iter().map(|t| h.signature(&table_tokens(t))).collect();
        let found = find_near_duplicates(&sigs, bands, rows, cut).expect("valid banding");
        let is_copy = |a: usize, b: usize| b >= 200 && a == b - 200;
        let tp = found.iter().filter(|p| is_copy(p.a, p.b)).count();
        let copies: Vec<f64> = (0..100).map(|t| sigs[t].jaccard(&sigs[t + 200])).collect();
        let indep: Vec<f64> = (0..200)
            .flat_map(|a| ((a + 1)..200).map(move |b| (a, b)))
            .map(|(a, b)| sigs[a].jaccard(&sigs[b]))
            .collect();
        let max_indep = indep.iter().copied().fold(0.0, f64::max);
        println!(
            "| {:.0}% | {:.3} | {max_indep:.3} | {bands}×{rows}, {cut} | {tp}/100 | {} of {} | {:.3} |",
            edit * 100.0,
            copies.iter().sum::<f64>() / 100.0,
            found.len() - tp,
            300 * 299 / 2 - 100,
            auc(&indep, &copies)
        );
    }

    // SPC false-alarm rate on in-control data (context for run rules).
    println!();
    let mut rng = Rng::new(SEED);
    for set in [RuleSet::WesternElectric, RuleSet::Nelson] {
        let any = (0..REPS)
            .filter(|_| {
                let pts: Vec<f64> = (0..25).map(|_| rng.normal()).collect();
                !check_rules(&pts, 0.0, 1.0, set).expect("sigma > 0").is_empty()
            })
            .count();
        println!(
            "- {set:?}: {:.3} of {REPS} in-control 25-point series raise ≥ 1 signal",
            any as f64 / REPS as f64
        );
    }
}

//! Simple wall-clock benchmark on 100 000 values (median of 7 runs).
//! Run: `cargo run --release -p quality-stats --example bench`.

use quality_stats::Measurement;
use quality_stats::capability::{Spec, capability};
use quality_stats::decimal::to_f64s;
use quality_stats::integrity::{
    LimitSide, MinHasher, benford_first_digit, exact_at_limit, minhash::DEFAULT_SEED, rounding_heaping, table_tokens,
    terminal_digit_preference, variance_too_low,
};
use quality_stats::spc::{RuleSet, check_rules, cusum, ewma, individuals_moving_range, xbar_r};
use quality_stats::synth::{Rng, honest};
use std::hint::black_box;
use std::time::Instant;

const N: usize = 100_000;

fn time(name: &str, mut f: impl FnMut()) {
    let mut t: Vec<f64> = (0..7)
        .map(|_| {
            let s = Instant::now();
            f();
            s.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    t.sort_by(f64::total_cmp);
    println!("| {name} | {:.2} |", t[3]);
}

fn main() {
    let mut rng = Rng::new(1);
    let ms = honest(&mut rng, N, 25.0, 0.8, 2);
    let texts: Vec<String> = ms.iter().map(|m| m.decimal().to_string()).collect();
    let xs = to_f64s(&ms);
    let groups: Vec<&[f64]> = xs.chunks(5).collect();
    let usl = Measurement::parse("27.00").expect("valid literal");
    let tables: Vec<Vec<Vec<Measurement>>> = ms
        .chunks(100)
        .map(|c| c.chunks(5).map(<[_]>::to_vec).collect())
        .collect();
    println!("| Operation on {N} values | median ms |");
    println!("|---|---|");
    time("parse decimal strings", || {
        black_box(Measurement::parse_all(&texts).ok());
    });
    time("terminal digit χ²", || {
        black_box(terminal_digit_preference(&ms));
    });
    time("heaping index", || {
        black_box(rounding_heaping(&ms));
    });
    time("Benford (guard refuses)", || {
        black_box(benford_first_digit(&ms));
    });
    time("variance too low", || {
        black_box(variance_too_low(&xs, 0.8, 0.01).ok());
    });
    time("exact at limit (median/MAD)", || {
        black_box(exact_at_limit(&ms, usl, LimitSide::Upper, None));
    });
    time("capability (I-MR sigma, CIs)", || {
        black_box(
            capability(
                &xs,
                Spec {
                    lsl: Some(22.0),
                    usl: Some(28.0),
                },
                None,
                0.95,
            )
            .ok(),
        );
    });
    time("X̄-R chart (20 000 subgroups of 5)", || {
        black_box(xbar_r(&groups).ok());
    });
    time("I-MR chart", || {
        black_box(individuals_moving_range(&xs).ok());
    });
    time("EWMA", || {
        black_box(ewma(&xs, 25.0, 0.8, 0.2, 3.0).ok());
    });
    time("CUSUM", || {
        black_box(cusum(&xs, 25.0, 0.8, 0.5, 5.0).ok());
    });
    time("Nelson rules", || {
        black_box(check_rules(&xs, 25.0, 0.8, RuleSet::Nelson).ok());
    });
    time("MinHash k=128 + LSH (1000 tables × 100 values)", || {
        let h = MinHasher::new(128, DEFAULT_SEED).expect("k > 0");
        let sigs: Vec<_> = tables.iter().map(|t| h.signature(&table_tokens(t))).collect();
        black_box(quality_stats::integrity::find_near_duplicates(&sigs, 32, 4, 0.5).ok());
    });
}

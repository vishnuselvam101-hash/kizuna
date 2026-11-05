//! Wall-clock benchmark: 10 000 stack-up chains (8 links each, half with Cpk data), 10 000 parses, 10 000
//! conformance decisions. Median of 7 runs. Run: `cargo run --release -p tolerance --example bench`.

use rust_decimal::Decimal;
use std::hint::black_box;
use std::time::Instant;
use tolerance::*;

const N: usize = 10_000;
const LINKS: usize = 8;

fn time(name: &str, mut f: impl FnMut()) {
    let mut t: Vec<f64> = (0..7)
        .map(|_| {
            let s = Instant::now();
            f();
            s.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    t.sort_by(f64::total_cmp);
    println!("| {name} | {:.2} | {:.2} |", t[3], t[3] * 1e3 / N as f64);
}

/// Deterministic xorshift so the bench needs no extra crate.
struct Rng(u64);
impl Rng {
    fn next(&mut self, n: u64) -> i64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n) as i64
    }
}

fn main() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let chains: Vec<Vec<Link>> = (0..N)
        .map(|c| {
            (0..LINKS)
                .map(|i| {
                    let nom = Decimal::new(1_000 + rng.next(50_000), 3);
                    let tol = Decimal::new(5 + rng.next(100), 3);
                    Link {
                        name: format!("c{c}l{i}"),
                        direction: if i == 0 { Direction::Plus } else { Direction::Minus },
                        spec: Spec::symmetric(nom, tol, Unit::Mm).expect("spec"),
                        process: (i % 2 == 0).then_some(Process::Cpk {
                            mean: None,
                            cpk: 1.0 + rng.next(100) as f64 / 100.0,
                        }),
                    }
                })
                .collect()
        })
        .collect();
    let gap = Spec::symmetric(Decimal::ONE, Decimal::new(2, 1), Unit::Mm).expect("gap");
    let opts = StackOptions::default();
    let corpus = [
        "φ10 +0.1/−0",
        "10±0.05",
        "25 h7",
        "M8×1.25 6g",
        "１０±０．０５",
        "9.9〜10.1mm",
        "400MPa以上",
        "10 mm ± 50 um",
    ];
    let texts: Vec<String> = (0..N).map(|i| corpus[i % corpus.len()].to_string()).collect();
    let spec = Spec::symmetric(Decimal::TEN, Decimal::new(5, 2), Unit::Mm).expect("spec");
    let ys: Vec<Quantity> = (0..N)
        .map(|_| Quantity::new(Decimal::new(9_900 + rng.next(200), 3), Unit::Mm).expect("q"))
        .collect();
    let u = Quantity::new(Decimal::new(10, 0), Unit::Um).expect("u");

    println!("| operation ({N} items) | median ms | µs / item |\n|---|---|---|");
    let mut findings = 0;
    time("stack-up analyze, 8 links", || {
        findings = 0;
        for c in &chains {
            findings += black_box(analyze(c, &gap, &opts).expect("ok")).findings.len();
        }
    });
    time("parse notation", || {
        for t in &texts {
            black_box(parse(t).expect("ok"));
        }
    });
    time("ISO 14253-1 judge", || {
        for y in &ys {
            black_box(judge(&spec, *y, u).expect("ok"));
        }
    });
    println!("\nfindings across {N} chains: {findings}");
}

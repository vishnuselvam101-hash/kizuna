//! Property tests for the Monte Carlo stack-up: determinism, yield/CI sanity, CI shrinkage, agreement with the
//! normal model of `analyze()`, and Wilson-interval coverage of an exactly known yield.

use proptest::prelude::*;
use rust_decimal::Decimal;
use tolerance::*;

fn dec(m: i64, scale: u32) -> Decimal {
    Decimal::new(m, scale)
}

fn chain(spec: &[(i64, i64, bool, Option<f64>)]) -> Vec<Link> {
    spec.iter()
        .enumerate()
        .map(|(i, &(n, t, plus, sigma))| Link {
            name: format!("L{i}"),
            direction: if plus { Direction::Plus } else { Direction::Minus },
            spec: Spec::symmetric(dec(n, 2), dec(t, 3), Unit::Mm).expect("spec"),
            process: sigma.map(|s| Process::Sigma { mean: None, sigma: s }),
        })
        .collect()
}

fn links_strategy() -> impl Strategy<Value = Vec<(i64, i64, bool, Option<f64>)>> {
    proptest::collection::vec(
        (
            100i64..10_000,
            5i64..200,
            any::<bool>(),
            proptest::option::of(0.001f64..0.05),
        ),
        1..6,
    )
}

/// Assembly spec centred on the nominal gap with a half-width `w` (thousandths of a mm).
fn gap(links: &[Link], w: i64) -> Spec {
    let nom: Decimal = links
        .iter()
        .map(|l| {
            let n = l.spec.nominal.expect("nominal");
            if l.direction == Direction::Plus { n } else { -n }
        })
        .sum();
    Spec::symmetric(nom, dec(w, 3), Unit::Mm).expect("gap")
}

/// A random explicit distribution for a link (lengths in mm around 10).
fn dist_strategy() -> impl Strategy<Value = Option<Distribution>> {
    prop_oneof![
        Just(None),
        (9.9f64..10.0, 0.01f64..0.2).prop_map(|(lo, w)| Some(Distribution::Uniform { lo, hi: lo + w })),
        (9.9f64..10.0, 0.0f64..1.0, 0.01f64..0.2).prop_map(|(lo, f, w)| Some(Distribution::Triangular {
            lo,
            mode: lo + f * w,
            hi: lo + w
        })),
        (-3.0f64..-1.0, 0.1f64..1.0, any::<bool>()).prop_map(|(mu, sigma, left_skewed)| Some(
            Distribution::ShiftedLogNormal {
                shift: 10.0,
                mu,
                sigma,
                left_skewed
            }
        )),
    ]
}

/// (P(X ≤ k), P(X ≥ k)) for X ~ Binomial(n, p), by summing the pmf in log space.
fn binomial_tails(k: usize, n: usize, p: f64) -> (f64, f64) {
    let p = p.clamp(0.0, 1.0);
    if p == 0.0 {
        return (1.0, if k == 0 { 1.0 } else { 0.0 });
    }
    let mut ln_c = 0.0; // ln C(n, i)
    let (mut le, mut ge) = (0.0, 0.0);
    for i in 0..=n {
        if i > 0 {
            ln_c += ((n - i + 1) as f64).ln() - (i as f64).ln();
        }
        let pmf = (ln_c + i as f64 * p.ln() + (n - i) as f64 * (-p).ln_1p()).exp();
        if i <= k {
            le += pmf;
        }
        if i >= k {
            ge += pmf;
        }
    }
    (le.min(1.0), ge.min(1.0))
}

proptest! {
    // Fixed seed: the property runs are reproducible (MATH v2 rule: deterministic seeds).
    #![proptest_config(ProptestConfig {
        cases: 48,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20_261_106),
        ..ProptestConfig::default()
    })]

    #[test]
    fn same_seed_same_report_and_yield_is_sane(
        spec in links_strategy(), dists in proptest::collection::vec(dist_strategy(), 6), w in 1i64..400,
        seed in any::<u64>(), n in 1usize..3_000,
    ) {
        let links = chain(&spec);
        let dists = &dists[..links.len()];
        let g = gap(&links, w);
        let a = monte_carlo(&links, dists, &g, n, seed).expect("mc");
        let b = monte_carlo(&links, dists, &g, n, seed).expect("mc");
        prop_assert_eq!(&a, &b);
        prop_assert!((0.0..=1.0).contains(&a.yield_estimate));
        prop_assert!(a.yield_ci95.0 <= a.yield_estimate && a.yield_estimate <= a.yield_ci95.1, "{a:?}");
        prop_assert!(0.0 <= a.yield_ci95.0 && a.yield_ci95.1 <= 1.0);
        prop_assert!(a.q_lower <= a.median && a.median <= a.q_upper);
        prop_assert_eq!(a.n, n);
        prop_assert_eq!(a.seed, seed);
        prop_assert!((a.ppm_out - (1.0 - a.yield_estimate) * 1e6).abs() < 1e-6);
    }

    #[test]
    fn wilson_width_shrinks_with_n(k in 0usize..1_000, extra in 0usize..1_000, mult in 2usize..50) {
        let n = k + extra + 1;
        let (lo1, hi1) = wilson_ci(k, n, Z95);
        let (lo2, hi2) = wilson_ci(k * mult, n * mult, Z95);
        prop_assert!(hi2 - lo2 < hi1 - lo1, "n={n} width {} vs n={} width {}", hi1 - lo1, n * mult, hi2 - lo2);
        let p = k as f64 / n as f64;
        prop_assert!(lo1 <= p && p <= hi1 && lo2 <= p && p <= hi2);
    }

    /// Normal links (fallback = analyze's model) must reproduce analyze()'s predicted yield: the failure count must
    /// not be in a binomial tail below 1e-6 (a 95 % interval would miss 5 % of generated cases by construction).
    #[test]
    fn all_normal_mc_matches_analyze(spec in links_strategy(), w in 20i64..300, seed in any::<u64>()) {
        let links = chain(&spec);
        let g = gap(&links, w);
        let r = analyze(&links, &g, &StackOptions::default()).expect("analyze");
        let n = 20_000;
        let mc = monte_carlo(&links, &vec![None; links.len()], &g, n, seed).expect("mc");
        // Exact two-sided binomial check of the failure count against analyze()'s failure probability (a Wilson
        // interval misjudges yields within a few ppm of 1, where one failure in 20 000 is already unusual).
        let (lo_tail, hi_tail) = binomial_tails(n - mc.passed, n, 1.0 - r.predicted_yield);
        prop_assert!(lo_tail > 1e-6 && hi_tail > 1e-6,
            "analyze {} vs mc {} (P(X ≤ k) {lo_tail:e}, P(X ≥ k) {hi_tail:e})", r.predicted_yield, mc.yield_estimate);
        prop_assert!((mc.mean - r.mean).abs() <= 5.0 * r.sigma / (n as f64).sqrt() + 1e-12);
    }
}

#[test]
fn ci_shrinks_with_more_samples_on_a_real_stack() {
    let links = chain(&[(1000, 100, true, None), (500, 50, false, None)]);
    let g = gap(&links, 120);
    let small = monte_carlo(&links, &[None, None], &g, 500, 7).expect("mc");
    let large = monte_carlo(&links, &[None, None], &g, 50_000, 7).expect("mc");
    let w = |r: &McReport| r.yield_ci95.1 - r.yield_ci95.0;
    assert!(w(&large) < w(&small) / 5.0, "{} vs {}", w(&large), w(&small));
}

/// Two uniform links U(9.9, 10.1) sum to a triangular gap on [19.8, 20.2]; the spec 20 ± 0.16 has the exact yield
/// 1 − 2 · (0.04² / (2 · 0.2²)) = 0.96. The Wilson 95 % interval must cover it in ≥ 93 % of 500 seeds.
#[test]
fn wilson_interval_covers_exact_triangular_yield() {
    let links = chain(&[(1000, 100, true, None), (1000, 100, true, None)]);
    let u = Some(Distribution::Uniform { lo: 9.9, hi: 10.1 });
    let g = Spec::symmetric(dec(20, 0), dec(16, 2), Unit::Mm).expect("gap");
    let truth = 0.96;
    let covered = (0..500u64)
        .filter(|&s| {
            let r = monte_carlo(&links, &[u, u], &g, 1_000, 1_000 + s).expect("mc");
            r.yield_ci95.0 <= truth && truth <= r.yield_ci95.1
        })
        .count();
    let coverage = covered as f64 / 500.0;
    assert!(coverage >= 0.93, "coverage {coverage}");
}

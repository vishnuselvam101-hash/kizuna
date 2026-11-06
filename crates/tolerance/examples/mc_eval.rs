//! Measurement: normal-approximation yield of `analyze()` (BEFORE, what main ships) versus the Monte Carlo yield
//! with its Wilson 95 % CI (AFTER), against a reference truth. Fixed seeds; prints Markdown.
//! Run: `cargo run --release -p tolerance --example mc_eval`.
//!
//! Truth: exact Irwin–Hall CDF (case 1); 1-D Simpson quadrature over the lognormal's normal driver, exact up to
//! quadrature error < 1e-10 (cases 3, 4); a 10⁷-sample Monte Carlo with different seeds (case 2, no closed form).

use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use std::str::FromStr;
use std::time::Instant;
use tolerance::*;

const N: usize = 100_000;
const SEED: u64 = 2026;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).expect("decimal")
}

fn df(x: f64) -> Decimal {
    Decimal::from_f64(x).expect("finite")
}

fn link(name: &str, dir: Direction, spec: Spec, process: Option<Process>) -> Link {
    Link {
        name: name.into(),
        direction: dir,
        spec,
        process,
    }
}

fn sym(nom: &str, tol: &str) -> Spec {
    Spec::symmetric(d(nom), d(tol), Unit::Mm).expect("spec")
}

fn with_process(links: &[Link], p: &[Option<Process>]) -> Vec<Link> {
    links
        .iter()
        .zip(p)
        .map(|(l, p)| Link {
            process: *p,
            ..l.clone()
        })
        .collect()
}

/// Irwin–Hall CDF of the sum of k U(0,1).
fn irwin_hall_cdf(x: f64, k: u32) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= f64::from(k) {
        return 1.0;
    }
    let (mut s, mut binom, mut fact) = (0.0, 1.0, 1.0);
    for i in 1..=k {
        fact *= f64::from(i);
    }
    for j in 0..=(x.floor() as u32) {
        let sign = if j % 2 == 0 { 1.0 } else { -1.0 };
        s += sign * binom * (x - f64::from(j)).powi(k as i32);
        binom = binom * f64::from(k - j) / f64::from(j + 1);
    }
    s / fact
}

/// P(lo ≤ base + exp(mu + sigma·Z) + N(0, sn²) ≤ hi) by Simpson's rule over Z ∈ [−9, 9].
fn lognormal_plus_normal_yield(base: f64, mu: f64, sigma: f64, sn: f64, lo: f64, hi: f64) -> f64 {
    let steps = 36_000;
    let (a, b) = (-9.0_f64, 9.0_f64);
    let h = (b - a) / f64::from(steps);
    let f = |z: f64| {
        let g = base + (mu + sigma * z).exp();
        let phi = (-0.5 * z * z).exp() / (2.0 * std::f64::consts::PI).sqrt();
        phi * (normal::cdf((hi - g) / sn) - normal::cdf((lo - g) / sn))
    };
    let mut s = f(a) + f(b);
    for i in 1..steps {
        let w = if i % 2 == 1 { 4.0 } else { 2.0 };
        s += w * f(a + f64::from(i) * h);
    }
    s * h / 3.0
}

struct Row {
    case: &'static str,
    truth: f64,
    truth_how: String,
    before_cp1: f64,
    before_fit: f64,
    fit_label: &'static str,
    after: McReport,
    ms: f64,
}

fn timed_mc(links: &[Link], dists: &[Option<Distribution>], gap: &Spec) -> (McReport, f64) {
    let t = Instant::now();
    let r = monte_carlo(links, dists, gap, N, SEED).expect("mc");
    (r, t.elapsed().as_secs_f64() * 1e3)
}

fn yield_of(links: &[Link], gap: &Spec) -> f64 {
    analyze(links, gap, &StackOptions::default())
        .expect("analyze")
        .predicted_yield
}

fn coverage(links: &[Link], dists: &[Option<Distribution>], gap: &Spec, truth: f64, seeds: u64, n: usize) -> f64 {
    let hit = (0..seeds)
        .filter(|s| {
            let r = monte_carlo(links, dists, gap, n, 50_000 + s).expect("mc");
            r.yield_ci95.0 <= truth && truth <= r.yield_ci95.1
        })
        .count();
    hit as f64 / seeds as f64
}

fn main() {
    let mut rows = Vec::new();

    // (1) Four uniform links 10 ± 0.1 (flat over the full tolerance), gap 40 ± 0.25.
    let l1: Vec<Link> = (0..4)
        .map(|i| link(&format!("U{i}"), Direction::Plus, sym("10", "0.1"), None))
        .collect();
    let g1 = sym("40", "0.25");
    let u = Some(Distribution::Uniform { lo: 9.9, hi: 10.1 });
    let d1 = vec![u; 4];
    let truth1 = 1.0 - 2.0 * irwin_hall_cdf(0.75, 4);
    let sig_u = Some(Process::Sigma {
        mean: None,
        sigma: 0.2 / 12f64.sqrt(),
    });
    let (r1, ms1) = timed_mc(&l1, &d1, &g1);
    rows.push(Row {
        case: "1 four uniform links",
        truth: truth1,
        truth_how: "exact Irwin–Hall".into(),
        before_cp1: yield_of(&l1, &g1),
        before_fit: yield_of(&with_process(&l1, &[sig_u; 4]), &g1),
        fit_label: "σ-matched",
        after: r1,
        ms: ms1,
    });

    // (2) Three skewed triangular links, gap 1 ± 0.1.
    let l2 = vec![
        link("A", Direction::Plus, sym("25", "0.05"), None),
        link("B", Direction::Minus, sym("12", "0.04"), None),
        link("C", Direction::Minus, sym("12", "0.04"), None),
    ];
    let g2 = sym("1", "0.1");
    let ta = Distribution::Triangular {
        lo: 24.95,
        mode: 24.97,
        hi: 25.05,
    };
    let tb = Distribution::Triangular {
        lo: 11.96,
        mode: 12.03,
        hi: 12.04,
    };
    let d2 = vec![Some(ta), Some(tb), Some(tb)];
    let tri_sd = |a: f64, b: f64, c: f64| ((a * a + b * b + c * c - a * b - a * c - b * c) / 18.0).sqrt();
    let p2 = [
        Some(Process::Sigma {
            mean: Some(d("24.99")),
            sigma: tri_sd(24.95, 24.97, 25.05),
        }),
        Some(Process::Sigma {
            mean: Some(d("12.01")),
            sigma: tri_sd(11.96, 12.03, 12.04),
        }),
        Some(Process::Sigma {
            mean: Some(d("12.01")),
            sigma: tri_sd(11.96, 12.03, 12.04),
        }),
    ];
    let t = Instant::now();
    let (mut ref_pass, ref_n) = (0usize, 10 * 1_000_000usize);
    for c in 0..10u64 {
        ref_pass += monte_carlo(&l2, &d2, &g2, 1_000_000, 10_000_000 + c)
            .expect("ref")
            .passed;
    }
    let ref_ms = t.elapsed().as_secs_f64() * 1e3;
    let truth2 = ref_pass as f64 / ref_n as f64;
    let ref_ci = wilson_ci(ref_pass, ref_n, Z95);
    let (r2, ms2) = timed_mc(&l2, &d2, &g2);
    rows.push(Row {
        case: "2 triangular links",
        truth: truth2,
        truth_how: format!(
            "MC 10⁷, seeds 10000000.. (95% CI {:.5}–{:.5}, {:.0} ms)",
            ref_ci.0, ref_ci.1, ref_ms
        ),
        before_cp1: yield_of(&l2, &g2),
        before_fit: yield_of(&with_process(&l2, &p2), &g2),
        fit_label: "σ-matched",
        after: r2,
        ms: ms2,
    });

    // (3) One strongly skewed link dominating: L = 20 + exp(ln 0.05 + 0.5 Z) (drawing 20 +0.15/0), minus two
    //     normal links 8 ± 0.02 with σ = 0.005; gap 4 +0.15/0.
    let (mu, sg, sn) = (0.05f64.ln(), 0.5f64, 0.005f64);
    let lspec = Spec::asymmetric(d("20"), d("0.15"), d("0"), Unit::Mm).expect("spec");
    let nproc = Some(Process::Sigma { mean: None, sigma: sn });
    let l3 = vec![
        link("skewed", Direction::Plus, lspec, None),
        link("N1", Direction::Minus, sym("8", "0.02"), nproc),
        link("N2", Direction::Minus, sym("8", "0.02"), nproc),
    ];
    let g3 = Spec::asymmetric(d("4"), d("0.15"), d("0"), Unit::Mm).expect("gap");
    let logn = Distribution::ShiftedLogNormal {
        shift: 20.0,
        mu,
        sigma: sg,
        left_skewed: false,
    };
    let d3 = vec![Some(logn), None, None];
    let truth3 = lognormal_plus_normal_yield(4.0, mu, sg, sn * 2f64.sqrt(), 4.0, 4.15);
    let ln_mean = (mu + sg * sg / 2.0).exp();
    let ln_sd = ln_mean * ((sg * sg).exp() - 1.0).sqrt();
    let p3 = [
        Some(Process::Sigma {
            mean: Some(df(20.0 + ln_mean)),
            sigma: ln_sd,
        }),
        nproc,
        nproc,
    ];
    let (r3, ms3) = timed_mc(&l3, &d3, &g3);
    rows.push(Row {
        case: "3 skewed lognormal link",
        truth: truth3,
        truth_how: "quadrature".into(),
        before_cp1: yield_of(&l3, &g3),
        before_fit: yield_of(&with_process(&l3, &p3), &g3),
        fit_label: "σ-matched",
        after: r3,
        ms: ms3,
    });

    // (4) Same stack, but the skewed link is known only from 500 measured parts (seed 77).
    let mut rng = SplitMix64::new(77);
    let sample: Vec<f64> = (0..500).map(|_| logn.sample(&mut rng)).collect();
    let fit = JohnsonFit::from_data(&sample).expect("johnson");
    let pearson = PearsonDiagnostic::from_data(&sample).expect("pearson");
    let sm = sample.iter().sum::<f64>() / 500.0;
    let ssd = (sample.iter().map(|x| (x - sm) * (x - sm)).sum::<f64>() / 499.0).sqrt();
    let p4 = [
        Some(Process::Sigma {
            mean: Some(df(sm)),
            sigma: ssd,
        }),
        nproc,
        nproc,
    ];
    let d4 = vec![Some(Distribution::Johnson(fit)), None, None];
    let (r4, ms4) = timed_mc(&l3, &d4, &g3);
    rows.push(Row {
        case: "4 Johnson fit, 500 parts",
        truth: truth3,
        truth_how: "quadrature (true lognormal)".into(),
        before_cp1: yield_of(&l3, &g3),
        before_fit: yield_of(&with_process(&l3, &p4), &g3),
        fit_label: "normal fit",
        after: r4,
        ms: ms4,
    });

    println!("## Monte Carlo stack-up yield vs normal approximation\n");
    println!("AFTER: `monte_carlo`, n = {N}, seed = {SEED}. BEFORE: `analyze()` predicted_yield with no process data");
    println!("(default Cp = 1) and with the true (or fitted) mean/σ as `Process::Sigma`.\n");
    println!(
        "| case | truth | truth method | BEFORE Cp=1 | BEFORE σ/fit | AFTER MC [95% CI] | err Cp=1 | err σ/fit | err MC | CI covers truth | MC ms |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    for r in &rows {
        let a = &r.after;
        println!(
            "| {} | {:.6} | {} | {:.6} | {:.6} ({}) | {:.6} [{:.6}, {:.6}] | {:.2e} | {:.2e} | {:.2e} | {} | {:.1} |",
            r.case,
            r.truth,
            r.truth_how,
            r.before_cp1,
            r.before_fit,
            r.fit_label,
            a.yield_estimate,
            a.yield_ci95.0,
            a.yield_ci95.1,
            (r.before_cp1 - r.truth).abs(),
            (r.before_fit - r.truth).abs(),
            (a.yield_estimate - r.truth).abs(),
            if a.yield_ci95.0 <= r.truth && r.truth <= a.yield_ci95.1 {
                "yes"
            } else {
                "no"
            },
            r.ms
        );
    }
    println!(
        "\nCase 4 fit: Johnson {:?} (γ {:.4}, δ {:.4}, ξ {:.5}, λ {:.5}); sample Pearson β₁ {:.3}, β₂ {:.3}, κ {:.3} → type {:?} (true lognormal σ=0.5 is type VI).",
        fit.family, fit.gamma, fit.delta, fit.xi, fit.lambda, pearson.beta1, pearson.beta2, pearson.kappa, pearson.kind
    );
    println!(
        "The case-4 CI covers Monte Carlo error only, not the error of fitting 500 parts; a miss there is fit error."
    );

    println!("\n### Wilson 95% CI coverage over 500 seeds (n = 10 000 each, seeds 50000..50499)\n");
    println!("| case | truth | coverage |");
    println!("|---|---|---|");
    println!(
        "| 1 four uniform links (exact) | {truth1:.6} | {:.3} |",
        coverage(&l1, &d1, &g1, truth1, 500, 10_000)
    );
    println!(
        "| 3 skewed lognormal link (quadrature) | {truth3:.6} | {:.3} |",
        coverage(&l3, &d3, &g3, truth3, 500, 10_000)
    );
}

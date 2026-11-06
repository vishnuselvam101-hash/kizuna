//! MATH v2 measurements (deterministic; prints Markdown): (a) multiple-testing control for certificate integrity
//! tests, (d) short-run SPC and BOCPD vs the existing charts, (e) pooled / Bayesian variance-too-low vs the
//! existing per-characteristic χ² test. Run: `cargo run --release -p quality-stats --example math_v2_eval`.
//! All data is SYNTHETIC (generator and tests written together); it shows how the methods behave on known
//! patterns, not accuracy on real certificates.

use quality_stats::Measurement;
use quality_stats::decimal::to_f64s;
use quality_stats::integrity::{
    VarianceGroup, rounding_heaping, shrink_posterior, terminal_digit_preference, variance_too_low,
    variance_too_low_estimated_sigma, variance_too_low_pooled,
};
use quality_stats::multitest::{
    SupplierCalibration, benjamini_hochberg, conformal_combined_p, fisher_combine, p_score, stouffer_combine,
};
use quality_stats::spc::{NigPrior, RuleSet, bocpd, check_rules, dnom, individuals_moving_range, q_chart};
use quality_stats::synth::{Rng, honest, record};
use rust_decimal::Decimal;

const SEED: u64 = 20_261_106;
const ALPHA: f64 = 0.01;
const SIGMA0: f64 = 0.05;
const RES: f64 = 0.001;

fn p_of(r: &quality_stats::Applicability<quality_stats::TestResult>) -> Option<f64> {
    r.applicable().map(|t| t.p_value)
}

/// Round the last digit of a share of values to the nearest 0 or 5 (hand "tidying"), as in `calibrate.rs`.
fn tidy(rng: &mut Rng, ms: &[Measurement], share: f64) -> Vec<Measurement> {
    ms.iter()
        .map(|m| {
            if rng.uniform() < share {
                let r = (m.units() as f64 / 5.0).round() as i64 * 5;
                Measurement::from_decimal(Decimal::new(r, m.scale()))
            } else {
                *m
            }
        })
        .collect()
}

fn rate(v: &[bool]) -> f64 {
    v.iter().filter(|x| **x).count() as f64 / v.len().max(1) as f64
}

// ---------------------------------------------------------------- (e) variance too low

fn chars(rng: &mut Rng, k: usize, smoothed: usize, f: f64, sd: f64) -> Vec<Vec<f64>> {
    (0..k)
        .map(|i| {
            let s = if i < smoothed { sd * f } else { sd };
            to_f64s(&honest(rng, 10, 10.0 + i as f64, s, 3))
        })
        .collect()
}

fn existing_any(cs: &[Vec<f64>], sigma: f64) -> (bool, f64) {
    // Existing practice: the per-characteristic χ² test, flag the certificate if any characteristic has p < α.
    let min_p = cs
        .iter()
        .map(|c| {
            variance_too_low(c, sigma, RES)
                .ok()
                .and_then(|r| p_of(&r))
                .unwrap_or(1.0)
        })
        .fold(1.0, f64::min);
    (min_p < ALPHA, min_p)
}

fn pooled(cs: &[Vec<f64>], sigma: f64) -> (f64, f64, f64) {
    let g: Vec<VarianceGroup> = cs
        .iter()
        .map(|c| VarianceGroup {
            values: c.clone(),
            stated_sigma: sigma,
            resolution: RES,
        })
        .collect();
    let r = variance_too_low_pooled(&g).expect("valid groups");
    let r = r.applicable().expect("applicable");
    let bf = shrink_posterior(r.statistic, r.df, 0.8).expect("valid").bayes_factor;
    (r.p_value, bf, r.statistic)
}

/// Threshold on `scores` (larger = more suspicious) that flags `target` of them: the ⌈target·n⌉-th largest.
fn cut_for_rate(mut scores: Vec<f64>, target: f64) -> f64 {
    scores.sort_by(|a, b| b.total_cmp(a));
    let k = ((target * scores.len() as f64).round() as usize).clamp(1, scores.len());
    scores[k - 1]
}

fn section_e() {
    const REPS: usize = 4000;
    println!("## (e) Variance too low: pooled likelihood-ratio / Bayes factor vs the existing χ² test\n");
    println!(
        "{REPS} honest + {REPS} manipulated certificates per row; σ0 = {SIGMA0}, resolution {RES}, 10 repeats per characteristic. \
Existing = per-characteristic lower-tail χ² (`variance_too_low`), certificate flagged if any p < {ALPHA}. New = pooled LR (`variance_too_low_pooled`) \
and Bayes factor (`shrink_posterior`), thresholds set on a separate honest calibration set (other seed) to the existing test's measured FPR.\n"
    );
    println!(
        "| K chars | smoothed | sd factor | existing FPR | existing recall | pooled-LR FPR | pooled-LR recall | Bayes-factor FPR | Bayes-factor recall |"
    );
    println!("|---|---|---|---|---|---|---|---|---|");
    for (k, sm, f) in [
        (1, 1, 0.5),
        (1, 1, 0.7),
        (5, 5, 0.5),
        (5, 5, 0.7),
        (5, 2, 0.5),
        (5, 2, 0.7),
    ] {
        let mut rng = Rng::new(SEED ^ (k * 100 + sm * 10) as u64 ^ (f * 1000.0) as u64);
        let mut cal = Rng::new(SEED.wrapping_mul(31) ^ (k * 100 + sm * 10) as u64 ^ (f * 1000.0) as u64);
        let honest_c: Vec<_> = (0..REPS).map(|_| chars(&mut rng, k, 0, 1.0, SIGMA0)).collect();
        let manip_c: Vec<_> = (0..REPS).map(|_| chars(&mut rng, k, sm, f, SIGMA0)).collect();
        let cal_c: Vec<_> = (0..REPS).map(|_| chars(&mut cal, k, 0, 1.0, SIGMA0)).collect();
        let ex_h: Vec<bool> = honest_c.iter().map(|c| existing_any(c, SIGMA0).0).collect();
        let ex_m: Vec<bool> = manip_c.iter().map(|c| existing_any(c, SIGMA0).0).collect();
        let fpr = rate(&ex_h);
        let cal_scores: Vec<(f64, f64)> = cal_c
            .iter()
            .map(|c| {
                let (p, bf, _) = pooled(c, SIGMA0);
                (-p.max(1e-300).log10(), bf.ln())
            })
            .collect();
        let cut_p = cut_for_rate(cal_scores.iter().map(|s| s.0).collect(), fpr);
        let cut_b = cut_for_rate(cal_scores.iter().map(|s| s.1).collect(), fpr);
        let flag = |set: &[Vec<Vec<f64>>]| -> (Vec<bool>, Vec<bool>) {
            set.iter()
                .map(|c| {
                    let (p, bf, _) = pooled(c, SIGMA0);
                    (-p.max(1e-300).log10() >= cut_p, bf.ln() >= cut_b)
                })
                .unzip()
        };
        let (lh, bh) = flag(&honest_c);
        let (lm, bm) = flag(&manip_c);
        println!(
            "| {k} | {sm} | {f} | {fpr:.4} | {:.3} | {:.4} | {:.3} | {:.4} | {:.3} |",
            rate(&ex_m),
            rate(&lh),
            rate(&lm),
            rate(&bh),
            rate(&bm)
        );
    }
    // σ0 estimated from a gauge study with ν df: the existing test treats σ̂0 as exact.
    println!(
        "\nσ0 *estimated* by a gauge study with ν = 10 df (stated σ̂0 = σ0·√(χ²₁₀/10)); K = 1, 10 repeats; nominal α = {ALPHA}:\n"
    );
    println!("| test | FPR (honest, true σ = σ0) | recall (sd = 0.5·σ0) | recall (sd = 0.3·σ0) |");
    println!("|---|---|---|---|");
    let mut rng = Rng::new(SEED ^ 0xE57);
    let (mut ch, mut fh) = (vec![], vec![]);
    let (mut cm5, mut fm5, mut cm3, mut fm3) = (vec![], vec![], vec![], vec![]);
    for _ in 0..REPS {
        let chi: f64 = (0..10).map(|_| rng.normal().powi(2)).sum();
        let s_hat = SIGMA0 * (chi / 10.0).sqrt();
        let h = to_f64s(&honest(&mut rng, 10, 10.0, SIGMA0, 3));
        let m5 = to_f64s(&honest(&mut rng, 10, 10.0, SIGMA0 * 0.5, 3));
        let m3 = to_f64s(&honest(&mut rng, 10, 10.0, SIGMA0 * 0.3, 3));
        let c = |v: &[f64]| p_of(&variance_too_low(v, s_hat, RES).expect("ok")).unwrap_or(1.0) < ALPHA;
        let fx = |v: &[f64]| {
            p_of(&variance_too_low_estimated_sigma(v, s_hat, 10.0, RES).expect("ok")).unwrap_or(1.0) < ALPHA
        };
        ch.push(c(&h));
        fh.push(fx(&h));
        cm5.push(c(&m5));
        fm5.push(fx(&m5));
        cm3.push(c(&m3));
        fm3.push(fx(&m3));
    }
    println!(
        "| existing χ² (σ̂0 as exact) | {:.4} | {:.3} | {:.3} |",
        rate(&ch),
        rate(&cm5),
        rate(&cm3)
    );
    println!(
        "| F test (`variance_too_low_estimated_sigma`) | {:.4} | {:.3} | {:.3} |\n",
        rate(&fh),
        rate(&fm5),
        rate(&fm3)
    );
}

// ---------------------------------------------------------------- (a) multiple testing

#[derive(Clone, Copy, PartialEq)]
enum Manip {
    None,
    Tidy,
    Smooth,
}

struct Supplier {
    name: &'static str,
    true_sd: f64,
    /// Gauge step in units of the last recorded digit (1 = 0.001; 5 = only …0/…5 last digits).
    step: i64,
}

const MEANS: [f64; 5] = [10.0, 20.0, 5.5, 12.0, 30.0];

fn certificate(rng: &mut Rng, s: &Supplier, m: Manip) -> Vec<f64> {
    let mut all: Vec<Measurement> = Vec::new();
    let mut groups = Vec::new();
    for mu in MEANS {
        let sd = if m == Manip::Smooth { s.true_sd * 0.5 } else { s.true_sd };
        let vals: Vec<Measurement> = (0..10)
            .map(|_| {
                let x = mu + sd * rng.normal();
                let u = (x * 1000.0 / s.step as f64).round() as i64 * s.step;
                Measurement::from_decimal(Decimal::new(u, 3))
            })
            .collect();
        groups.push(to_f64s(&vals));
        all.extend(vals);
    }
    if m == Manip::Tidy {
        all = tidy(rng, &all, 0.3);
        groups = all.chunks(10).map(to_f64s).collect();
    }
    let mut p = vec![p_of(&terminal_digit_preference(&all)), p_of(&rounding_heaping(&all))];
    for g in &groups {
        p.push(variance_too_low(g, SIGMA0, RES).ok().and_then(|r| p_of(&r)));
    }
    p.into_iter().map(p_score).collect() // per-test scores −log10 p (0 = not applicable)
}

fn section_a(nref: usize) {
    const NTEST: usize = 400;
    const NMANIP: usize = 40;
    let suppliers = [
        Supplier {
            name: "S0",
            true_sd: SIGMA0,
            step: 1,
        },
        Supplier {
            name: "S1",
            true_sd: SIGMA0,
            step: 1,
        },
        Supplier {
            name: "S2",
            true_sd: SIGMA0,
            step: 1,
        },
        Supplier {
            name: "S3 (0.005 gauge)",
            true_sd: SIGMA0,
            step: 5,
        },
        Supplier {
            name: "S4 (σ0 stated 1/0.6× too high)",
            true_sd: SIGMA0 * 0.6,
            step: 1,
        },
    ];
    println!("## (a) Multiple-testing control: certificate integrity tests ({nref} references per supplier)\n");
    println!(
        "Per certificate 7 tests (terminal-digit χ², 0/5 heaping, variance-too-low ×5 characteristics; 50 values at 0.001). \
Per supplier {nref} cleared reference certificates, then {NTEST} honest + {NMANIP} tidied (30 % of last digits to 0/5) + {NMANIP} smoothed (sd × 0.5) test certificates. \
Flag rules at α = {ALPHA}: **union** = any test p < α (main today: each signal fires on its own threshold); **Fisher** / **Stouffer** = combined p < α; \
**calibrated 1-stage** = conformal p of the Fisher score against the supplier's own cleared certificates (`SupplierCalibration`) ≤ α; **calibrated 2-stage** = per-test then combined conformal p (`conformal_combined_p`) ≤ α. Batch rows use 2-stage.\n"
    );
    println!(
        "| supplier | FAR union | FAR Fisher | FAR Stouffer | FAR calibrated 1-stage | FAR calibrated 2-stage | recall tidy (union / 1-stage / 2-stage) | recall smooth (union / 1-stage / 2-stage) |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    let mut batch: Vec<(f64, bool, bool)> = Vec::new(); // (calibrated p, manipulated, union flag)
    let mut cal1 = SupplierCalibration::new(20);
    for (si, s) in suppliers.iter().enumerate() {
        let mut rng = Rng::new(SEED ^ ((si as u64 + 1) * 0x9E37));
        let refs: Vec<Vec<f64>> = (0..nref).map(|_| certificate(&mut rng, s, Manip::None)).collect();
        for r in &refs {
            let ps: Vec<f64> = r.iter().map(|x| 10f64.powf(-x)).collect();
            cal1.add_cleared(s.name, -fisher_combine(&ps).expect("p").p_value.max(1e-300).log10())
                .expect("finite");
        }
        let mut rows: Vec<(Manip, [bool; 5], f64)> = Vec::new();
        for (m, n) in [(Manip::None, NTEST), (Manip::Tidy, NMANIP), (Manip::Smooth, NMANIP)] {
            for _ in 0..n {
                let sc = certificate(&mut rng, s, m);
                let ps: Vec<f64> = sc.iter().map(|x| 10f64.powf(-x)).collect();
                let union = ps.iter().any(|p| *p < ALPHA);
                let fisher = fisher_combine(&ps).expect("p").p_value < ALPHA;
                let stouffer = stouffer_combine(&ps, None).expect("p").p_value < ALPHA;
                let cp = conformal_combined_p(&sc, &refs).expect("p");
                let fscore = -fisher_combine(&ps).expect("p").p_value.max(1e-300).log10();
                let one = cal1
                    .calibrated_p(s.name, fscore)
                    .expect("finite")
                    .is_some_and(|c| c.p_value <= ALPHA);
                rows.push((m, [union, fisher, stouffer, one, cp <= ALPHA], cp));
                batch.push((cp, m != Manip::None, union));
            }
        }
        let share = |m: Manip, j: usize| {
            let v: Vec<bool> = rows.iter().filter(|r| r.0 == m).map(|r| r.1[j]).collect();
            rate(&v)
        };
        println!(
            "| {} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} / {:.3} / {:.3} | {:.3} / {:.3} / {:.3} |",
            s.name,
            share(Manip::None, 0),
            share(Manip::None, 1),
            share(Manip::None, 2),
            share(Manip::None, 3),
            share(Manip::None, 4),
            share(Manip::Tidy, 0),
            share(Manip::Tidy, 3),
            share(Manip::Tidy, 4),
            share(Manip::Smooth, 0),
            share(Manip::Smooth, 3),
            share(Manip::Smooth, 4),
        );
    }
    // Batch FDR: BH over the calibrated p-values of every test certificate, vs the union rule.
    let ps: Vec<f64> = batch.iter().map(|b| b.0).collect();
    let manip = batch.iter().filter(|b| b.1).count();
    println!(
        "\nBatch of {} certificates ({manip} manipulated, {:.1} %):\n",
        batch.len(),
        100.0 * manip as f64 / batch.len() as f64
    );
    println!("| rule | flagged | true flags | false discovery proportion | recall |");
    println!("|---|---|---|---|---|");
    let report = |name: &str, flags: &[bool]| {
        let f = flags.iter().filter(|x| **x).count();
        let tp = flags.iter().zip(&batch).filter(|(x, b)| **x && b.1).count();
        println!(
            "| {name} | {f} | {tp} | {:.3} | {:.3} |",
            if f == 0 { 0.0 } else { (f - tp) as f64 / f as f64 },
            tp as f64 / manip as f64
        );
    };
    report(
        "union, each test p < 0.01",
        &batch.iter().map(|b| b.2).collect::<Vec<_>>(),
    );
    report(
        "calibrated p ≤ 0.01",
        &ps.iter().map(|p| *p <= ALPHA).collect::<Vec<_>>(),
    );
    for q in [0.05, 0.10, 0.20] {
        report(
            &format!("calibrated + Benjamini–Hochberg q = {q}"),
            &benjamini_hochberg(&ps, q).expect("valid"),
        );
    }
    println!(
        "\nNote: the smallest calibrated p with {nref} references is 1/{} = {:.4}; BH over {} certificates needs p ≤ q·rank/m, \
so with discrete conformal p-values BH can reject only when many certificates share the minimum.\n",
        nref + 1,
        1.0 / (nref + 1) as f64,
        batch.len()
    );
}

// ---------------------------------------------------------------- (d) short-run SPC

fn first_alarm_imr(xs: &[f64], phase1: usize, we: bool) -> Option<usize> {
    let c = individuals_moving_range(&xs[..phase1]).ok()?;
    let (center, sigma) = (c.individuals.center, c.sigma_within);
    if sigma <= 0.0 {
        return None;
    }
    let rest = &xs[phase1..];
    let v = check_rules(
        rest,
        center,
        sigma,
        if we { RuleSet::WesternElectric } else { RuleSet::Nelson },
    )
    .ok()?;
    v.iter()
        .filter(|x| we || x.rule == "N1")
        .map(|x| x.index + phase1)
        .min()
}

fn first_alarm_q(xs: &[f64], we: bool) -> Option<usize> {
    let q = q_chart(xs).ok()?;
    let pts: Vec<f64> = q.iter().map(|p| p.q).collect();
    let v = check_rules(
        &pts,
        0.0,
        1.0,
        if we { RuleSet::WesternElectric } else { RuleSet::Nelson },
    )
    .ok()?;
    v.iter()
        .filter(|x| we || x.rule == "N1")
        .map(|x| q[x.index].index)
        .min()
}

fn first_alarm_bocpd(xs: &[f64]) -> Option<usize> {
    let s5 = quality_stats::describe::summary(&xs[..5]).ok()?.sd().max(1e-9);
    let st = bocpd(xs, NigPrior::weak(xs[0], s5), 100.0, 5, 200).ok()?;
    st.iter()
        .enumerate()
        .skip(5)
        .find(|(_, s)| s.p_recent > 0.5)
        .map(|(i, _)| i)
}

fn section_d() {
    const REPS: usize = 4000;
    const N: usize = 30;
    const SHIFT_AT: usize = 15;
    println!("## (d) Short-run SPC: Q-chart and BOCPD vs I-MR with short Phase I\n");
    println!(
        "{REPS} series of {N} individual values per row, unknown μ (uniform 0–100) and σ (uniform 0.01–5), mean shift of δσ at value {} (index {SHIFT_AT}). \
**I-MR** = limits from the first 10 values (the most a short run allows), then rule 1 (beyond 3σ) or all Western Electric rules on values 11–30. \
**Q-chart** = Quesenberry μ, σ unknown, signals from value 3. **BOCPD** = Adams–MacKay, hazard 1/100, prior σ guess = sd of the first 5 values, alarm when P(run length < 5) > 0.5 from value 6. \
Detected = first alarm at or after the shift; false alarm (δ = 0) = any alarm.\n",
        SHIFT_AT + 1
    );
    println!("| δ | I-MR rule 1 | I-MR WE | Q rule 1 | Q WE | BOCPD | median delay I-MR WE / Q WE / BOCPD |");
    println!("|---|---|---|---|---|---|---|");
    for delta in [0.0, 1.0, 2.0, 3.0] {
        let mut rng = Rng::new(SEED ^ (delta * 10.0) as u64 ^ 0xD);
        let mut hits = [0usize; 5];
        let mut delays: [Vec<usize>; 3] = [vec![], vec![], vec![]];
        for _ in 0..REPS {
            let (mu, sd) = (rng.uniform() * 100.0, 0.01 + rng.uniform() * 5.0);
            let xs: Vec<f64> = (0..N)
                .map(|i| mu + sd * (rng.normal() + if i >= SHIFT_AT { delta } else { 0.0 }))
                .collect();
            let alarms = [
                first_alarm_imr(&xs, 10, false),
                first_alarm_imr(&xs, 10, true),
                first_alarm_q(&xs, false),
                first_alarm_q(&xs, true),
                first_alarm_bocpd(&xs),
            ];
            for (j, a) in alarms.iter().enumerate() {
                let hit = if delta == 0.0 {
                    a.is_some()
                } else {
                    a.is_some_and(|i| i >= SHIFT_AT)
                };
                hits[j] += usize::from(hit);
            }
            if delta > 0.0 {
                for (k, j) in [1usize, 3, 4].iter().enumerate() {
                    if let Some(i) = alarms[*j].filter(|i| *i >= SHIFT_AT) {
                        delays[k].push(i - SHIFT_AT);
                    }
                }
            }
        }
        let med = |v: &mut Vec<usize>| {
            v.sort_unstable();
            v.get(v.len() / 2).map_or("–".to_string(), |x| x.to_string())
        };
        let r = |k: usize| hits[k] as f64 / REPS as f64;
        println!(
            "| {delta} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {} / {} / {} |",
            r(0),
            r(1),
            r(2),
            r(3),
            r(4),
            med(&mut delays[0]),
            med(&mut delays[1]),
            med(&mut delays[2]),
        );
    }
    // DNOM: 6 part numbers × 5 pieces, one process (σ = 0.02), shift 1.5σ from the 4th part on.
    println!(
        "\nDNOM: 6 part numbers × 5 pieces (nominals 10, 25, 5, 40, 12, 8 mm), common σ = 0.02, mean shift 1.5σ from part 4 (value 16). \
Per-part charts are impossible (5 values per part). DNOM deviations → Q-chart, all WE rules:\n"
    );
    println!("| case | alarm rate (any alarm) | detected at/after the shift |");
    println!("|---|---|---|");
    for shift in [0.0, 1.5, 3.0] {
        let mut rng = Rng::new(SEED ^ 0xD0 ^ (shift * 10.0) as u64);
        let (mut any, mut det) = (0, 0);
        for _ in 0..REPS {
            let mut pts = Vec::new();
            for (p, nom) in [10.0, 25.0, 5.0, 40.0, 12.0, 8.0].iter().enumerate() {
                for _ in 0..5 {
                    let d = if p >= 3 { shift * 0.02 } else { 0.0 };
                    pts.push((record(nom + d + 0.02 * rng.normal(), 3).to_f64(), *nom));
                }
            }
            let dev = dnom(&pts).expect("finite");
            let a = first_alarm_q(&dev, true);
            any += usize::from(a.is_some());
            det += usize::from(a.is_some_and(|i| i >= 15));
        }
        println!(
            "| shift {shift}σ | {:.3} | {:.3} |",
            any as f64 / REPS as f64,
            det as f64 / REPS as f64
        );
    }
    println!();
}

fn main() {
    let which = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    println!("# MATH v2 evaluation (seed {SEED})\n");
    if which == "all" || which == "a" {
        section_a(100);
        section_a(400);
    }
    if which == "all" || which == "d" {
        section_d();
    }
    if which == "all" || which == "e" {
        section_e();
    }
}

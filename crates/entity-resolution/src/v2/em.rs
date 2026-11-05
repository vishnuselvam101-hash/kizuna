//! Fellegi–Sunter model: per-field agreement-level probabilities m (matches) and u (non-matches), estimated by EM
//! on unlabelled comparison vectors under conditional independence. Missing fields contribute nothing.

use serde::Serialize;
use std::collections::BTreeMap;

/// Comparison vector: one agreement level per field (`None` = field missing on either side).
pub type Gamma = Vec<Option<u8>>;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FsParams {
    /// Share of matches among the training pairs.
    pub p: f64,
    pub m: Vec<Vec<f64>>,
    pub u: Vec<Vec<f64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EmReport {
    pub training_pairs: u64,
    pub distinct_patterns: usize,
    pub iterations: usize,
    pub converged: bool,
    /// Log-likelihood never decreased by more than numerical noise.
    pub monotone: bool,
    pub log_likelihood: Option<f64>,
    /// "estimated" when the EM result passed every check, otherwise "default" (prior parameters).
    pub used: &'static str,
    pub fallback_reason: Option<&'static str>,
}

const FLOOR: f64 = 1e-6;

fn normalise(v: &mut [f64]) {
    for x in v.iter_mut() {
        *x = x.max(FLOOR);
    }
    let s: f64 = v.iter().sum();
    for x in v.iter_mut() {
        *x /= s;
    }
}

fn class_lik(probs: &[Vec<f64>], g: &Gamma) -> f64 {
    g.iter()
        .zip(probs)
        .filter_map(|(l, p)| l.map(|l| p[l as usize]))
        .product()
}

fn log_likelihood(par: &FsParams, patterns: &BTreeMap<Gamma, u64>) -> f64 {
    patterns
        .iter()
        .map(|(g, n)| *n as f64 * (par.p * class_lik(&par.m, g) + (1.0 - par.p) * class_lik(&par.u, g)).ln())
        .sum()
}

/// EM over pattern counts. Deterministic: patterns are iterated in `BTreeMap` order. With `fixed_u`, u is held at
/// the given values (estimated from random pairs, which are almost all non-matches) and EM fits only p and m;
/// this stops the u class from absorbing structured non-matches such as "same place, different business".
/// `p_max` caps the match share (a cardinality bound: a record has only a few true duplicates); the capped
/// M-step is still the constrained maximiser, so the likelihood stays monotone.
pub fn estimate(
    patterns: &BTreeMap<Gamma, u64>,
    init: &FsParams,
    max_iter: usize,
    tol: f64,
    min_pairs: u64,
    fixed_u: Option<&[Vec<f64>]>,
    p_max: f64,
) -> (FsParams, EmReport) {
    let total: u64 = patterns.values().sum();
    let mut rep = EmReport {
        training_pairs: total,
        distinct_patterns: patterns.len(),
        iterations: 0,
        converged: false,
        monotone: true,
        log_likelihood: None,
        used: "default",
        fallback_reason: None,
    };
    if total < min_pairs || patterns.len() < 2 {
        rep.fallback_reason = Some("too_few_training_pairs");
        return (init.clone(), rep);
    }
    let mut par = init.clone();
    par.p = par.p.min(p_max);
    if let Some(u) = fixed_u {
        par.u = u.to_vec();
    }
    let mut ll = log_likelihood(&par, patterns);
    for it in 1..=max_iter {
        // E-step: posterior match probability per pattern; M-step: weighted level frequencies.
        let mut mc: Vec<Vec<f64>> = par.m.iter().map(|v| vec![0.0; v.len()]).collect();
        let mut uc = mc.clone();
        let mut msum = 0.0;
        for (g, n) in patterns {
            let (a, b) = (par.p * class_lik(&par.m, g), (1.0 - par.p) * class_lik(&par.u, g));
            let w = a / (a + b);
            let n = *n as f64;
            msum += n * w;
            for (f, l) in g.iter().enumerate() {
                if let Some(l) = l {
                    mc[f][*l as usize] += n * w;
                    uc[f][*l as usize] += n * (1.0 - w);
                }
            }
        }
        let mut next = FsParams {
            p: (msum / total as f64).clamp(FLOOR, p_max.clamp(FLOOR, 1.0 - FLOOR)),
            m: mc,
            u: uc,
        };
        next.m.iter_mut().for_each(|v| normalise(v));
        next.u.iter_mut().for_each(|v| normalise(v));
        if let Some(u) = fixed_u {
            next.u = u.to_vec();
        }
        let nll = log_likelihood(&next, patterns);
        // The probability floor can cost a hair of likelihood; anything larger is a real defect.
        if nll < ll - 1e-6 * ll.abs().max(1.0) {
            rep.monotone = false;
        }
        let delta = (nll - ll).abs();
        par = next;
        ll = nll;
        rep.iterations = it;
        if delta <= tol * ll.abs().max(1.0) {
            rep.converged = true;
            break;
        }
    }
    rep.log_likelihood = Some(ll);
    // Label switching: the "match" class must be the one that agrees exactly on the first field more often.
    if fixed_u.is_none() && par.m[0][0] < par.u[0][0] {
        std::mem::swap(&mut par.m, &mut par.u);
        par.p = 1.0 - par.p;
    }
    rep.fallback_reason = if !rep.converged {
        Some("not_converged")
    } else if !rep.monotone {
        Some("likelihood_decreased")
    } else if !(1e-5..=0.95).contains(&par.p) {
        Some("degenerate_match_share")
    } else if par.m[0][0] <= par.u[0][0] * 2.0 {
        Some("classes_not_separated")
    } else {
        None
    };
    if rep.fallback_reason.is_some() {
        return (init.clone(), rep);
    }
    rep.used = "estimated";
    (par, rep)
}

/// Match weight log2(m(γ)/u(γ)); missing fields contribute 0.
pub fn weight(par: &FsParams, g: &Gamma) -> f64 {
    g.iter()
        .enumerate()
        .filter_map(|(f, l)| l.map(|l| (par.m[f][l as usize] / par.u[f][l as usize]).log2()))
        .sum()
}

/// Classic Fellegi–Sunter thresholds from the fitted model over all complete patterns: the upper threshold keeps
/// the expected false-match share among pairs above it ≤ `mu`; the lower keeps the expected share of matches
/// falling below it ≤ `lambda`. Returns (t_match, t_possible) with t_possible ≤ t_match.
pub fn thresholds(par: &FsParams, mu: f64, lambda: f64) -> (f64, f64) {
    let mut pats: Vec<(f64, f64, f64)> = vec![(0.0, 1.0, 1.0)]; // (weight, m(γ), u(γ))
    for f in 0..par.m.len() {
        let mut next = Vec::with_capacity(pats.len() * par.m[f].len());
        for (w, m, u) in &pats {
            for l in 0..par.m[f].len() {
                let (ml, ul) = (par.m[f][l], par.u[f][l]);
                next.push((w + (ml / ul).log2(), m * ml, u * ul));
            }
        }
        pats = next;
    }
    pats.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut t_match = f64::INFINITY;
    let mut cu = 0.0;
    for (w, _, u) in &pats {
        cu += u;
        if cu > mu {
            break;
        }
        t_match = *w;
    }
    let mut t_possible = f64::NEG_INFINITY;
    let mut cm = 0.0;
    for (w, m, _) in pats.iter().rev() {
        cm += m;
        if cm > lambda {
            t_possible = *w;
            break;
        }
    }
    // Never call a pair with a negative weight (evidence against) a possible match.
    let t_possible = t_possible.max(0.0);
    (t_match.max(t_possible), t_possible)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth(seed: u64) -> BTreeMap<Gamma, u64> {
        // Three fields (EM needs >= 3 for identifiability); true m=[.8,.15,.05]/[.9,.1]/[.85,.15],
        // u=[.02,.08,.9]/[.3,.7]/[.25,.75], p=.1.
        let mut rng = super::super::Rng::new(seed);
        let mut out = BTreeMap::new();
        let pick = |rng: &mut super::super::Rng, p: &[f64]| {
            let x = rng.unit();
            let mut c = 0.0;
            for (i, v) in p.iter().enumerate() {
                c += v;
                if x < c {
                    return i as u8;
                }
            }
            (p.len() - 1) as u8
        };
        for _ in 0..20_000 {
            let is_m = rng.unit() < 0.1;
            let (a, b, c): (&[f64], &[f64], &[f64]) = if is_m {
                (&[0.8, 0.15, 0.05], &[0.9, 0.1], &[0.85, 0.15])
            } else {
                (&[0.02, 0.08, 0.9], &[0.3, 0.7], &[0.25, 0.75])
            };
            let g = vec![
                Some(pick(&mut rng, a)),
                Some(pick(&mut rng, b)),
                Some(pick(&mut rng, c)),
            ];
            *out.entry(g).or_insert(0) += 1;
        }
        out
    }

    fn init() -> FsParams {
        FsParams {
            p: 0.3,
            m: vec![vec![0.6, 0.3, 0.1], vec![0.7, 0.3], vec![0.7, 0.3]],
            u: vec![vec![0.1, 0.3, 0.6], vec![0.4, 0.6], vec![0.4, 0.6]],
        }
    }

    #[test]
    fn em_recovers_known_parameters_and_converges() {
        let (par, rep) = estimate(&synth(7), &init(), 1000, 1e-10, 50, None, 1.0);
        assert_eq!(rep.used, "estimated", "{rep:?}");
        assert!(rep.converged && rep.monotone, "{rep:?}");
        assert!((par.p - 0.1).abs() < 0.02, "{par:?}");
        assert!(
            (par.m[0][0] - 0.8).abs() < 0.05 && (par.u[0][2] - 0.9).abs() < 0.03,
            "{par:?}"
        );
    }

    #[test]
    fn em_is_deterministic_and_falls_back_on_tiny_input() {
        let a = estimate(&synth(3), &init(), 1000, 1e-10, 50, None, 1.0);
        let b = estimate(&synth(3), &init(), 1000, 1e-10, 50, None, 1.0);
        assert_eq!(a, b);
        let tiny: BTreeMap<Gamma, u64> = [(vec![Some(0), Some(0), Some(0)], 3)].into_iter().collect();
        let (par, rep) = estimate(&tiny, &init(), 1000, 1e-10, 50, None, 1.0);
        assert_eq!((rep.used, par), ("default", init()));
    }

    #[test]
    fn fixed_u_is_kept_and_m_still_recovered() {
        let u = vec![vec![0.02, 0.08, 0.9], vec![0.3, 0.7], vec![0.25, 0.75]];
        let (par, rep) = estimate(&synth(7), &init(), 1000, 1e-10, 50, Some(&u), 1.0);
        assert_eq!(rep.used, "estimated", "{rep:?}");
        assert_eq!(par.u, u);
        assert!(
            (par.p - 0.1).abs() < 0.02 && (par.m[0][0] - 0.8).abs() < 0.05,
            "{par:?}"
        );
    }

    #[test]
    fn match_share_cap_is_respected() {
        let (par, rep) = estimate(&synth(7), &init(), 1000, 1e-10, 50, None, 0.05);
        assert!(rep.monotone, "{rep:?}");
        assert!(par.p <= 0.05 + 1e-12, "{par:?}");
    }

    #[test]
    fn label_switching_is_corrected() {
        let mut swapped = init();
        std::mem::swap(&mut swapped.m, &mut swapped.u);
        swapped.p = 0.7;
        let (par, rep) = estimate(&synth(7), &swapped, 1000, 1e-10, 50, None, 1.0);
        assert_eq!(rep.used, "estimated", "{rep:?}");
        assert!(par.m[0][0] > par.u[0][0] && par.p < 0.5, "{par:?}");
    }

    #[test]
    fn thresholds_are_ordered_and_weights_symmetric_in_missing() {
        let (par, _) = estimate(&synth(7), &init(), 1000, 1e-10, 50, None, 1.0);
        let (tm, tp) = thresholds(&par, 0.01, 0.01);
        assert!(tm.is_finite());
        assert!(tm >= tp && tp >= 0.0, "{tm} {tp}");
        assert_eq!(weight(&par, &vec![None, None, None]), 0.0);
        assert!(weight(&par, &vec![Some(0), Some(0), Some(0)]) > tm - 1e-9);
        assert!(weight(&par, &vec![Some(2), Some(1), Some(1)]) < tp);
    }
}

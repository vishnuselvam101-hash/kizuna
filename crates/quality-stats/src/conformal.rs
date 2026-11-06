//! Split-conformal review gate with a coverage guarantee (MATH v2 (b)).
//!
//! The review gate flags a mapped value for human confirmation when its score (e.g. the calibrated probability
//! that it is correct) is at or below a threshold. We want the gate to catch at least 1 − α of the *wrong*
//! values. Class-conditional ("Mondrian") split conformal prediction (Vovk et al. 2005, ch. 4; Angelopoulos &
//! Bates 2023, §1–2) gives this with a finite-sample guarantee:
//!
//! Take the scores s₁…sₙ of the wrong values in a calibration set and let k = ⌈(1 − α)(n + 1)⌉. If k ≤ n the
//! threshold is the k-th smallest score; otherwise no finite threshold suffices and every value is flagged. For a
//! new wrong value whose score is exchangeable with the calibration wrong values,
//! P(s_new ≤ threshold) ≥ 1 − α (and ≤ 1 − α + 1/(n + 1) when there are no ties).
//!
//! The guarantee is marginal (on average over calibration sets) and needs exchangeability: a new document layout
//! or OCR engine is *not* exchangeable with the calibration data, so the coverage on such data must be measured,
//! not assumed. The gate only asks a person to look; it never accepts or rejects a value on its own.

use crate::error::{StatsError, StatsResult, check_finite};

/// A fitted lower-tail gate: flag a value when `score ≤ threshold`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConformalGate {
    /// `None` = flag everything (too few calibration positives for the requested α).
    pub threshold: Option<f64>,
    pub alpha: f64,
    /// Calibration positives (wrong values) used.
    pub n: usize,
    /// Rank of the threshold among the sorted calibration scores (1-based), ⌈(1 − α)(n + 1)⌉.
    pub k: usize,
}

impl ConformalGate {
    /// Fit on the scores of the calibration positives (values that should be flagged). α in (0, 1).
    pub fn fit(positive_scores: &[f64], alpha: f64) -> StatsResult<Self> {
        check_finite(positive_scores, "scores")?;
        if !(alpha > 0.0 && alpha < 1.0) {
            return Err(StatsError::InvalidParameter("alpha must be in (0, 1)"));
        }
        let n = positive_scores.len();
        // ⌈(1 − α)(n + 1)⌉ with a guard against 0.9·10 = 9.000000000000002.
        let k = (((1.0 - alpha) * (n + 1) as f64) - 1e-9).ceil().max(1.0) as usize;
        let threshold = if k <= n {
            let mut s = positive_scores.to_vec();
            s.sort_by(f64::total_cmp);
            Some(s[k - 1])
        } else {
            None
        };
        Ok(ConformalGate { threshold, alpha, n, k })
    }

    /// Whether a value with this score is flagged for review.
    pub fn flags(&self, score: f64) -> bool {
        self.threshold.is_none_or(|t| score <= t)
    }

    /// The smallest number of calibration positives that allows a finite threshold: ⌈1/α⌉ − 1.
    pub fn min_positives(alpha: f64) -> usize {
        ((1.0 / alpha) - 1e-9).ceil() as usize - 1
    }
}

/// Share of `scores` the gate flags (coverage when `scores` are positives, review load when they are all values).
pub fn flagged_share(gate: &ConformalGate, scores: &[f64]) -> Option<f64> {
    if scores.is_empty() {
        return None;
    }
    Some(scores.iter().filter(|s| gate.flags(**s)).count() as f64 / scores.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::Rng;

    #[test]
    fn hand_computed() {
        // n = 9, α = 0.1 → k = ⌈0.9·10⌉ = 9 → threshold = the largest score.
        let s: Vec<f64> = (1..=9).map(f64::from).collect();
        let g = ConformalGate::fit(&s, 0.1).unwrap();
        assert_eq!((g.k, g.threshold), (9, Some(9.0)));
        // n = 19, α = 0.1 → k = 18.
        let s: Vec<f64> = (1..=19).map(f64::from).collect();
        assert_eq!(ConformalGate::fit(&s, 0.1).unwrap().threshold, Some(18.0));
        // n = 8 < 9 → flag everything.
        let g = ConformalGate::fit(&s[..8], 0.1).unwrap();
        assert_eq!(g.threshold, None);
        assert!(g.flags(1e9));
        assert_eq!(ConformalGate::min_positives(0.1), 9);
        assert_eq!(ConformalGate::min_positives(0.05), 19);
        assert!(ConformalGate::fit(&s, 0.0).is_err());
        assert!(ConformalGate::fit(&[f64::NAN], 0.1).is_err());
    }

    #[test]
    fn marginal_coverage_holds_on_exchangeable_data() {
        // 4 000 random calibration/test splits of skewed, tied scores: mean coverage ≥ 1 − α.
        let mut r = Rng::new(11);
        for alpha in [0.1, 0.2] {
            let mut cov = 0.0;
            let reps = 4000;
            for _ in 0..reps {
                let draw = |r: &mut Rng| ((r.uniform() * r.uniform()) * 20.0).round() / 20.0;
                let cal: Vec<f64> = (0..30).map(|_| draw(&mut r)).collect();
                let g = ConformalGate::fit(&cal, alpha).unwrap();
                cov += f64::from(u8::from(g.flags(draw(&mut r))));
            }
            let cov = cov / reps as f64;
            // Mean of 4 000 Bernoulli(≥ 0.9): allow 3 standard errors below.
            assert!(
                cov >= 1.0 - alpha - 3.0 * (alpha * (1.0 - alpha) / reps as f64).sqrt(),
                "{alpha}: {cov}"
            );
        }
    }

    proptest::proptest! {
        #[test]
        fn calibration_coverage_and_monotonicity(s in proptest::collection::vec(-5f64..5.0, 0..80), a in 0.01f64..0.5) {
            let g = ConformalGate::fit(&s, a).unwrap();
            // On its own calibration set the gate flags at least k of n.
            if let Some(t) = g.threshold {
                proptest::prop_assert!(s.iter().filter(|x| **x <= t).count() >= g.k);
                proptest::prop_assert!(s.len() >= ConformalGate::min_positives(a));
            } else {
                proptest::prop_assert!(s.len() < g.k);
            }
            // A smaller α (stronger guarantee) never lowers the threshold.
            let g2 = ConformalGate::fit(&s, a / 2.0).unwrap();
            match (g.threshold, g2.threshold) {
                (Some(t1), Some(t2)) => proptest::prop_assert!(t2 >= t1),
                (Some(_), None) | (None, None) => {}
                (None, Some(_)) => proptest::prop_assert!(false, "smaller α gave a finite threshold"),
            }
            // Order of the input does not matter.
            let mut rev = s.clone();
            rev.reverse();
            proptest::prop_assert_eq!(ConformalGate::fit(&rev, a).unwrap(), g);
        }
    }
}

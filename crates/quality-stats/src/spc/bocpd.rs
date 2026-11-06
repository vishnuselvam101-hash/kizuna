//! Bayesian online change-point detection (R. P. Adams & D. J. C. MacKay, "Bayesian online changepoint
//! detection", arXiv:0710.3742, 2007) for a Gaussian process with unknown mean and variance.
//!
//! The run length r_t (values since the last change) has a posterior that is updated in O(r) per value:
//! growth P(r_t = r + 1) ∝ P(r_{t−1} = r)·π(x_t | r)·(1 − H), change P(r_t = 0) ∝ Σ_r P(r_{t−1} = r)·π(x_t | r)·H,
//! with constant hazard H = 1/λ (expected run length λ). Each run length keeps a Normal–inverse-gamma posterior
//! (μ, κ, α, β); its predictive π is Student t with 2α df, location μ and scale² = β(κ + 1)/(ακ) (Murphy 2007,
//! "Conjugate Bayesian analysis of the Gaussian distribution", §6). Run lengths beyond `max_run` are dropped and
//! the rest renormalised (the truncation is documented in the output).
//!
//! With this recursion r_t = 0 means "the next value starts a new segment", so a change shows up as mass on small
//! run lengths a few values after it. [`BocpdStep::p_recent`] = P(r_t < window) is the signal; it is ≥ the prior
//! mass on short runs at the start of a series, so callers ignore the first `window` steps.

use crate::error::{StatsError, StatsResult, check_finite};
use crate::special::ln_gamma;

/// Normal–inverse-gamma prior for each segment's mean and variance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NigPrior {
    pub mu0: f64,
    pub kappa0: f64,
    pub alpha0: f64,
    pub beta0: f64,
}

impl NigPrior {
    /// Weak prior centred on `mu0` with prior σ guess `sigma` (α0 = 1, β0 = σ², κ0 = 1).
    pub fn weak(mu0: f64, sigma: f64) -> Self {
        NigPrior {
            mu0,
            kappa0: 1.0,
            alpha0: 1.0,
            beta0: sigma * sigma,
        }
    }
}

/// Posterior summary after one value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BocpdStep {
    /// P(r_t < window | x_1..x_t).
    pub p_recent: f64,
    /// Most probable run length.
    pub map_run: usize,
}

#[derive(Clone, Copy)]
struct Nig {
    mu: f64,
    kappa: f64,
    alpha: f64,
    beta: f64,
}

impl Nig {
    fn ln_pred(&self, x: f64) -> f64 {
        let nu = 2.0 * self.alpha;
        let s2 = self.beta * (self.kappa + 1.0) / (self.alpha * self.kappa);
        let z = (x - self.mu) * (x - self.mu) / (nu * s2);
        ln_gamma((nu + 1.0) / 2.0)
            - ln_gamma(nu / 2.0)
            - 0.5 * (nu * std::f64::consts::PI * s2).ln()
            - (nu + 1.0) / 2.0 * z.ln_1p()
    }

    fn update(&self, x: f64) -> Nig {
        let k = self.kappa + 1.0;
        Nig {
            mu: (self.kappa * self.mu + x) / k,
            kappa: k,
            alpha: self.alpha + 0.5,
            beta: self.beta + self.kappa * (x - self.mu) * (x - self.mu) / (2.0 * k),
        }
    }
}

/// Runs BOCPD over `xs`. `hazard_lambda` > 1 (expected run length), `window` ≥ 1, `max_run` ≥ `window`.
pub fn bocpd(
    xs: &[f64],
    prior: NigPrior,
    hazard_lambda: f64,
    window: usize,
    max_run: usize,
) -> StatsResult<Vec<BocpdStep>> {
    check_finite(xs, "values")?;
    let p = [prior.mu0, prior.kappa0, prior.alpha0, prior.beta0, hazard_lambda];
    check_finite(&p, "prior / hazard")?;
    if prior.kappa0 <= 0.0 || prior.alpha0 <= 0.0 || prior.beta0 <= 0.0 {
        return Err(StatsError::InvalidParameter("kappa0, alpha0, beta0 must be > 0"));
    }
    if hazard_lambda <= 1.0 || window == 0 || max_run < window {
        return Err(StatsError::InvalidParameter(
            "hazard_lambda > 1, window ≥ 1, max_run ≥ window",
        ));
    }
    let h = 1.0 / hazard_lambda;
    let base = Nig {
        mu: prior.mu0,
        kappa: prior.kappa0,
        alpha: prior.alpha0,
        beta: prior.beta0,
    };
    let mut probs = vec![1.0_f64];
    let mut params = vec![base];
    let mut out = Vec::with_capacity(xs.len());
    for &x in xs {
        // Work in logs relative to the max predictive to avoid underflow on outliers.
        let lp: Vec<f64> = params.iter().map(|q| q.ln_pred(x)).collect();
        let mx = lp.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let pred: Vec<f64> = lp.iter().map(|l| (l - mx).exp()).collect();
        let mut next = Vec::with_capacity((probs.len() + 1).min(max_run + 1));
        let cp: f64 = probs.iter().zip(&pred).map(|(r, p)| r * p * h).sum();
        next.push(cp);
        next.extend(probs.iter().zip(&pred).map(|(r, p)| r * p * (1.0 - h)));
        let mut np = Vec::with_capacity(next.len());
        np.push(base);
        np.extend(params.iter().map(|q| q.update(x)));
        next.truncate(max_run + 1);
        np.truncate(max_run + 1);
        let total: f64 = next.iter().sum();
        if total > 0.0 && total.is_finite() {
            next.iter_mut().for_each(|v| *v /= total);
        } else {
            // Every run length gave the value zero predictive density (cannot happen with Student t, kept for safety).
            next = vec![0.0; next.len()];
            next[0] = 1.0;
        }
        let map_run = next
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(&a.0)))
            .map_or(0, |(i, _)| i);
        out.push(BocpdStep {
            p_recent: next.iter().take(window).sum::<f64>().min(1.0),
            map_run,
        });
        probs = next;
        params = np;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::Rng;

    #[test]
    fn detects_a_clear_shift_and_stays_quiet_without_one() {
        let mut r = Rng::new(5);
        let mut xs: Vec<f64> = (0..40).map(|_| 10.0 + 0.1 * r.normal()).collect();
        xs.extend((0..20).map(|_| 10.5 + 0.1 * r.normal()));
        let steps = bocpd(&xs, NigPrior::weak(10.0, 0.1), 100.0, 5, 500).unwrap();
        assert_eq!(steps.len(), 60);
        // Quiet before the change (after the warm-up window).
        assert!(steps[5..40].iter().all(|s| s.p_recent < 0.5), "{:?}", &steps[5..40]);
        // Detected within 3 values after it, and the run length restarts.
        assert!(steps[40..43].iter().any(|s| s.p_recent > 0.5), "{:?}", &steps[38..45]);
        assert!(steps[59].map_run < 25, "{:?}", steps[59]);
    }

    #[test]
    fn rejects_bad_parameters() {
        let p = NigPrior::weak(0.0, 1.0);
        assert!(bocpd(&[1.0], p, 1.0, 5, 10).is_err());
        assert!(bocpd(&[1.0], p, 10.0, 0, 10).is_err());
        assert!(bocpd(&[1.0], p, 10.0, 5, 4).is_err());
        assert!(bocpd(&[f64::NAN], p, 10.0, 5, 10).is_err());
        assert!(bocpd(&[1.0], NigPrior { beta0: 0.0, ..p }, 10.0, 5, 10).is_err());
        assert!(bocpd(&[], p, 10.0, 5, 10).unwrap().is_empty());
    }

    proptest::proptest! {
        #[test]
        fn probabilities_are_valid_and_deterministic(xs in proptest::collection::vec(-1e3f64..1e3, 1..60), w in 1usize..8) {
            let p = NigPrior::weak(0.0, 10.0);
            let a = bocpd(&xs, p, 50.0, w, 30).unwrap();
            let b = bocpd(&xs, p, 50.0, w, 30).unwrap();
            proptest::prop_assert_eq!(&a, &b);
            for (t, s) in a.iter().enumerate() {
                proptest::prop_assert!((0.0..=1.0 + 1e-12).contains(&s.p_recent));
                proptest::prop_assert!(s.map_run <= (t + 1).min(30));
            }
        }
    }
}

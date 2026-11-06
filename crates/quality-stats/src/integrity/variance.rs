//! "Too good to be true": is the sample variance smaller than the stated measurement method allows?
//!
//! Repeated measurements of one item (or of a stable reference) cannot scatter less than the gauge/method
//! repeatability σ0 (from the method standard, a gauge R&R study or the calibration certificate). Under H0 the
//! values are N(μ, σ_ref²) and (n − 1)s²/σ_ref² ~ χ²_{n−1} (Montgomery §3.3; ISO 5725-2 precision), so the one-sided
//! lower-tail p = P(χ²_{n−1} ≤ (n − 1)s²/σ_ref²) is small when values are suspiciously consistent (copied, smoothed
//! or invented around a target).
//!
//! Recording resolution h adds about h²/12 of rounding variance (Sheppard's correction), so σ_ref² = σ0² + h²/12.
//! When h > σ0 rounding dominates and identical values are expected, so the test refuses.

use crate::describe::summary;
use crate::error::{StatsError, StatsResult, check_finite, need};
use crate::special::{chi2_cdf, f_cdf};
use crate::{Applicability, TestResult};

/// Lower-tail χ² variance test. `statistic` = (n − 1)s²/σ_ref², df = n − 1. Needs n ≥ 2, σ0 > 0, h ≥ 0.
pub fn variance_too_low(values: &[f64], stated_sigma: f64, resolution: f64) -> StatsResult<Applicability<TestResult>> {
    need(values.len(), 2)?;
    check_finite(values, "values")?;
    if !stated_sigma.is_finite() || !resolution.is_finite() {
        return Err(StatsError::NonFinite("stated sigma / resolution"));
    }
    if stated_sigma <= 0.0 || resolution < 0.0 {
        return Err(StatsError::InvalidParameter(
            "stated sigma must be > 0 and resolution >= 0",
        ));
    }
    if resolution > stated_sigma {
        return Ok(Applicability::NotApplicable(format!(
            "resolution {resolution} is coarser than the stated sigma {stated_sigma}; identical rounded values are expected"
        )));
    }
    let s = summary(values)?;
    let var_ref = stated_sigma * stated_sigma + resolution * resolution / 12.0;
    let df = (s.n - 1) as f64;
    let stat = df * s.variance / var_ref;
    let p = chi2_cdf(stat, df);
    Ok(Applicability::Applicable(TestResult {
        statistic: stat,
        df,
        p_value: p,
        n: s.n,
        explanation: format!(
            "sample sd {:.4e} vs expected ≥ {:.4e} (stated sigma with rounding); ratio {:.3}; χ²({df}) = {stat:.3}, lower-tail p = {p:.3e}",
            s.sd(),
            var_ref.sqrt(),
            s.sd() / var_ref.sqrt()
        ),
    }))
}

/// One characteristic's repeated measurements and its stated method σ0 and recording resolution h.
#[derive(Debug, Clone, PartialEq)]
pub struct VarianceGroup {
    pub values: Vec<f64>,
    pub stated_sigma: f64,
    pub resolution: f64,
}

/// Pooled likelihood-ratio test that a certificate's characteristics are *jointly* too consistent.
///
/// Model: characteristic i has true σᵢ = λ·σ_ref,i with one common shrink factor λ (a person smoothing or
/// inventing numbers does it to the whole table). H0: λ = 1, H1: λ < 1. With Tᵢ = (nᵢ − 1)sᵢ²/σ_ref,i², the
/// likelihood ratio is monotone in T = ΣTᵢ ~ λ²·χ²_D, D = Σ(nᵢ − 1), so the uniformly most powerful test rejects
/// for small T: p = P(χ²_D ≤ T). With one characteristic this is exactly [`variance_too_low`] (which is already
/// UMP for a single sample); the gain comes from pooling. Groups whose resolution is coarser than σ0 or with fewer
/// than 2 values are skipped (listed in the explanation); if none remain the test does not apply.
pub fn variance_too_low_pooled(groups: &[VarianceGroup]) -> StatsResult<Applicability<TestResult>> {
    let (mut t, mut d, mut n, mut skipped) = (0.0, 0.0, 0usize, 0usize);
    for g in groups {
        if !g.stated_sigma.is_finite() || !g.resolution.is_finite() {
            return Err(StatsError::NonFinite("stated sigma / resolution"));
        }
        if g.stated_sigma <= 0.0 || g.resolution < 0.0 {
            return Err(StatsError::InvalidParameter(
                "stated sigma must be > 0 and resolution >= 0",
            ));
        }
        check_finite(&g.values, "values")?;
        if g.values.len() < 2 || g.resolution > g.stated_sigma {
            skipped += 1;
            continue;
        }
        let s = summary(&g.values)?;
        let var_ref = g.stated_sigma * g.stated_sigma + g.resolution * g.resolution / 12.0;
        t += (s.n - 1) as f64 * s.variance / var_ref;
        d += (s.n - 1) as f64;
        n += s.n;
    }
    if d == 0.0 {
        return Ok(Applicability::NotApplicable(format!(
            "no characteristic has ≥ 2 values with a resolution finer than its stated sigma ({skipped} skipped)"
        )));
    }
    let p = chi2_cdf(t, d);
    Ok(Applicability::Applicable(TestResult {
        statistic: t,
        df: d,
        p_value: p,
        n,
        explanation: format!(
            "pooled over {} characteristic(s) ({skipped} skipped): estimated common sd ratio {:.3}; χ²({d}) = {t:.3}, lower-tail p = {p:.3e}",
            groups.len() - skipped,
            (t / d).sqrt()
        ),
    }))
}

/// Variance-too-low test when σ0 itself was *estimated* (gauge R&R / method study with `nu` degrees of freedom).
///
/// Treating an estimated σ̂0 as exact makes [`variance_too_low`] reject honest data too often whenever σ̂0 came out
/// high. Under H0 (true σ = σ0), s²/σ̂0² ~ F(n − 1, ν) (independent samples), so p = P(F ≤ s²/σ̂_ref²) is exact.
/// Rounding variance h²/12 is added to σ̂0² as in [`variance_too_low`].
pub fn variance_too_low_estimated_sigma(
    values: &[f64],
    sigma_hat: f64,
    nu: f64,
    resolution: f64,
) -> StatsResult<Applicability<TestResult>> {
    if !nu.is_finite() || nu <= 0.0 {
        return Err(StatsError::InvalidParameter("nu must be > 0"));
    }
    let base = variance_too_low(values, sigma_hat, resolution)?;
    let Applicability::Applicable(r) = base else {
        return Ok(base);
    };
    let f = r.statistic / r.df;
    let p = f_cdf(f, r.df, nu);
    Ok(Applicability::Applicable(TestResult {
        statistic: f,
        df: r.df,
        p_value: p,
        n: r.n,
        explanation: format!(
            "variance ratio s²/σ̂_ref² = {f:.3} with σ̂0 from a study with {nu} df; F({}, {nu}) lower-tail p = {p:.3e}",
            r.df
        ),
    }))
}

/// Posterior summary of the common shrink factor λ = σ_true / σ_ref.
#[derive(Debug, Clone, PartialEq)]
pub struct ShrinkPosterior {
    /// Posterior median of λ.
    pub median: f64,
    /// P(λ < `below` | data).
    pub p_below: f64,
    pub below: f64,
    /// Bayes factor of H1 (λ < 1 under the prior restricted to λ < 1) against H0 (λ = 1).
    pub bayes_factor: f64,
}

/// Bayesian version of the pooled test: prior log-uniform on λ ∈ [0.05, 2], likelihood T | λ ~ λ²·χ²_D (from
/// [`variance_too_low_pooled`]'s T and D). Computed on a 4 000-point grid in log λ (deterministic). For a fixed D
/// the Bayes factor is monotone in T, so as a *test* it ranks certificates exactly like the pooled LR test; its value
/// is the interpretable output ("the table scatters like a process with half the stated σ, P(λ < 0.8) = 0.97").
pub fn shrink_posterior(t: f64, d: f64, below: f64) -> StatsResult<ShrinkPosterior> {
    if !(t.is_finite() && d.is_finite() && below.is_finite()) || t < 0.0 || d <= 0.0 || below <= 0.0 {
        return Err(StatsError::InvalidParameter("need T ≥ 0, D > 0, below > 0"));
    }
    const LO: f64 = 0.05;
    const HI: f64 = 2.0;
    const N: usize = 4000;
    // log-likelihood of λ up to a constant: −D ln λ − T / (2λ²).
    let ll = |l: f64| -d * l.ln() - t / (2.0 * l * l);
    let ll1 = ll(1.0);
    let (a, b) = (LO.ln(), HI.ln());
    let step = (b - a) / N as f64;
    let grid: Vec<(f64, f64)> = (0..N)
        .map(|i| {
            let l = (a + (i as f64 + 0.5) * step).exp();
            (l, ll(l))
        })
        .collect();
    let mx = grid.iter().map(|g| g.1).fold(f64::NEG_INFINITY, f64::max);
    let w: Vec<f64> = grid.iter().map(|g| (g.1 - mx).exp()).collect();
    let total: f64 = w.iter().sum();
    let mut acc = 0.0;
    let mut median = HI;
    for (g, wi) in grid.iter().zip(&w) {
        acc += wi;
        if acc >= total / 2.0 {
            median = g.0;
            break;
        }
    }
    let p_below = grid
        .iter()
        .zip(&w)
        .filter(|(g, _)| g.0 < below)
        .map(|(_, x)| x)
        .sum::<f64>()
        / total;
    // H1 prior: log-uniform on [LO, 1). BF = mean over that prior of L(λ)/L(1).
    let h1: Vec<f64> = grid.iter().filter(|g| g.0 < 1.0).map(|g| g.1).collect();
    let bf = if h1.is_empty() {
        0.0
    } else {
        let m1 = h1.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let mean = h1.iter().map(|x| (x - m1).exp()).sum::<f64>() / h1.len() as f64;
        (m1 - ll1 + mean.ln()).exp()
    };
    Ok(ShrinkPosterior {
        median,
        p_below,
        below,
        bayes_factor: bf,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grp(v: &[f64], s: f64) -> VarianceGroup {
        VarianceGroup {
            values: v.to_vec(),
            stated_sigma: s,
            resolution: 0.0,
        }
    }

    #[test]
    fn pooled_equals_single_for_one_group_and_adds_up() {
        let v = [9.9, 10.0, 10.1];
        let one = variance_too_low_pooled(&[grp(&v, 0.1)]).unwrap();
        let single = variance_too_low(&v, 0.1, 0.0).unwrap();
        assert!((one.applicable().unwrap().p_value - single.applicable().unwrap().p_value).abs() < 1e-15);
        // Two identical groups: T = 4, D = 4 → p = P(χ²₄ ≤ 4) = 1 − e^{−2}(1 + 2) = 0.593994.
        let two = variance_too_low_pooled(&[grp(&v, 0.1), grp(&v, 0.1)]).unwrap();
        let r = two.applicable().unwrap();
        assert!((r.statistic - 4.0).abs() < 1e-9 && r.df == 4.0);
        assert!((r.p_value - (1.0 - 3.0 * (-2.0f64).exp())).abs() < 1e-12);
        // Coarse-resolution groups are skipped; all skipped → not applicable.
        let coarse = VarianceGroup {
            values: v.to_vec(),
            stated_sigma: 0.1,
            resolution: 0.5,
        };
        assert!(
            variance_too_low_pooled(std::slice::from_ref(&coarse))
                .unwrap()
                .applicable()
                .is_none()
        );
        assert!(variance_too_low_pooled(&[]).unwrap().applicable().is_none());
        assert_eq!(
            variance_too_low_pooled(&[coarse, grp(&v, 0.1)])
                .unwrap()
                .applicable()
                .unwrap()
                .df,
            2.0
        );
        assert!(variance_too_low_pooled(&[grp(&v, 0.0)]).is_err());
    }

    #[test]
    fn estimated_sigma_test() {
        // F = 1 with df (2, 2): P(F ≤ 1) = 0.5 (F(d, d) has median 1).
        let r = variance_too_low_estimated_sigma(&[9.9, 10.0, 10.1], 0.1, 2.0, 0.0).unwrap();
        assert!((r.applicable().unwrap().p_value - 0.5).abs() < 1e-12);
        // ν → ∞ approaches the known-σ test.
        let a = variance_too_low_estimated_sigma(&[9.95, 10.0, 10.02, 9.99], 0.1, 1e7, 0.0).unwrap();
        let b = variance_too_low(&[9.95, 10.0, 10.02, 9.99], 0.1, 0.0).unwrap();
        assert!((a.applicable().unwrap().p_value - b.applicable().unwrap().p_value).abs() < 1e-4);
        assert!(variance_too_low_estimated_sigma(&[1.0, 2.0], 0.1, 0.0, 0.0).is_err());
    }

    #[test]
    fn posterior_tracks_the_data() {
        // T = D·0.25 is what λ = 0.5 produces on average: posterior median near 0.5, P(λ < 0.8) high, BF > 1.
        let p = shrink_posterior(45.0 * 0.25, 45.0, 0.8).unwrap();
        assert!((p.median - 0.5).abs() < 0.08, "{p:?}");
        assert!(p.p_below > 0.99 && p.bayes_factor > 100.0, "{p:?}");
        let h = shrink_posterior(45.0, 45.0, 0.8).unwrap();
        assert!(
            (h.median - 1.0).abs() < 0.15 && h.p_below < 0.05 && h.bayes_factor < 1.0,
            "{h:?}"
        );
        assert!(shrink_posterior(1.0, 0.0, 0.8).is_err());
    }

    proptest::proptest! {
        #[test]
        fn bayes_factor_is_monotone_in_t(d in 1u32..80, t in 0.01f64..200.0, dt in 0.0f64..50.0) {
            let a = shrink_posterior(t, f64::from(d), 0.8).unwrap();
            let b = shrink_posterior(t + dt, f64::from(d), 0.8).unwrap();
            proptest::prop_assert!(b.bayes_factor <= a.bayes_factor * (1.0 + 1e-9));
            proptest::prop_assert!(b.p_below <= a.p_below + 1e-9);
            proptest::prop_assert!((0.0..=1.0).contains(&a.p_below));
        }

        #[test]
        fn pooled_p_in_unit_interval(g in proptest::collection::vec((proptest::collection::vec(-5f64..5.0, 2..15), 0.01f64..3.0), 1..6)) {
            let groups: Vec<VarianceGroup> = g.iter().map(|(v, s)| grp(v, *s)).collect();
            let r = variance_too_low_pooled(&groups).unwrap();
            let r = r.applicable().unwrap();
            proptest::prop_assert!((0.0..=1.0).contains(&r.p_value));
            // Order of the characteristics does not matter.
            let mut rev = groups.clone();
            rev.reverse();
            let r2 = variance_too_low_pooled(&rev).unwrap();
            proptest::prop_assert!((r2.applicable().unwrap().p_value - r.p_value).abs() < 1e-12);
        }
    }

    #[test]
    fn hand_computed() {
        // Values 9.9, 10.0, 10.1: s² = 0.01; σ0 = 0.1, h = 0 → χ² = 2·0.01/0.01 = 2, df 2, p = 1 − e^{−1} = 0.632.
        let r = variance_too_low(&[9.9, 10.0, 10.1], 0.1, 0.0).unwrap();
        let r = r.applicable().unwrap();
        assert!((r.statistic - 2.0).abs() < 1e-9);
        assert!((r.p_value - (1.0 - (-1.0f64).exp())).abs() < 1e-9);
        // Identical values with fine resolution → p = 0.
        let z = variance_too_low(&[5.0; 10], 0.1, 0.01).unwrap();
        assert_eq!(z.applicable().unwrap().p_value, 0.0);
    }

    #[test]
    fn guards_and_errors() {
        assert!(variance_too_low(&[1.0], 0.1, 0.0).is_err());
        assert!(variance_too_low(&[1.0, 2.0], 0.0, 0.0).is_err());
        assert!(variance_too_low(&[1.0, f64::NAN], 0.1, 0.0).is_err());
        assert!(variance_too_low(&[1.0, 1.0], 0.1, 0.5).unwrap().applicable().is_none());
    }

    proptest::proptest! {
        #[test]
        fn p_in_unit_interval_and_monotone_in_sigma(v in proptest::collection::vec(-10f64..10.0, 2..50), s in 0.01f64..5.0) {
            let a = variance_too_low(&v, s, 0.0).unwrap();
            let b = variance_too_low(&v, s * 2.0, 0.0).unwrap();
            let (a, b) = (a.applicable().unwrap(), b.applicable().unwrap());
            proptest::prop_assert!((0.0..=1.0).contains(&a.p_value));
            // Claiming a larger method sigma makes the same data look more "too consistent".
            proptest::prop_assert!(b.p_value <= a.p_value + 1e-12);
        }
    }
}

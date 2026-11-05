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
use crate::special::chi2_cdf;
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

#[cfg(test)]
mod tests {
    use super::*;

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

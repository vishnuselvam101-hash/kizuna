//! Descriptive statistics in `f64`.
//!
//! Mean and variance use Welford's one-pass update (Welford 1962; Knuth TAOCP vol. 2 §4.2.2). Its relative error is
//! bounded by roughly n·ε·κ where ε = 2^-53 and κ = 1 + |mean|/sd is the condition number (Chan, Golub & LeVeque
//! 1983); for certificate data (n ≤ 10^6, κ ≤ 10^4) that is ≤ 1e-6 relative in the worst case and typically ~1e-12.

use crate::error::{StatsResult, check_finite, need};

/// Mean, sample variance (n − 1 denominator) and count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub n: usize,
    pub mean: f64,
    /// Sample variance s² with denominator n − 1 (0 when n = 1).
    pub variance: f64,
    pub min: f64,
    pub max: f64,
}

impl Summary {
    /// Sample standard deviation s.
    pub fn sd(&self) -> f64 {
        self.variance.sqrt()
    }
}

/// Summarise `xs` (n ≥ 1, all finite).
pub fn summary(xs: &[f64]) -> StatsResult<Summary> {
    need(xs.len(), 1)?;
    check_finite(xs, "values")?;
    let (mut mean, mut m2) = (0.0f64, 0.0f64);
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for (i, &x) in xs.iter().enumerate() {
        let delta = x - mean;
        mean += delta / (i + 1) as f64;
        m2 += delta * (x - mean);
        min = min.min(x);
        max = max.max(x);
    }
    let n = xs.len();
    let variance = if n > 1 { (m2 / (n - 1) as f64).max(0.0) } else { 0.0 };
    Ok(Summary {
        n,
        mean,
        variance,
        min,
        max,
    })
}

/// Median of finite values (n ≥ 1).
pub fn median(xs: &[f64]) -> StatsResult<f64> {
    need(xs.len(), 1)?;
    check_finite(xs, "values")?;
    let mut v = xs.to_vec();
    v.sort_by(f64::total_cmp);
    let m = v.len() / 2;
    Ok(if v.len() % 2 == 1 {
        v[m]
    } else {
        0.5 * (v[m - 1] + v[m])
    })
}

/// Median absolute deviation scaled by 1.4826 (consistent estimator of σ for normal data; Rousseeuw & Croux 1993).
pub fn mad_sigma(xs: &[f64]) -> StatsResult<f64> {
    let med = median(xs)?;
    let dev: Vec<f64> = xs.iter().map(|x| (x - med).abs()).collect();
    Ok(1.482_602_218_505_602 * median(&dev)?)
}

/// Empirical quantile (type 7, linear interpolation; Hyndman & Fan 1996) of finite values, q in [0, 1].
pub fn quantile(xs: &[f64], q: f64) -> StatsResult<f64> {
    need(xs.len(), 1)?;
    check_finite(xs, "values")?;
    let mut v = xs.to_vec();
    v.sort_by(f64::total_cmp);
    let h = (v.len() - 1) as f64 * q.clamp(0.0, 1.0);
    let lo = h.floor() as usize;
    let hi = (lo + 1).min(v.len() - 1);
    Ok(v[lo] + (h - lo as f64) * (v[hi] - v[lo]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hand_computed() {
        let s = summary(&[2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]).unwrap();
        assert_eq!(s.mean, 5.0);
        assert!((s.variance - 32.0 / 7.0).abs() < 1e-15);
        assert_eq!((s.min, s.max), (2.0, 9.0));
        assert_eq!(median(&[3.0, 1.0, 2.0]).unwrap(), 2.0);
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]).unwrap(), 2.5);
        assert_eq!(quantile(&[1.0, 2.0, 3.0, 4.0, 5.0], 0.25).unwrap(), 2.0);
        assert_eq!(summary(&[3.0]).unwrap().variance, 0.0);
        assert!(summary(&[]).is_err());
        assert!(summary(&[1.0, f64::NAN]).is_err());
    }

    #[test]
    fn welford_is_stable_with_large_offset() {
        // Naive Σx² − n·x̄² loses everything here; Welford keeps ~1e-7 relative.
        let xs: Vec<f64> = (0..1000).map(|i| 1e9 + (i % 10) as f64).collect();
        let s = summary(&xs).unwrap();
        assert!((s.variance - 8.258_258_258_258_258).abs() < 1e-6);
    }

    proptest::proptest! {
        #[test]
        fn shift_invariant_variance(xs in proptest::collection::vec(-1e3f64..1e3, 2..200), c in -1e3f64..1e3) {
            let a = summary(&xs).unwrap();
            let shifted: Vec<f64> = xs.iter().map(|x| x + c).collect();
            let b = summary(&shifted).unwrap();
            proptest::prop_assert!((a.variance - b.variance).abs() <= 1e-9 * (1.0 + a.variance));
            proptest::prop_assert!(a.variance >= 0.0 && a.min <= a.mean + 1e-9 && a.mean <= a.max + 1e-9);
        }
    }
}

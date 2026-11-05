//! EWMA and tabular CUSUM charts (Montgomery ch. 9), which detect small sustained shifts that Shewhart charts miss.

use crate::error::{StatsError, StatsResult, check_finite, need};

fn positive(x: f64) -> bool {
    x > 0.0 && x.is_finite()
}

fn check_target(target: f64, sigma: f64) -> StatsResult<()> {
    if !target.is_finite() || !sigma.is_finite() {
        return Err(StatsError::NonFinite("target/sigma"));
    }
    if sigma <= 0.0 {
        return Err(StatsError::ZeroVariance("sigma must be > 0"));
    }
    Ok(())
}

/// EWMA chart output.
#[derive(Debug, Clone, PartialEq)]
pub struct EwmaChart {
    /// z_i = λ x_i + (1 − λ) z_{i−1}, z_0 = target.
    pub z: Vec<f64>,
    /// Exact time-varying limits target ± L σ √(λ/(2−λ) · (1 − (1−λ)^{2i})).
    pub lcl: Vec<f64>,
    pub ucl: Vec<f64>,
    /// Indices where z is outside its limits.
    pub signals: Vec<usize>,
}

/// EWMA chart with smoothing `lambda` in (0, 1] and width `l` (typ. λ = 0.1, L = 2.7, or λ = 0.2, L = 3).
pub fn ewma(xs: &[f64], target: f64, sigma: f64, lambda: f64, l: f64) -> StatsResult<EwmaChart> {
    need(xs.len(), 1)?;
    check_finite(xs, "values")?;
    check_target(target, sigma)?;
    if lambda.is_nan() || lambda <= 0.0 || lambda > 1.0 || !positive(l) {
        return Err(StatsError::InvalidParameter("ewma needs 0 < lambda <= 1 and L > 0"));
    }
    let mut out = EwmaChart {
        z: vec![],
        lcl: vec![],
        ucl: vec![],
        signals: vec![],
    };
    let mut z = target;
    let mut decay = 1.0; // (1 − λ)^{2i}
    for (i, &x) in xs.iter().enumerate() {
        z = lambda * x + (1.0 - lambda) * z;
        decay *= (1.0 - lambda) * (1.0 - lambda);
        let w = l * sigma * (lambda / (2.0 - lambda) * (1.0 - decay)).sqrt();
        out.z.push(z);
        out.lcl.push(target - w);
        out.ucl.push(target + w);
        if z > target + w || z < target - w {
            out.signals.push(i);
        }
    }
    Ok(out)
}

/// Tabular CUSUM output.
#[derive(Debug, Clone, PartialEq)]
pub struct CusumChart {
    /// C⁺_i = max(0, x_i − (μ0 + K) + C⁺_{i−1}).
    pub upper: Vec<f64>,
    /// C⁻_i = max(0, (μ0 − K) − x_i + C⁻_{i−1}).
    pub lower: Vec<f64>,
    /// Indices where C⁺ > H or C⁻ > H.
    pub signals: Vec<usize>,
}

/// Tabular CUSUM with reference value K = k·σ and decision interval H = h·σ (typ. k = 0.5, h = 4 or 5).
pub fn cusum(xs: &[f64], target: f64, sigma: f64, k: f64, h: f64) -> StatsResult<CusumChart> {
    need(xs.len(), 1)?;
    check_finite(xs, "values")?;
    check_target(target, sigma)?;
    if k.is_nan() || k < 0.0 || k.is_infinite() || !positive(h) {
        return Err(StatsError::InvalidParameter("cusum needs k >= 0 and h > 0"));
    }
    let (kk, hh) = (k * sigma, h * sigma);
    let (mut cp, mut cm) = (0.0f64, 0.0f64);
    let mut out = CusumChart {
        upper: vec![],
        lower: vec![],
        signals: vec![],
    };
    for (i, &x) in xs.iter().enumerate() {
        cp = (x - (target + kk) + cp).max(0.0);
        cm = ((target - kk) - x + cm).max(0.0);
        out.upper.push(cp);
        out.lower.push(cm);
        if cp > hh || cm > hh {
            out.signals.push(i);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Montgomery, Introduction to SQC, Table 9.1 data (μ0 = 10, σ = 1; shift to 11 from observation 21).
    const MONTGOMERY_9_1: [f64; 30] = [
        9.45, 7.99, 9.29, 11.66, 12.16, 10.18, 8.04, 11.46, 9.20, 10.34, 9.03, 11.47, 10.51, 9.40, 10.08, 9.37, 10.62,
        10.31, 8.52, 10.84, 10.90, 9.33, 12.29, 11.50, 10.60, 11.08, 10.38, 11.62, 11.31, 10.52,
    ];

    #[test]
    fn cusum_textbook_example() {
        // Published: with k = 0.5, h = 5 the upper CUSUM first exceeds H at period 29, C⁺_29 = 5.28.
        let c = cusum(&MONTGOMERY_9_1, 10.0, 1.0, 0.5, 5.0).unwrap();
        assert_eq!(c.signals.first(), Some(&28));
        assert!((c.upper[28] - 5.28).abs() < 0.005);
        assert!((c.upper[0] - 0.0).abs() < 1e-12 && (c.lower[0] - 0.05).abs() < 1e-12);
    }

    #[test]
    fn ewma_textbook_example() {
        // Published (λ = 0.1, L = 2.7): z1 = 9.945, z2 = 9.7495, UCL1 = 10.27; first signal at period 29
        // (z29 = 10.6468 > UCL 10.6187).
        let e = ewma(&MONTGOMERY_9_1, 10.0, 1.0, 0.1, 2.7).unwrap();
        assert!((e.z[0] - 9.945).abs() < 1e-12 && (e.z[1] - 9.7495).abs() < 1e-12);
        assert!((e.ucl[0] - 10.27).abs() < 1e-12);
        assert_eq!(e.signals.first(), Some(&28));
        assert!((e.z[28] - 10.6468).abs() < 5e-5);
    }

    #[test]
    fn edge_cases() {
        assert!(ewma(&[], 0.0, 1.0, 0.2, 3.0).is_err());
        assert!(ewma(&[1.0], 0.0, 0.0, 0.2, 3.0).is_err());
        assert!(ewma(&[1.0], 0.0, 1.0, 0.0, 3.0).is_err());
        assert!(cusum(&[f64::INFINITY], 0.0, 1.0, 0.5, 5.0).is_err());
        assert!(cusum(&[1.0], 0.0, 1.0, 0.5, 0.0).is_err());
    }

    proptest::proptest! {
        #[test]
        fn invariants(xs in proptest::collection::vec(-10f64..10.0, 1..200), lambda in 0.01f64..1.0) {
            let c = cusum(&xs, 0.0, 1.0, 0.5, 4.0).unwrap();
            proptest::prop_assert!(c.upper.iter().chain(&c.lower).all(|v| *v >= 0.0));
            let e = ewma(&xs, 0.0, 1.0, lambda, 3.0).unwrap();
            let (lo, hi) = xs.iter().fold((0.0f64, 0.0f64), |(a, b), x| (a.min(*x), b.max(*x)));
            // z is a convex combination of the target and the data.
            proptest::prop_assert!(e.z.iter().all(|z| *z >= lo - 1e-9 && *z <= hi + 1e-9));
            proptest::prop_assert!(e.ucl.windows(2).all(|w| w[1] >= w[0] - 1e-12));
        }
    }
}

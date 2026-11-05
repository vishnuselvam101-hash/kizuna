//! Control-chart constants for subgroup size n.
//!
//! d2(n) = E[R]/σ and d3(n) = sd[R]/σ for the range R of n standard normal values; values tabulated in ASTM E2587
//! Table 1 / Montgomery Appendix VI (d2 is re-derived by numerical integration in the tests, which checks the
//! table). c4(n) = √(2/(n−1)) Γ(n/2)/Γ((n−1)/2) is computed exactly. Derived factors follow Montgomery §6.2–6.3:
//! A2 = 3/(d2√n), D3 = max(0, 1 − 3d3/d2), D4 = 1 + 3d3/d2, A3 = 3/(c4√n),
//! B3 = max(0, 1 − 3√(1−c4²)/c4), B4 = 1 + 3√(1−c4²)/c4.

use crate::error::{StatsError, StatsResult};
use crate::special::ln_gamma;

/// Largest subgroup size for which range-based constants are tabulated (ranges are inefficient beyond ~10 anyway).
pub const MAX_RANGE_N: usize = 25;

const D2: [f64; 24] = [
    1.128, 1.693, 2.059, 2.326, 2.534, 2.704, 2.847, 2.970, 3.078, 3.173, 3.258, 3.336, 3.407, 3.472, 3.532, 3.588,
    3.640, 3.689, 3.735, 3.778, 3.819, 3.858, 3.895, 3.931,
];
const D3: [f64; 24] = [
    0.853, 0.888, 0.880, 0.864, 0.848, 0.833, 0.820, 0.808, 0.797, 0.787, 0.778, 0.770, 0.763, 0.756, 0.750, 0.744,
    0.739, 0.734, 0.729, 0.724, 0.720, 0.716, 0.712, 0.708,
];

fn range_index(n: usize) -> StatsResult<usize> {
    if (2..=MAX_RANGE_N).contains(&n) {
        Ok(n - 2)
    } else {
        Err(StatsError::InvalidParameter(
            "range-based charts need subgroup size 2..=25",
        ))
    }
}

/// d2(n).
pub fn d2(n: usize) -> StatsResult<f64> {
    Ok(D2[range_index(n)?])
}

/// d3(n).
pub fn d3(n: usize) -> StatsResult<f64> {
    Ok(D3[range_index(n)?])
}

/// c4(n), n ≥ 2.
pub fn c4(n: usize) -> StatsResult<f64> {
    if n < 2 {
        return Err(StatsError::InvalidParameter("c4 needs n >= 2"));
    }
    let nf = n as f64;
    Ok((2.0 / (nf - 1.0)).sqrt() * (ln_gamma(nf / 2.0) - ln_gamma((nf - 1.0) / 2.0)).exp())
}

/// (A2, D3, D4) for the X̄-R chart.
pub fn xbar_r_factors(n: usize) -> StatsResult<(f64, f64, f64)> {
    let (d2, d3) = (d2(n)?, d3(n)?);
    Ok((
        3.0 / (d2 * (n as f64).sqrt()),
        (1.0 - 3.0 * d3 / d2).max(0.0),
        1.0 + 3.0 * d3 / d2,
    ))
}

/// (A3, B3, B4) for the X̄-S chart.
pub fn xbar_s_factors(n: usize) -> StatsResult<(f64, f64, f64)> {
    let c4 = c4(n)?;
    let k = 3.0 * (1.0 - c4 * c4).sqrt() / c4;
    Ok((3.0 / (c4 * (n as f64).sqrt()), (1.0 - k).max(0.0), 1.0 + k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::special::normal_cdf;

    /// d2(n) = ∫ [1 − Φ(x)^n − (1 − Φ(x))^n] dx (expected range of n standard normals), Simpson's rule on [−10, 10].
    fn d2_integral(n: usize) -> f64 {
        let (a, b, m) = (-10.0, 10.0, 4000);
        let h = (b - a) / m as f64;
        let f = |x: f64| {
            let p = normal_cdf(x);
            1.0 - p.powi(n as i32) - (1.0 - p).powi(n as i32)
        };
        let mut s = f(a) + f(b);
        for i in 1..m {
            s += f(a + i as f64 * h) * if i % 2 == 1 { 4.0 } else { 2.0 };
        }
        s * h / 3.0
    }

    #[test]
    fn d2_table_matches_integral() {
        for n in 2..=MAX_RANGE_N {
            assert!((d2(n).unwrap() - d2_integral(n)).abs() < 6e-4, "n={n}");
        }
        assert!((d2_integral(2) - 2.0 / std::f64::consts::PI.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn published_factors() {
        // Montgomery Appendix VI, n = 5: A2 .577, D3 0, D4 2.114, A3 1.427, B3 0, B4 2.089, c4 .9400.
        let (a2, d3, d4) = xbar_r_factors(5).unwrap();
        assert!((a2 - 0.577).abs() < 5e-4 && d3 == 0.0 && (d4 - 2.114).abs() < 5e-4);
        let (a3, b3, b4) = xbar_s_factors(5).unwrap();
        assert!((a3 - 1.427).abs() < 5e-4 && b3 == 0.0 && (b4 - 2.089).abs() < 5e-4);
        assert!((c4(5).unwrap() - 0.9400).abs() < 5e-5);
        // n = 10: D3 .223, D4 1.777, B3 .284, B4 1.716.
        let (_, d3, d4) = xbar_r_factors(10).unwrap();
        assert!((d3 - 0.223).abs() < 1e-3 && (d4 - 1.777).abs() < 1e-3);
        let (_, b3, b4) = xbar_s_factors(10).unwrap();
        assert!((b3 - 0.284).abs() < 1e-3 && (b4 - 1.716).abs() < 1e-3);
        // n = 2: c4 = √(2/π).
        assert!((c4(2).unwrap() - (2.0 / std::f64::consts::PI).sqrt()).abs() < 1e-14);
        assert!(d2(1).is_err() && d2(26).is_err() && c4(1).is_err());
    }
}

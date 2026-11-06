//! Short-run SPC: Q-charts and deviation-from-nominal (DNOM) charts.
//!
//! A supplier making 15 pieces of a part number never collects the 20–25 subgroups a Shewhart chart needs to
//! estimate its limits. Two standard answers:
//!
//! * **Q-charts** (C. P. Quesenberry, "SPC Q charts for start-up processes and short or long runs", J. Quality
//!   Technology 23 (1991) 213–224). Each new value is standardised against the values before it and mapped to a
//!   standard normal under H0 (independent N(μ, σ²)), so a chart with limits ±3 and the usual run rules is valid
//!   from the 3rd value on, with μ and σ unknown. Case "μ, σ unknown":
//!   Q_r = Φ⁻¹(T_{r−2}(w_r)), w_r = √((r − 1)/r)·(x_r − x̄_{r−1}) / s_{r−1}, r ≥ 3.
//!   Case "σ known": Q_r = √((r − 1)/r)·(x_r − x̄_{r−1}) / σ, r ≥ 2.
//!   The Q_r are independent N(0, 1) under H0 (Quesenberry 1991, Theorem 1). After a shift the running mean
//!   absorbs it, so a Q-chart catches a shift soon after it happens or not at all.
//! * **DNOM** (Montgomery §10.1; Bothe 1989): several part numbers with different nominals but one process share a
//!   chart of x − nominal (or (x − nominal)/σ_part when the parts' spreads differ).

use crate::error::{StatsError, StatsResult, check_finite, need};
use crate::special::{normal_quantile, student_t_cdf};

/// One point of a Q-chart. `index` is the 0-based position of the value in the input series.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QPoint {
    pub index: usize,
    pub q: f64,
}

/// Q statistics of a series with unknown mean and variance (first point at index 2). A value whose predecessors
/// are all identical (s = 0) has no Q statistic and is skipped; the explanation of a finding should say so.
pub fn q_chart(xs: &[f64]) -> StatsResult<Vec<QPoint>> {
    need(xs.len(), 3)?;
    check_finite(xs, "values")?;
    let mut out = Vec::with_capacity(xs.len() - 2);
    // Welford running mean / sum of squared deviations of x_1..x_{r-1}.
    let (mut mean, mut m2) = (xs[0], 0.0);
    for (i, &x) in xs.iter().enumerate().skip(1) {
        let r = (i + 1) as f64; // 1-based position of x
        if i >= 2 {
            let s = (m2 / (r - 2.0)).sqrt();
            if s > 0.0 {
                let w = ((r - 1.0) / r).sqrt() * (x - mean) / s;
                let u = student_t_cdf(w, r - 2.0).clamp(1e-300, 1.0 - 1e-16);
                out.push(QPoint {
                    index: i,
                    q: normal_quantile(u),
                });
            }
        }
        let d = x - mean;
        mean += d / r;
        m2 += d * (x - mean);
    }
    Ok(out)
}

/// Q statistics with known σ (mean unknown; first point at index 1).
pub fn q_chart_known_sigma(xs: &[f64], sigma: f64) -> StatsResult<Vec<QPoint>> {
    need(xs.len(), 2)?;
    check_finite(xs, "values")?;
    if !(sigma.is_finite() && sigma > 0.0) {
        return Err(StatsError::InvalidParameter("sigma must be > 0"));
    }
    let mut mean = xs[0];
    let mut out = Vec::with_capacity(xs.len() - 1);
    for (i, &x) in xs.iter().enumerate().skip(1) {
        let r = (i + 1) as f64;
        out.push(QPoint {
            index: i,
            q: ((r - 1.0) / r).sqrt() * (x - mean) / sigma,
        });
        mean += (x - mean) / r;
    }
    Ok(out)
}

/// Deviation from nominal: x − nominal for each (value, nominal) in production order.
pub fn dnom(points: &[(f64, f64)]) -> StatsResult<Vec<f64>> {
    let flat: Vec<f64> = points.iter().flat_map(|(a, b)| [*a, *b]).collect();
    check_finite(&flat, "values / nominals")?;
    Ok(points.iter().map(|(x, n)| x - n).collect())
}

/// Standardised deviation from nominal: (x − nominal) / σ_part for each (value, nominal, σ_part).
pub fn dnom_standardized(points: &[(f64, f64, f64)]) -> StatsResult<Vec<f64>> {
    let flat: Vec<f64> = points.iter().flat_map(|(a, b, c)| [*a, *b, *c]).collect();
    check_finite(&flat, "values / nominals / sigmas")?;
    if points.iter().any(|p| p.2 <= 0.0) {
        return Err(StatsError::InvalidParameter("part sigma must be > 0"));
    }
    Ok(points.iter().map(|(x, n, s)| (x - n) / s).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::Rng;

    #[test]
    fn hand_computed() {
        // x = 1, 2, 4: r = 3, x̄₂ = 1.5, s₂ = 0.7071, w = √(2/3)·2.5/0.7071 = 2.8868; T₁(w) = ½ + atan(w)/π.
        let q = q_chart(&[1.0, 2.0, 4.0]).unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].index, 2);
        let w = (2.0f64 / 3.0).sqrt() * 2.5 / 0.5f64.sqrt();
        let u = 0.5 + w.atan() / std::f64::consts::PI;
        assert!((q[0].q - normal_quantile(u)).abs() < 1e-12);
        // Known σ = 1: Q₂ = √(1/2)·(2 − 1) = 0.7071, Q₃ = √(2/3)·(4 − 1.5).
        let k = q_chart_known_sigma(&[1.0, 2.0, 4.0], 1.0).unwrap();
        assert!((k[0].q - 0.5f64.sqrt()).abs() < 1e-15 && (k[1].q - (2.0f64 / 3.0).sqrt() * 2.5).abs() < 1e-12);
        // Identical predecessors give no Q point.
        assert!(q_chart(&[5.0, 5.0, 6.0]).unwrap().is_empty());
        assert!(q_chart(&[1.0, 2.0]).is_err());
        assert_eq!(dnom(&[(10.02, 10.0), (19.97, 20.0)]).unwrap().len(), 2);
        assert!(dnom_standardized(&[(1.0, 1.0, 0.0)]).is_err());
    }

    #[test]
    fn q_statistics_are_standard_normal_under_h0() {
        // Pooled over 2 000 runs of 15 values with different unknown μ, σ: mean ≈ 0, sd ≈ 1, |Q| > 3 ≈ 0.27 %.
        let mut r = Rng::new(3);
        let mut qs = Vec::new();
        for _ in 0..2000 {
            let (mu, sd) = (r.uniform() * 100.0, 0.01 + r.uniform() * 5.0);
            let xs: Vec<f64> = (0..15).map(|_| mu + sd * r.normal()).collect();
            qs.extend(q_chart(&xs).unwrap().into_iter().map(|p| p.q));
        }
        let s = crate::describe::summary(&qs).unwrap();
        assert!(s.mean.abs() < 0.02 && (s.sd() - 1.0).abs() < 0.02, "{s:?}");
        let out = qs.iter().filter(|q| q.abs() > 3.0).count() as f64 / qs.len() as f64;
        assert!(out < 0.0045, "{out}");
    }

    proptest::proptest! {
        #[test]
        fn q_chart_is_location_scale_invariant(xs in proptest::collection::vec(-100f64..100.0, 3..40), a in -1e3f64..1e3, b in 0.01f64..100.0) {
            let q1 = q_chart(&xs).unwrap();
            let ys: Vec<f64> = xs.iter().map(|x| a + b * x).collect();
            let q2 = q_chart(&ys).unwrap();
            proptest::prop_assert_eq!(q1.len(), q2.len());
            for (p, q) in q1.iter().zip(&q2) {
                proptest::prop_assert_eq!(p.index, q.index);
                proptest::prop_assert!((p.q - q.q).abs() < 1e-6 * (1.0 + p.q.abs()));
            }
        }

        #[test]
        fn dnom_shifts_by_nominal(v in proptest::collection::vec((-100f64..100.0, -100f64..100.0), 0..30)) {
            let d = dnom(&v).unwrap();
            for ((x, n), e) in v.iter().zip(&d) { proptest::prop_assert!((x - n - e).abs() < 1e-12); }
        }
    }
}

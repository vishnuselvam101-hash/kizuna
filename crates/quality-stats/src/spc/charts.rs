//! Shewhart variables charts: X̄-R, X̄-S and individuals / moving range (Montgomery ch. 6).
//!
//! Limits are "Phase I" trial limits estimated from the same data (three-sigma). Callers holding established
//! Phase II limits can compare points against them directly.

use super::constants::{c4, d2, xbar_r_factors, xbar_s_factors};
use crate::describe::summary;
use crate::error::{StatsError, StatsResult, check_finite, need};

/// Centre line and three-sigma limits of one chart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChartLimits {
    pub lcl: f64,
    pub center: f64,
    pub ucl: f64,
}

impl ChartLimits {
    /// Indices of points strictly outside the limits.
    pub fn out_of_control(&self, points: &[f64]) -> Vec<usize> {
        points
            .iter()
            .enumerate()
            .filter(|(_, p)| **p > self.ucl || **p < self.lcl)
            .map(|(i, _)| i)
            .collect()
    }
}

/// X̄ chart plus a dispersion chart (R or S) over equal-size subgroups.
#[derive(Debug, Clone, PartialEq)]
pub struct SubgroupChart {
    pub subgroup_size: usize,
    /// Subgroup means.
    pub means: Vec<f64>,
    /// Subgroup ranges (X̄-R) or standard deviations (X̄-S).
    pub dispersion: Vec<f64>,
    pub xbar: ChartLimits,
    pub dispersion_limits: ChartLimits,
    /// Within-subgroup σ estimate: R̄/d2 or S̄/c4.
    pub sigma_within: f64,
}

/// (subgroup size, means, ranges, standard deviations).
type SubgroupStats = (usize, Vec<f64>, Vec<f64>, Vec<f64>);

fn subgroup_stats<S: AsRef<[f64]>>(subgroups: &[S], min_n: usize) -> StatsResult<SubgroupStats> {
    need(subgroups.len(), 2)?;
    let n = subgroups[0].as_ref().len();
    need(n, min_n)?;
    let (mut means, mut ranges, mut sds) = (vec![], vec![], vec![]);
    for g in subgroups {
        let g = g.as_ref();
        if g.len() != n {
            return Err(StatsError::InvalidParameter("all subgroups must have the same size"));
        }
        check_finite(g, "subgroup")?;
        let s = summary(g)?;
        means.push(s.mean);
        ranges.push(s.max - s.min);
        sds.push(s.sd());
    }
    Ok((n, means, ranges, sds))
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

/// X̄-R chart: CL = X̿, limits X̿ ± A2·R̄; R chart limits D3·R̄, D4·R̄; σ̂ = R̄/d2. Subgroup size 2..=25, ≥ 2 subgroups.
pub fn xbar_r<S: AsRef<[f64]>>(subgroups: &[S]) -> StatsResult<SubgroupChart> {
    let (n, means, ranges, _) = subgroup_stats(subgroups, 2)?;
    let (a2, d3f, d4f) = xbar_r_factors(n)?;
    let (xbb, rbar) = (mean(&means), mean(&ranges));
    Ok(SubgroupChart {
        subgroup_size: n,
        xbar: ChartLimits {
            lcl: xbb - a2 * rbar,
            center: xbb,
            ucl: xbb + a2 * rbar,
        },
        dispersion_limits: ChartLimits {
            lcl: d3f * rbar,
            center: rbar,
            ucl: d4f * rbar,
        },
        sigma_within: rbar / d2(n)?,
        means,
        dispersion: ranges,
    })
}

/// X̄-S chart: limits X̿ ± A3·S̄; S chart limits B3·S̄, B4·S̄; σ̂ = S̄/c4. Subgroup size ≥ 2, ≥ 2 subgroups.
pub fn xbar_s<S: AsRef<[f64]>>(subgroups: &[S]) -> StatsResult<SubgroupChart> {
    let (n, means, _, sds) = subgroup_stats(subgroups, 2)?;
    let (a3, b3, b4) = xbar_s_factors(n)?;
    let (xbb, sbar) = (mean(&means), mean(&sds));
    Ok(SubgroupChart {
        subgroup_size: n,
        xbar: ChartLimits {
            lcl: xbb - a3 * sbar,
            center: xbb,
            ucl: xbb + a3 * sbar,
        },
        dispersion_limits: ChartLimits {
            lcl: b3 * sbar,
            center: sbar,
            ucl: b4 * sbar,
        },
        sigma_within: sbar / c4(n)?,
        means,
        dispersion: sds,
    })
}

/// Individuals and moving-range chart.
#[derive(Debug, Clone, PartialEq)]
pub struct IndividualsChart {
    /// |x_i − x_{i−1}| for i = 1..n (length n − 1).
    pub moving_ranges: Vec<f64>,
    pub individuals: ChartLimits,
    pub moving_range: ChartLimits,
    /// MR̄ / d2(2) = MR̄ / 1.128.
    pub sigma_within: f64,
}

/// I-MR chart (span-2 moving range): limits x̄ ± 3·MR̄/d2(2); MR chart UCL = D4(2)·MR̄, LCL = 0. Needs n ≥ 2.
pub fn individuals_moving_range(xs: &[f64]) -> StatsResult<IndividualsChart> {
    need(xs.len(), 2)?;
    check_finite(xs, "individuals")?;
    let mrs: Vec<f64> = xs.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    let mrbar = mean(&mrs);
    let sigma = mrbar / d2(2)?;
    let (_, d3f, d4f) = xbar_r_factors(2)?;
    let xbar = mean(xs);
    Ok(IndividualsChart {
        individuals: ChartLimits {
            lcl: xbar - 3.0 * sigma,
            center: xbar,
            ucl: xbar + 3.0 * sigma,
        },
        moving_range: ChartLimits {
            lcl: d3f * mrbar,
            center: mrbar,
            ucl: d4f * mrbar,
        },
        sigma_within: sigma,
        moving_ranges: mrs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} vs {b}");
    }

    #[test]
    fn xbar_r_hand_computed() {
        // Means 2, 5, 8 → X̿ = 5; ranges 2, 2, 2 → R̄ = 2; n = 3: A2 = 3/(1.693·√3).
        let g = [vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0], vec![7.0, 8.0, 9.0]];
        let c = xbar_r(&g).unwrap();
        let a2 = 3.0 / (1.693 * 3f64.sqrt());
        close(c.xbar.center, 5.0);
        close(c.xbar.ucl, 5.0 + a2 * 2.0);
        close(c.dispersion_limits.center, 2.0);
        close(c.dispersion_limits.ucl, (1.0 + 3.0 * 0.888 / 1.693) * 2.0);
        close(c.dispersion_limits.lcl, 0.0);
        close(c.sigma_within, 2.0 / 1.693);
        assert_eq!(c.xbar.out_of_control(&c.means), vec![0, 2]);
    }

    #[test]
    fn xbar_s_hand_computed() {
        // Each subgroup {m−1, m, m+1}: s = 1, S̄ = 1; c4(3) = √π/2 · ... computed = 0.886227.
        let g = [vec![1.0, 2.0, 3.0], vec![2.0, 3.0, 4.0]];
        let c = xbar_s(&g).unwrap();
        close(c.dispersion_limits.center, 1.0);
        close(c.sigma_within, 1.0 / 0.886_226_925_452_758);
        close(c.xbar.center, 2.5);
    }

    #[test]
    fn imr_hand_computed() {
        // MRs 2, 1, 3 → MR̄ = 2; x̄ = 11.75; σ̂ = 2/1.128.
        let c = individuals_moving_range(&[10.0, 12.0, 11.0, 14.0]).unwrap();
        assert_eq!(c.moving_ranges, vec![2.0, 1.0, 3.0]);
        close(c.individuals.center, 11.75);
        close(c.individuals.ucl, 11.75 + 3.0 * 2.0 / 1.128);
        close(c.moving_range.ucl, (1.0 + 3.0 * 0.853 / 1.128) * 2.0);
    }

    #[test]
    fn edge_cases() {
        assert!(individuals_moving_range(&[1.0]).is_err());
        assert!(individuals_moving_range(&[1.0, f64::NAN]).is_err());
        assert!(xbar_r(&[vec![1.0, 2.0]]).is_err());
        assert!(xbar_r(&[vec![1.0, 2.0], vec![1.0]]).is_err());
        assert!(xbar_r(&[vec![1.0], vec![2.0]]).is_err());
        // Zero variance is reported, not an error: limits collapse onto the centre.
        let c = xbar_r(&[vec![3.0, 3.0], vec![3.0, 3.0]]).unwrap();
        assert_eq!((c.xbar.lcl, c.xbar.ucl, c.sigma_within), (3.0, 3.0, 0.0));
    }

    proptest::proptest! {
        #[test]
        fn limits_ordered_and_shift_equivariant(
            data in proptest::collection::vec(proptest::collection::vec(-100f64..100.0, 4), 2..30),
            shift in -50f64..50.0,
        ) {
            let a = xbar_r(&data).unwrap();
            proptest::prop_assert!(a.xbar.lcl <= a.xbar.center && a.xbar.center <= a.xbar.ucl);
            let shifted: Vec<Vec<f64>> = data.iter().map(|g| g.iter().map(|x| x + shift).collect()).collect();
            let b = xbar_r(&shifted).unwrap();
            proptest::prop_assert!((b.xbar.center - a.xbar.center - shift).abs() < 1e-9);
            proptest::prop_assert!((b.sigma_within - a.sigma_within).abs() < 1e-9);
            let s = xbar_s(&data).unwrap();
            proptest::prop_assert!(s.dispersion_limits.lcl >= 0.0 && s.sigma_within >= 0.0);
        }
    }
}

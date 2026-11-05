//! Process capability and performance indices with confidence intervals.
//!
//! With mean μ̂, within-subgroup σ̂_w (short-term, e.g. R̄/d2 or MR̄/1.128) and overall sample sd s (long-term):
//! - Cp = (USL − LSL)/(6σ̂_w), Cpu = (USL − μ̂)/(3σ̂_w), Cpl = (μ̂ − LSL)/(3σ̂_w), Cpk = min(Cpu, Cpl);
//! - Pp, Ppu, Ppl, Ppk: the same with s in place of σ̂_w (AIAG SPC manual, 2nd ed.; Montgomery §8.2–8.3).
//!
//! One-sided specifications: only the index for the given side exists, Cpk = Cpu (or Cpl); Cp/Pp are `None`.
//!
//! Confidence intervals (normal data, two-sided level 1 − α, ν = n − 1):
//! - Cp, Pp: exact chi-square interval Cp·√(χ²_{α/2,ν}/ν) … Cp·√(χ²_{1−α/2,ν}/ν) (Montgomery eq. 8.19). For Cp the
//!   σ̂_w is not a plain sample sd, so using ν = n − 1 is an approximation (slightly too narrow for range-based σ̂).
//! - Cpk, Ppk and one-sided indices: Bissell (1990) normal approximation
//!   Ĉ ± z_{1−α/2} √(1/(9n) + Ĉ²/(2(n−1))). Reasonable for n ≥ 25; reported for smaller n with that caveat.
//!
//! The indices assume a stable (in-control), approximately normal process; check the charts first.

use crate::describe::summary;
use crate::error::{StatsError, StatsResult, check_finite, need};
use crate::spc::individuals_moving_range;
use crate::special::{chi2_quantile, normal_quantile};

/// Specification limits; at least one must be present and LSL < USL when both are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    pub lsl: Option<f64>,
    pub usl: Option<f64>,
}

/// A point estimate with a two-sided confidence interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    pub value: f64,
    pub lower: f64,
    pub upper: f64,
}

/// Capability (within σ) and performance (overall σ) indices.
#[derive(Debug, Clone, PartialEq)]
pub struct CapabilityReport {
    pub n: usize,
    pub mean: f64,
    pub sigma_within: f64,
    pub sigma_overall: f64,
    pub confidence: f64,
    /// `None` for a one-sided specification.
    pub cp: Option<Estimate>,
    /// min of the existing one-sided indices (= Cpu or Cpl for a one-sided spec).
    pub cpk: Estimate,
    pub cpu: Option<f64>,
    pub cpl: Option<f64>,
    pub pp: Option<Estimate>,
    pub ppk: Estimate,
    /// Plain-language caveats (small n, approximations).
    pub notes: Vec<String>,
}

/// Compute indices for `values` against `spec`.
///
/// `sigma_within`: short-term σ from a control chart; `None` uses the individuals moving range (MR̄/1.128).
/// `confidence` in (0, 1), e.g. 0.95. Needs n ≥ 2 and a positive spread.
pub fn capability(
    values: &[f64],
    spec: Spec,
    sigma_within: Option<f64>,
    confidence: f64,
) -> StatsResult<CapabilityReport> {
    need(values.len(), 2)?;
    check_finite(values, "values")?;
    if !(confidence > 0.0 && confidence < 1.0) {
        return Err(StatsError::InvalidParameter("confidence must be in (0, 1)"));
    }
    let limits = [spec.lsl, spec.usl];
    if limits.iter().flatten().any(|v| !v.is_finite()) {
        return Err(StatsError::NonFinite("spec limits"));
    }
    match (spec.lsl, spec.usl) {
        (None, None) => return Err(StatsError::InvalidParameter("need at least one specification limit")),
        (Some(l), Some(u)) if l >= u => return Err(StatsError::InvalidParameter("LSL must be below USL")),
        _ => {}
    }
    let s = summary(values)?;
    let sigma_w = match sigma_within {
        Some(v) if !v.is_finite() => return Err(StatsError::NonFinite("sigma_within")),
        Some(v) => v,
        None => individuals_moving_range(values)?.sigma_within,
    };
    if sigma_w <= 0.0 || s.sd() <= 0.0 {
        return Err(StatsError::ZeroVariance(
            "capability indices are undefined for zero spread",
        ));
    }
    let n = s.n;
    let nu = (n - 1) as f64;
    let alpha = 1.0 - confidence;
    let z = normal_quantile(1.0 - alpha / 2.0);
    let bissell = |c: f64| {
        let half = z * (1.0 / (9.0 * n as f64) + c * c / (2.0 * nu)).sqrt();
        Estimate {
            value: c,
            lower: c - half,
            upper: c + half,
        }
    };
    let chi = |c: f64| Estimate {
        value: c,
        lower: c * (chi2_quantile(alpha / 2.0, nu) / nu).sqrt(),
        upper: c * (chi2_quantile(1.0 - alpha / 2.0, nu) / nu).sqrt(),
    };
    let one_sided = |sigma: f64| {
        (
            spec.usl.map(|u| (u - s.mean) / (3.0 * sigma)),
            spec.lsl.map(|l| (s.mean - l) / (3.0 * sigma)),
        )
    };
    let k_of = |(u, l): (Option<f64>, Option<f64>)| u.into_iter().chain(l).fold(f64::INFINITY, f64::min);
    let width = spec.lsl.zip(spec.usl).map(|(l, u)| u - l);

    let (cpu, cpl) = one_sided(sigma_w);
    let mut notes = vec![];
    if n < 25 {
        notes.push(format!(
            "n = {n} < 25: Bissell intervals are approximate and wide; treat indices as indicative"
        ));
    }
    if width.is_none() {
        notes.push("one-sided specification: Cp/Pp undefined, Cpk = the existing one-sided index".into());
    }
    if sigma_within.is_none() {
        notes.push("within sigma estimated from the moving range of consecutive values (MR̄/1.128)".into());
    }
    Ok(CapabilityReport {
        n,
        mean: s.mean,
        sigma_within: sigma_w,
        sigma_overall: s.sd(),
        confidence,
        cp: width.map(|w| chi(w / (6.0 * sigma_w))),
        cpk: bissell(k_of((cpu, cpl))),
        cpu,
        cpl,
        pp: width.map(|w| chi(w / (6.0 * s.sd()))),
        ppk: bissell(k_of(one_sided(s.sd()))),
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() <= tol, "{a} vs {b}");
    }

    #[test]
    fn hand_computed_two_sided() {
        // Mean 10 (3×9, 3×11, 4×10), σ_w given as 0.5: Cp = 7/3, Cpu = 4/1.5, Cpl = 3/1.5 = Cpk = 2.
        let v = [9.0, 11.0, 9.0, 11.0, 10.0, 10.0, 9.0, 11.0, 10.0, 10.0];
        let s = summary(&v).unwrap();
        let r = capability(
            &v,
            Spec {
                lsl: Some(7.0),
                usl: Some(14.0),
            },
            Some(0.5),
            0.95,
        )
        .unwrap();
        close(r.cp.unwrap().value, 7.0 / 3.0, 1e-12);
        close(r.cpu.unwrap(), 4.0 / 1.5, 1e-12);
        close(r.cpl.unwrap(), 3.0 / 1.5, 1e-12);
        close(r.cpk.value, 2.0, 1e-12);
        close(r.pp.unwrap().value, 7.0 / (6.0 * s.sd()), 1e-12);
        close(r.ppk.value, 3.0 / (3.0 * s.sd()), 1e-12);
        // Bissell half-width for Cpk = 2, n = 10: 1.959964·√(1/90 + 4/18) = 0.946 47.
        close(
            r.cpk.upper - r.cpk.value,
            1.959_963_984_540_054 * (1.0 / 90.0 + 4.0 / 18.0f64).sqrt(),
            1e-12,
        );
        // χ² interval for Cp, ν = 9: χ²_.025 = 2.700389, χ²_.975 = 19.022768.
        close(
            r.cp.unwrap().lower,
            7.0 / 3.0 * (2.700_389_499_980_7 / 9.0f64).sqrt(),
            1e-8,
        );
        close(
            r.cp.unwrap().upper,
            7.0 / 3.0 * (19.022_767_798_641_6 / 9.0f64).sqrt(),
            1e-8,
        );
    }

    #[test]
    fn one_sided_specs() {
        let v = [9.0, 11.0, 10.0, 10.5, 9.5];
        let r = capability(
            &v,
            Spec {
                lsl: None,
                usl: Some(13.0),
            },
            Some(1.0),
            0.95,
        )
        .unwrap();
        assert!(r.cp.is_none() && r.pp.is_none() && r.cpl.is_none());
        close(r.cpk.value, 1.0, 1e-12);
        let r = capability(
            &v,
            Spec {
                lsl: Some(8.5),
                usl: None,
            },
            Some(0.5),
            0.9,
        )
        .unwrap();
        close(r.cpk.value, 1.0, 1e-12);
        assert!(r.notes.iter().any(|n| n.contains("one-sided")));
    }

    #[test]
    fn errors() {
        let sp = Spec {
            lsl: Some(0.0),
            usl: Some(1.0),
        };
        assert!(capability(&[1.0], sp, None, 0.95).is_err());
        assert!(capability(&[2.0, 2.0, 2.0], sp, None, 0.95).is_err());
        assert!(capability(&[1.0, 2.0], Spec { lsl: None, usl: None }, None, 0.95).is_err());
        assert!(
            capability(
                &[1.0, 2.0],
                Spec {
                    lsl: Some(1.0),
                    usl: Some(1.0)
                },
                None,
                0.95
            )
            .is_err()
        );
        assert!(capability(&[1.0, 2.0], sp, None, 1.0).is_err());
        assert!(capability(&[1.0, 2.0], sp, Some(f64::NAN), 0.9).is_err());
    }

    proptest::proptest! {
        #[test]
        fn invariant_under_affine_change_of_units(
            v in proptest::collection::vec(-5f64..5.0, 3..80), a in 0.01f64..100.0, b in -100f64..100.0
        ) {
            let s = summary(&v).unwrap();
            proptest::prop_assume!(s.sd() > 1e-6 && individuals_moving_range(&v).unwrap().sigma_within > 1e-6);
            let sp = Spec { lsl: Some(-6.0), usl: Some(6.0) };
            let r1 = capability(&v, sp, None, 0.95).unwrap();
            let w: Vec<f64> = v.iter().map(|x| a * x + b).collect();
            let sp2 = Spec { lsl: Some(-6.0 * a + b), usl: Some(6.0 * a + b) };
            let r2 = capability(&w, sp2, None, 0.95).unwrap();
            let tol = 1e-7 * (1.0 + r1.cpk.value.abs());
            proptest::prop_assert!((r1.cpk.value - r2.cpk.value).abs() < tol);
            proptest::prop_assert!((r1.cp.unwrap().value - r2.cp.unwrap().value).abs() < tol);
            proptest::prop_assert!(r1.cpk.value <= r1.cp.unwrap().value + 1e-12);
            proptest::prop_assert!(r1.cpk.lower <= r1.cpk.value && r1.cpk.value <= r1.cpk.upper);
        }
    }
}

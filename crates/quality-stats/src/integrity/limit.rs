//! Exact-at-limit frequency: are values *exactly* equal to a specification limit more frequent than chance?
//!
//! A common manipulation is to replace an out-of-specification result with the limit itself ("clamping") or to
//! nudge borderline values onto it. Equality is checked on the exact decimals, so "10.00" equals the limit "10.0".
//!
//! Model: the process is approximately N(μ, σ²) with robust estimates μ̂ = median, σ̂ = 1.4826·MAD (insensitive to
//! the few clamped values). A value recorded at resolution h equals L when the true value falls in
//! [L − h/2, L + h/2), so p0 = Φ((L + h/2 − μ̂)/σ̂) − Φ((L − h/2 − μ̂)/σ̂). With k of n values at L, the one-sided
//! exact binomial p = P(X ≥ k | n, p0) (Clopper–Pearson style tail). `statistic` = k / (n·p0), the
//! observed/expected ratio. h defaults to the finest resolution present in the values.
//! Guards: n ≥ 10; σ̂ > 0; p0 ≥ 1e-12 (otherwise the normal model says nothing useful about the limit).

use rust_decimal::Decimal;

use crate::decimal::{Measurement, to_f64s};
use crate::describe::{mad_sigma, median};
use crate::special::{binomial_sf, normal_cdf};
use crate::{Applicability, TestResult};

/// Which limit is being tested (affects only the wording).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitSide {
    Lower,
    Upper,
}

/// Minimum values.
pub const MIN_N: usize = 10;

/// Exact-at-limit test (see module docs). `resolution` overrides the inferred recording step.
pub fn exact_at_limit(
    ms: &[Measurement],
    limit: Measurement,
    side: LimitSide,
    resolution: Option<Decimal>,
) -> Applicability<TestResult> {
    if ms.len() < MIN_N {
        return Applicability::NotApplicable(format!("{} values; need at least {MIN_N}", ms.len()));
    }
    let h = resolution.unwrap_or_else(|| {
        let s = ms.iter().map(Measurement::scale).max().unwrap_or(0);
        Decimal::new(1, s)
    });
    let h = Measurement::from_decimal(h).to_f64();
    if !(h > 0.0 && h.is_finite()) {
        return Applicability::NotApplicable("resolution must be positive".into());
    }
    let xs = to_f64s(ms);
    let (Ok(mu), Ok(sigma)) = (median(&xs), mad_sigma(&xs)) else {
        return Applicability::NotApplicable("cannot estimate location/scale".into());
    };
    if sigma <= 0.0 {
        return Applicability::NotApplicable(
            "robust spread (MAD) is zero; no continuous model to compare against".into(),
        );
    }
    let l = limit.to_f64();
    let p0 = normal_cdf((l + h / 2.0 - mu) / sigma) - normal_cdf((l - h / 2.0 - mu) / sigma);
    if p0.is_nan() || p0 < 1e-12 {
        return Applicability::NotApplicable(format!(
            "limit is {:.1} robust sd from the median; the model cannot judge values at it",
            (l - mu) / sigma
        ));
    }
    let n = ms.len();
    let k = ms.iter().filter(|m| m.decimal() == limit.decimal()).count();
    let ratio = k as f64 / (n as f64 * p0);
    let p = binomial_sf(k as u64, n as u64, p0);
    let which = match side {
        LimitSide::Lower => "lower",
        LimitSide::Upper => "upper",
    };
    Applicability::Applicable(TestResult {
        statistic: ratio,
        df: 0.0,
        p_value: p,
        n,
        explanation: format!(
            "{k} of {n} values exactly at the {which} limit {}; expected {:.2} (p0 = {p0:.3e} at resolution {h}); \
             observed/expected {ratio:.2}, p = {p:.3e}",
            limit.decimal(),
            n as f64 * p0
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(v: &[&str]) -> Vec<Measurement> {
        Measurement::parse_all(v).unwrap()
    }

    #[test]
    fn hand_computed_model() {
        // 20 values, 2 exactly at 10.5; expected count n·p0 from the median/MAD model, h = 0.1 (finest scale).
        let v = parse(&[
            "9.6", "9.7", "9.8", "9.8", "9.8", "9.9", "10.0", "10.0", "10.0", "10.0", "10.0", "10.0", "10.1", "10.2",
            "10.2", "10.2", "10.3", "10.4", "10.5", "10.5",
        ]);
        let limit = Measurement::parse("10.50").unwrap();
        let r = exact_at_limit(&v, limit, LimitSide::Upper, None);
        let r = r.applicable().unwrap();
        let xs = to_f64s(&v);
        let (mu, s) = (median(&xs).unwrap(), mad_sigma(&xs).unwrap());
        let p0 = normal_cdf((10.55 - mu) / s) - normal_cdf((10.45 - mu) / s);
        assert!((r.statistic - 2.0 / (20.0 * p0)).abs() < 1e-9);
        assert!((r.p_value - binomial_sf(2, 20, p0)).abs() < 1e-12);
        assert!(r.explanation.starts_with("2 of 20"));
    }

    #[test]
    fn clamped_data_is_flagged() {
        let mut v: Vec<String> = (0..40)
            .map(|i| format!("{:.2}", 9.0 + (i % 10) as f64 * 0.07))
            .collect();
        v.extend(std::iter::repeat_n("10.00".to_string(), 6));
        let ms = Measurement::parse_all(&v).unwrap();
        let r = exact_at_limit(&ms, Measurement::parse("10.0").unwrap(), LimitSide::Upper, None);
        assert!(r.applicable().unwrap().p_value < 1e-4);
    }

    #[test]
    fn guards() {
        let few = parse(&["1.0", "2.0"]);
        assert!(
            exact_at_limit(&few, few[0], LimitSide::Lower, None)
                .applicable()
                .is_none()
        );
        let flat = parse(&["1.0"; 12]);
        assert!(
            exact_at_limit(&flat, flat[0], LimitSide::Lower, None)
                .applicable()
                .is_none()
        );
        let v: Vec<Measurement> = (0..12)
            .map(|i| Measurement::parse(&format!("{}.0", i)).unwrap())
            .collect();
        let far = Measurement::parse("1000.0").unwrap();
        assert!(exact_at_limit(&v, far, LimitSide::Upper, None).applicable().is_none());
    }

    proptest::proptest! {
        #[test]
        fn p_in_unit_interval(units in proptest::collection::vec(900i64..1100, 10..100), lim in 950i64..1050) {
            let ms: Vec<Measurement> = units.iter().map(|u| Measurement::from_decimal(Decimal::new(*u, 2))).collect();
            if let Applicability::Applicable(r) = exact_at_limit(&ms, Measurement::from_decimal(Decimal::new(lim, 2)), LimitSide::Upper, None) {
                proptest::prop_assert!((0.0..=1.0).contains(&r.p_value) && r.statistic >= 0.0);
            }
        }
    }
}

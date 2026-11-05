//! Terminal-digit preference and rounding heaping.
//!
//! When the spread of honest measurements covers many units of the last recorded digit, that digit is close to
//! uniform on 0–9 (the mod-10 residue of a smooth density; for a normal with sd σ_u last-digit units the deviation
//! from uniform is bounded by 2·Σ_k exp(−2π²k²σ_u²/100), ≈ 1.4e-2 at σ_u = 5). People who invent or "tidy" numbers
//! prefer some digits, mostly 0 and 5 (Preece 1981, "Distributions of final digits in data"; Mosimann, Wiseman &
//! Edelman 1995 on terminal digits in fabricated data).
//!
//! - [`terminal_digit_preference`]: Pearson χ² goodness of fit of the 10 last-digit counts to uniform, df = 9.
//! - [`rounding_heaping`]: Whipple-style heaping index H = (share of last digits in {0, 5}) / 0.2
//!   (1 = no heaping, 5 = every value ends in 0 or 5; Whipple's index from demography, here ×1 not ×100),
//!   with an exact one-sided binomial p-value P(X ≥ k | n, 0.2).
//!
//! Applicability guards: only values written with the most common number of decimals are used (mixed precision
//! would mix digit places); n ≥ 50 (expected count ≥ 5 per digit, Cochran's rule); and the spread must be at least
//! 5 last-digit units (sd of the integer units), otherwise non-uniform last digits are expected even when honest.
//! Integers written without decimals ("1200") count their final 0 as a recorded digit, which can be misleading for
//! values rounded to tens; the caller should prefer fields with a stated resolution.

use crate::decimal::Measurement;
use crate::describe::summary;
use crate::special::{binomial_sf, chi2_sf};
use crate::{Applicability, TestResult};

/// Minimum values for the digit tests.
pub const MIN_N: usize = 50;
/// Minimum sd in last-digit units.
pub const MIN_SPREAD_UNITS: f64 = 5.0;

struct DigitSample {
    digits: Vec<u8>,
    excluded: usize,
    scale: u32,
}

fn digit_sample(ms: &[Measurement]) -> Applicability<DigitSample> {
    let mut by_scale = std::collections::BTreeMap::<u32, usize>::new();
    for m in ms {
        *by_scale.entry(m.scale()).or_default() += 1;
    }
    // Most common scale; ties → the smaller scale (deterministic).
    let Some((&scale, _)) = by_scale.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))) else {
        return Applicability::NotApplicable("no values".into());
    };
    let used: Vec<&Measurement> = ms.iter().filter(|m| m.scale() == scale).collect();
    if used.len() < MIN_N {
        return Applicability::NotApplicable(format!(
            "only {} values share the most common precision ({scale} decimals); need {MIN_N} for a digit test",
            used.len()
        ));
    }
    let units: Vec<f64> = used.iter().map(|m| m.units() as f64).collect();
    let sd = summary(&units).map(|s| s.sd()).unwrap_or(0.0);
    if sd < MIN_SPREAD_UNITS {
        return Applicability::NotApplicable(format!(
            "spread is {sd:.2} last-digit units (< {MIN_SPREAD_UNITS}); last digits are not expected to be uniform even for honest data"
        ));
    }
    Applicability::Applicable(DigitSample {
        digits: used.iter().map(|m| m.terminal_digit()).collect(),
        excluded: ms.len() - used.len(),
        scale,
    })
}

/// χ² test of last-digit uniformity. `statistic` = Pearson χ², df = 9, p = upper tail.
pub fn terminal_digit_preference(ms: &[Measurement]) -> Applicability<TestResult> {
    let sample = match digit_sample(ms) {
        Applicability::Applicable(s) => s,
        Applicability::NotApplicable(r) => return Applicability::NotApplicable(r),
    };
    let mut counts = [0usize; 10];
    for &d in &sample.digits {
        counts[d as usize] += 1;
    }
    let n = sample.digits.len();
    let e = n as f64 / 10.0;
    let chi2: f64 = counts.iter().map(|&o| (o as f64 - e).powi(2) / e).sum();
    let p = chi2_sf(chi2, 9.0);
    let top = (0..10).max_by_key(|&d| (counts[d], std::cmp::Reverse(d))).unwrap_or(0);
    Applicability::Applicable(TestResult {
        statistic: chi2,
        df: 9.0,
        p_value: p,
        n,
        explanation: format!(
            "last digits at {} decimals: counts {counts:?}; most frequent digit {top}; χ²(9) = {chi2:.2}, p = {p:.3e}; {} values with other precision excluded",
            sample.scale, sample.excluded
        ),
    })
}

/// Heaping on 0 and 5. `statistic` = H (1 = none, 5 = all), p = P(X ≥ k), X ~ Bin(n, 0.2).
pub fn rounding_heaping(ms: &[Measurement]) -> Applicability<TestResult> {
    let sample = match digit_sample(ms) {
        Applicability::Applicable(s) => s,
        Applicability::NotApplicable(r) => return Applicability::NotApplicable(r),
    };
    let n = sample.digits.len();
    let k = sample.digits.iter().filter(|d| **d == 0 || **d == 5).count();
    let h = k as f64 / (0.2 * n as f64);
    let p = binomial_sf(k as u64, n as u64, 0.2);
    Applicability::Applicable(TestResult {
        statistic: h,
        df: 0.0,
        p_value: p,
        n,
        explanation: format!("{k} of {n} last digits are 0 or 5 (expected 20%); heaping index {h:.2}, p = {p:.3e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(units: &[i64], scale: u32) -> Vec<Measurement> {
        units
            .iter()
            .map(|u| Measurement::from_decimal(rust_decimal::Decimal::new(*u, scale)))
            .collect()
    }

    #[test]
    fn uniform_digits_give_zero_chi2() {
        // 100 values 1000..1099: each last digit exactly 10 times → χ² = 0, p = 1; H = 1.
        let v = ms(&(1000..1100).collect::<Vec<_>>(), 2);
        let r = terminal_digit_preference(&v);
        let r = r.applicable().unwrap();
        assert_eq!(r.statistic, 0.0);
        assert!((r.p_value - 1.0).abs() < 1e-12);
        assert_eq!(rounding_heaping(&v).applicable().unwrap().statistic, 1.0);
    }

    #[test]
    fn hand_computed_preference() {
        // 100 values, 60 ending in 0, 40 spread over 1..9 (digits 1..4 five each, 5..9 four each).
        let mut u = vec![];
        for i in 0..60 {
            u.push(1000 + 10 * i);
        }
        for (d, c) in [(1, 5), (2, 5), (3, 5), (4, 5), (5, 4), (6, 4), (7, 4), (8, 4), (9, 4)] {
            for j in 0..c {
                u.push(1000 + 10 * j + d);
            }
        }
        let v = ms(&u, 1);
        // χ² = (60−10)²/10 + 4·(5−10)²/10 + 5·(4−10)²/10 = 250 + 10 + 18 = 278.
        let r = terminal_digit_preference(&v);
        assert!((r.applicable().unwrap().statistic - 278.0).abs() < 1e-9);
        // H = 64/20 = 3.2.
        let h = rounding_heaping(&v);
        let h = h.applicable().unwrap();
        assert!((h.statistic - 3.2).abs() < 1e-12 && h.p_value < 1e-20);
    }

    #[test]
    fn guards() {
        assert!(
            terminal_digit_preference(&ms(&(0..49).collect::<Vec<_>>(), 1))
                .applicable()
                .is_none()
        );
        // Spread of ~3 units: honest data would not be uniform → refuse.
        let narrow: Vec<i64> = (0..100).map(|i| 500 + (i % 10)).collect();
        assert!(
            matches!(terminal_digit_preference(&ms(&narrow, 2)), Applicability::NotApplicable(r) if r.contains("spread"))
        );
        assert!(rounding_heaping(&[]).applicable().is_none());
        // Mixed precision: only the modal scale is used.
        let mut mixed = ms(&(1000..1100).collect::<Vec<_>>(), 2);
        mixed.extend(ms(&[1, 2, 3], 1));
        assert_eq!(terminal_digit_preference(&mixed).applicable().unwrap().n, 100);
    }

    proptest::proptest! {
        #[test]
        fn permutation_invariant_and_bounded(mut u in proptest::collection::vec(0i64..100_000, 50..300)) {
            let a = terminal_digit_preference(&ms(&u, 2));
            u.reverse();
            let b = terminal_digit_preference(&ms(&u, 2));
            proptest::prop_assert_eq!(a.clone(), b);
            if let Applicability::Applicable(r) = a {
                proptest::prop_assert!(r.statistic >= 0.0 && (0.0..=1.0).contains(&r.p_value));
            }
            if let Applicability::Applicable(h) = rounding_heaping(&ms(&u, 2)) {
                proptest::prop_assert!((0.0..=5.0).contains(&h.statistic) && (0.0..=1.0).contains(&h.p_value));
            }
        }
    }
}

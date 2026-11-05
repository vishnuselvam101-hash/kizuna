//! Benford first-digit test — only where Benford's law can be expected.
//!
//! Benford's law, P(first significant digit = d) = log10(1 + 1/d) (Newcomb 1881; Benford 1938; Hill 1995), holds
//! for data that are scale-invariant or products of many factors and span several orders of magnitude (invoice
//! amounts, impurity concentrations across lots). It does **not** hold for measurements of a controlled process
//! (a 520 ± 10 MPa tensile strength always starts with 5): applying it there produces false alarms, so this test
//! refuses unless (a) n ≥ 100 non-zero values and (b) the central 90% of |values| spans at least two orders of
//! magnitude, log10(q95/q05) ≥ 2 (Nigrini 2012, *Benford's Law*, ch. 3 conditions).
//!
//! Statistic: Pearson χ² over digits 1–9 (df = 8). The explanation also gives Nigrini's mean absolute deviation
//! (MAD) with his first-digit conformity bands (≤ .006 close, ≤ .012 acceptable, ≤ .015 marginal, else
//! non-conformity), because χ² rejects trivial deviations at large n.

use crate::decimal::Measurement;
use crate::describe::quantile;
use crate::special::chi2_sf;
use crate::{Applicability, TestResult};

/// Minimum non-zero values.
pub const MIN_N: usize = 100;
/// Minimum log10(q95/q05) of |values|.
pub const MIN_DECADES: f64 = 2.0;

/// First significant digit of a non-zero decimal, from its exact digits.
pub fn first_digit(m: &Measurement) -> Option<u8> {
    let mut u = m.units().unsigned_abs();
    if u == 0 {
        return None;
    }
    while u >= 10 {
        u /= 10;
    }
    Some(u as u8)
}

/// Benford first-digit test (see module docs for the guard).
pub fn benford_first_digit(ms: &[Measurement]) -> Applicability<TestResult> {
    let nz: Vec<&Measurement> = ms.iter().filter(|m| m.units() != 0).collect();
    if nz.len() < MIN_N {
        return Applicability::NotApplicable(format!("{} non-zero values; Benford needs at least {MIN_N}", nz.len()));
    }
    let mags: Vec<f64> = nz.iter().map(|m| m.to_f64().abs()).collect();
    let (q05, q95) = match (quantile(&mags, 0.05), quantile(&mags, 0.95)) {
        (Ok(a), Ok(b)) if a > 0.0 => (a, b),
        _ => return Applicability::NotApplicable("cannot determine the magnitude range".into()),
    };
    let decades = (q95 / q05).log10();
    if decades < MIN_DECADES {
        return Applicability::NotApplicable(format!(
            "values span {decades:.2} orders of magnitude (central 90%); Benford's law needs ≥ {MIN_DECADES} — \
             measurements of a controlled process are not expected to follow it"
        ));
    }
    let mut counts = [0usize; 9];
    for m in &nz {
        if let Some(d) = first_digit(m) {
            counts[d as usize - 1] += 1;
        }
    }
    let n = nz.len() as f64;
    let (mut chi2, mut mad) = (0.0, 0.0);
    for (i, &o) in counts.iter().enumerate() {
        let p = (1.0 + 1.0 / (i + 1) as f64).log10();
        chi2 += (o as f64 - n * p).powi(2) / (n * p);
        mad += (o as f64 / n - p).abs() / 9.0;
    }
    let band = match mad {
        m if m <= 0.006 => "close conformity",
        m if m <= 0.012 => "acceptable conformity",
        m if m <= 0.015 => "marginal conformity",
        _ => "non-conformity",
    };
    let p = chi2_sf(chi2, 8.0);
    Applicability::Applicable(TestResult {
        statistic: chi2,
        df: 8.0,
        p_value: p,
        n: nz.len(),
        explanation: format!(
            "first digits 1–9 {counts:?} over {decades:.1} decades; χ²(8) = {chi2:.2}, p = {p:.3e}; MAD = {mad:.4} ({band})"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    fn m(units: i64, scale: u32) -> Measurement {
        Measurement::from_decimal(Decimal::new(units, scale))
    }

    #[test]
    fn first_digits() {
        assert_eq!(first_digit(&m(-345, 2)), Some(3));
        assert_eq!(first_digit(&m(7, 4)), Some(7));
        assert_eq!(first_digit(&m(0, 1)), None);
    }

    #[test]
    fn exact_benford_counts_give_small_chi2() {
        // 10^(k/100), k = 0..600: log-mantissas equally spaced over exactly 6 decades — Benford by construction.
        let v: Vec<Measurement> = (0..600)
            .map(|k| Measurement::parse(&format!("{:.4}", 10f64.powf(k as f64 / 100.0))).unwrap())
            .collect();
        let r = benford_first_digit(&v);
        let r = r.applicable().unwrap();
        assert!(r.p_value > 0.05, "{r:?}");
        assert!(r.explanation.contains("conformity"));
    }

    #[test]
    fn refuses_controlled_process_data() {
        let v: Vec<Measurement> = (0..500).map(|i| m(5100 + (i % 200), 1)).collect();
        assert!(
            matches!(benford_first_digit(&v), Applicability::NotApplicable(r) if r.contains("orders of magnitude"))
        );
        assert!(benford_first_digit(&v[..50]).applicable().is_none());
    }

    #[test]
    fn detects_uniform_first_digits() {
        // Each first digit 1–9 equally often over 3 decades: χ² far above Benford.
        let v: Vec<Measurement> = (0..900)
            .map(|i| m(((i % 9) + 1) * 10i64.pow((i / 9 % 4) as u32), 0))
            .collect();
        let r = benford_first_digit(&v);
        assert!(r.applicable().unwrap().p_value < 1e-10);
    }
}

//! Exact decimal measurements parsed from the text that was written on the certificate.
//!
//! `rust_decimal` keeps the written scale ("12.30" has scale 2), so the terminal digit and the recording resolution
//! are taken from what was actually written, not from a float that has forgotten it. Conversion to `f64` happens
//! only when a value enters a statistic; for |x| < 2^53·10^-scale the conversion error is ≤ ½ ulp (≈1.1e-16
//! relative), far below any measurement resolution.

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use std::str::FromStr;

use crate::error::{StatsError, StatsResult};

/// A measured value exactly as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Measurement(Decimal);

impl Measurement {
    /// Parse a plain decimal: optional sign, digits, optional `.` and digits. Surrounding whitespace is ignored.
    /// Exponents, thousands separators, decimal commas, `NaN` and `inf` are rejected (the caller normalises
    /// locale formats before this point, so nothing is guessed here).
    pub fn parse(text: &str) -> StatsResult<Self> {
        let t = text.trim();
        let body = t.strip_prefix(['+', '-']).unwrap_or(t);
        let (int, frac) = body.split_once('.').unwrap_or((body, ""));
        let digits_ok = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        if body.is_empty() || (int.is_empty() && frac.is_empty()) || !digits_ok(int) || !digits_ok(frac) {
            return Err(StatsError::Parse(text.to_string()));
        }
        Decimal::from_str(t)
            .map(Measurement)
            .map_err(|_| StatsError::Parse(text.to_string()))
    }

    /// Parse many values; the first failure is reported.
    pub fn parse_all<S: AsRef<str>>(texts: &[S]) -> StatsResult<Vec<Self>> {
        texts.iter().map(|s| Self::parse(s.as_ref())).collect()
    }

    /// Wrap an existing decimal.
    pub fn from_decimal(d: Decimal) -> Self {
        Measurement(d)
    }

    /// The exact value.
    pub fn decimal(&self) -> Decimal {
        self.0
    }

    /// Number of digits written after the decimal point (trailing zeros count: "1.50" → 2).
    pub fn scale(&self) -> u32 {
        self.0.scale()
    }

    /// The last written digit (0–9), e.g. "12.35" → 5, "1200" → 0.
    pub fn terminal_digit(&self) -> u8 {
        (self.0.mantissa().unsigned_abs() % 10) as u8
    }

    /// The value in units of its last written digit, e.g. "12.35" → 1235 (sign kept).
    pub fn units(&self) -> i128 {
        self.0.mantissa()
    }

    /// Nearest `f64` (≤ ½ ulp error for values representable in 53 bits of mantissa).
    pub fn to_f64(&self) -> f64 {
        self.0.to_f64().unwrap_or(f64::NAN)
    }

    /// Canonical text without trailing zeros, used for duplicate detection ("12.30" and "12.3" are the same value).
    pub fn canonical(&self) -> String {
        self.0.normalize().to_string()
    }
}

/// Convert a slice of measurements to `f64` for the statistics.
pub fn to_f64s(ms: &[Measurement]) -> Vec<f64> {
    ms.iter().map(Measurement::to_f64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_written_scale_and_digit() {
        let m = Measurement::parse(" 12.30 ").unwrap();
        assert_eq!(m.scale(), 2);
        assert_eq!(m.terminal_digit(), 0);
        assert_eq!(m.units(), 1230);
        assert_eq!(m.canonical(), "12.3");
        let n = Measurement::parse("-0.05").unwrap();
        assert_eq!((n.scale(), n.terminal_digit(), n.units()), (2, 5, -5));
        assert_eq!(Measurement::parse("1200").unwrap().terminal_digit(), 0);
        assert_eq!(Measurement::parse(".5").unwrap().to_f64(), 0.5);
    }

    #[test]
    fn rejects_non_plain_decimals() {
        for bad in [
            "", "-", ".", "1e3", "1,5", "1.2.3", "NaN", "inf", "0x10", "1 000", "--1",
        ] {
            assert!(Measurement::parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn exact_equality_is_decimal_not_float() {
        // 0.1 + 0.2 != 0.3 in f64; the decimal comparison used by the limit test is exact.
        let a = Measurement::parse("0.30").unwrap().decimal();
        let b = Measurement::parse("0.1").unwrap().decimal() + Measurement::parse("0.2").unwrap().decimal();
        assert_eq!(a, b);
    }

    proptest::proptest! {
        #[test]
        fn roundtrip_integer_units(units in -1_000_000_000i64..1_000_000_000, scale in 0u32..6) {
            let d = Decimal::new(units, scale);
            let m = Measurement::parse(&d.to_string()).unwrap();
            proptest::prop_assert_eq!(m.units(), units as i128);
            proptest::prop_assert_eq!(m.scale(), scale);
            proptest::prop_assert_eq!(m.terminal_digit() as i64, (units % 10).abs());
        }
    }
}

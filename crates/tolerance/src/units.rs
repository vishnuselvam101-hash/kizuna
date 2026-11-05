//! Units with exact decimal conversion.
//!
//! Every quantity is held in its dimension's canonical unit — millimetres for length, MPa (= N/mm²) for stress.
//! All conversion factors *into* the canonical unit are exact decimals (1 in = 25.4 mm exactly by the 1959
//! international yard and pound agreement; 1 µm = 0.001 mm; 1 MPa = 1 N/mm²; 1 GPa = 1000 MPa; 1 kPa = 0.001 MPa),
//! so reading a value is always exact. Converting *out* (e.g. mm → in) divides; a result that would need more than
//! rust_decimal's 28 fractional digits is refused rather than silently rounded, so a round trip unit → canonical →
//! unit either returns the identical number or an error.

use rust_decimal::Decimal;

use crate::error::{TolError, TolResult};

/// Physical dimension of a quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dimension {
    /// Canonical unit: millimetre.
    Length,
    /// Canonical unit: MPa (N/mm²).
    Stress,
}

/// Supported units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unit {
    Mm,
    Um,
    Inch,
    MPa,
    NPerMm2,
    GPa,
    KPa,
}

impl Unit {
    /// All units, for tests and exhaustive iteration.
    pub const ALL: [Unit; 7] = [
        Unit::Mm,
        Unit::Um,
        Unit::Inch,
        Unit::MPa,
        Unit::NPerMm2,
        Unit::GPa,
        Unit::KPa,
    ];

    pub fn dimension(self) -> Dimension {
        match self {
            Unit::Mm | Unit::Um | Unit::Inch => Dimension::Length,
            Unit::MPa | Unit::NPerMm2 | Unit::GPa | Unit::KPa => Dimension::Stress,
        }
    }

    /// Exact factor that converts a value in this unit into the canonical unit.
    pub fn to_canonical(self) -> Decimal {
        match self {
            Unit::Mm | Unit::MPa | Unit::NPerMm2 => Decimal::ONE,
            Unit::Um | Unit::KPa => Decimal::new(1, 3),
            Unit::Inch => Decimal::new(254, 1),
            Unit::GPa => Decimal::new(1000, 0),
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Unit::Mm => "mm",
            Unit::Um => "µm",
            Unit::Inch => "in",
            Unit::MPa => "MPa",
            Unit::NPerMm2 => "N/mm²",
            Unit::GPa => "GPa",
            Unit::KPa => "kPa",
        }
    }

    /// Canonical unit of a dimension.
    pub fn canonical(dim: Dimension) -> Unit {
        match dim {
            Dimension::Length => Unit::Mm,
            Dimension::Stress => Unit::MPa,
        }
    }
}

/// Convert `value` given in `unit` into the canonical unit of its dimension (always exact or an error).
pub fn to_canonical(value: Decimal, unit: Unit) -> TolResult<Decimal> {
    let f = unit.to_canonical();
    let c = value
        .checked_mul(f)
        .ok_or_else(|| TolError::Arithmetic(format!("{value} {} overflows", unit.symbol())))?;
    // rust_decimal rounds when the scale would exceed 28; detect it instead of accepting a rounded value.
    if c.checked_div(f) != Some(value) {
        return Err(TolError::Arithmetic(format!(
            "{value} {} is not exactly representable in {}",
            unit.symbol(),
            Unit::canonical(unit.dimension()).symbol()
        )));
    }
    Ok(c)
}

/// Convert a canonical value into `unit`; refuses when the decimal result would be rounded.
pub fn from_canonical(canonical: Decimal, unit: Unit) -> TolResult<Decimal> {
    let f = unit.to_canonical();
    let v = canonical
        .checked_div(f)
        .ok_or_else(|| TolError::Arithmetic(format!("{canonical} overflows in {}", unit.symbol())))?;
    if v.checked_mul(f) != Some(canonical) {
        return Err(TolError::Arithmetic(format!(
            "{canonical} has no exact decimal value in {}",
            unit.symbol()
        )));
    }
    Ok(v)
}

/// Convert for display: exact when possible, otherwise rounded to 9 decimal places (never used in decisions).
pub fn display_in(canonical: Decimal, unit: Unit) -> Decimal {
    from_canonical(canonical, unit)
        .unwrap_or_else(|_| (canonical / unit.to_canonical()).round_dp(9))
        .normalize()
}

/// A value with its dimension, held exactly in the canonical unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Quantity {
    canonical: Decimal,
    dimension: Dimension,
}

impl Quantity {
    pub fn new(value: Decimal, unit: Unit) -> TolResult<Self> {
        Ok(Quantity {
            canonical: to_canonical(value, unit)?,
            dimension: unit.dimension(),
        })
    }

    pub fn canonical(&self) -> Decimal {
        self.canonical
    }

    pub fn dimension(&self) -> Dimension {
        self.dimension
    }

    /// The exact value in `unit`.
    pub fn value_in(&self, unit: Unit) -> TolResult<Decimal> {
        if unit.dimension() != self.dimension {
            return Err(TolError::Dimension(format!(
                "{:?} quantity requested in {}",
                self.dimension,
                unit.symbol()
            )));
        }
        from_canonical(self.canonical, unit)
    }

    /// Canonical value after checking the dimension.
    pub(crate) fn canonical_as(&self, dim: Dimension) -> TolResult<Decimal> {
        if dim != self.dimension {
            return Err(TolError::Dimension(format!(
                "expected {dim:?}, got {:?}",
                self.dimension
            )));
        }
        Ok(self.canonical)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).expect("test decimal")
    }

    #[test]
    fn inch_is_exactly_25_4_mm() {
        assert_eq!(Quantity::new(d("1"), Unit::Inch).map(|q| q.canonical()), Ok(d("25.4")));
        assert_eq!(
            Quantity::new(d("0.001"), Unit::Inch).map(|q| q.canonical()),
            Ok(d("0.0254"))
        );
        let q = Quantity::new(d("25.4"), Unit::Mm).expect("q");
        assert_eq!(q.value_in(Unit::Inch), Ok(d("1")));
        assert_eq!(q.value_in(Unit::Um), Ok(d("25400")));
    }

    #[test]
    fn mm_to_inch_without_exact_decimal_is_refused() {
        let q = Quantity::new(d("1"), Unit::Mm).expect("q");
        assert!(matches!(q.value_in(Unit::Inch), Err(TolError::Arithmetic(_))));
        assert_eq!(display_in(q.canonical(), Unit::Inch), d("0.039370079"));
    }

    #[test]
    fn stress_units() {
        let q = Quantity::new(d("0.21"), Unit::GPa).expect("q");
        assert_eq!(q.value_in(Unit::NPerMm2), Ok(d("210")));
        assert_eq!(q.value_in(Unit::KPa), Ok(d("210000")));
        assert!(matches!(q.value_in(Unit::Mm), Err(TolError::Dimension(_))));
    }
}

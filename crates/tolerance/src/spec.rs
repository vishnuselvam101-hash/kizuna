//! The specification model: a nominal value with limits, held exactly in the canonical unit.

use rust_decimal::Decimal;

use crate::error::{TolResult, invalid};
use crate::iso286::{self, Fit};
use crate::units::{Dimension, Unit, display_in, to_canonical};

/// One limit of a specification. `inclusive == false` comes from 未満 / 超 / `<` / `>` notations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bound {
    pub value: Decimal,
    pub inclusive: bool,
}

impl Bound {
    pub fn inclusive(value: Decimal) -> Self {
        Bound { value, inclusive: true }
    }
}

/// A dimensional or material specification. All values are canonical (mm or MPa); `display_unit` is the unit the
/// spec was written in and is used only for explanations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Spec {
    pub dimension: Dimension,
    pub nominal: Option<Decimal>,
    pub lower: Option<Bound>,
    pub upper: Option<Bound>,
    pub display_unit: Unit,
    /// The ISO 286 fit the limits came from, if any.
    pub fit: Option<Fit>,
}

impl Spec {
    fn build(nominal: Option<Decimal>, lower: Option<Bound>, upper: Option<Bound>, unit: Unit) -> TolResult<Self> {
        if lower.is_none() && upper.is_none() {
            return invalid("a specification needs at least one limit");
        }
        if let (Some(l), Some(u)) = (lower, upper)
            && (l.value > u.value || (l.value == u.value && !(l.inclusive && u.inclusive)))
        {
            return invalid(format!("lower limit {} is above upper limit {}", l.value, u.value));
        }
        Ok(Spec {
            dimension: unit.dimension(),
            nominal,
            lower,
            upper,
            display_unit: unit,
            fit: None,
        })
    }

    /// `nominal ± tol`.
    pub fn symmetric(nominal: Decimal, tol: Decimal, unit: Unit) -> TolResult<Self> {
        if tol.is_sign_negative() && !tol.is_zero() {
            return invalid(format!("symmetric tolerance ±{tol} is negative"));
        }
        Self::asymmetric(nominal, tol, -tol, unit)
    }

    /// `nominal +upper_dev / lower_dev` with signed deviations (e.g. `+0.1 / −0.05`); deviations in `unit`.
    pub fn asymmetric(nominal: Decimal, upper_dev: Decimal, lower_dev: Decimal, unit: Unit) -> TolResult<Self> {
        Self::with_deviations(nominal, unit, upper_dev, lower_dev, unit)
    }

    /// Nominal in `unit`, deviations in `dev_unit` (e.g. `10 mm +50/−0 µm`).
    pub fn with_deviations(
        nominal: Decimal,
        unit: Unit,
        upper_dev: Decimal,
        lower_dev: Decimal,
        dev_unit: Unit,
    ) -> TolResult<Self> {
        if unit.dimension() != dev_unit.dimension() {
            return Err(crate::TolError::Dimension(format!(
                "nominal in {} but deviation in {}",
                unit.symbol(),
                dev_unit.symbol()
            )));
        }
        if upper_dev < lower_dev {
            return invalid(format!(
                "upper deviation {upper_dev} is below lower deviation {lower_dev}"
            ));
        }
        let n = to_canonical(nominal, unit)?;
        let (ud, ld) = (to_canonical(upper_dev, dev_unit)?, to_canonical(lower_dev, dev_unit)?);
        let add = |a: Decimal, b: Decimal| {
            a.checked_add(b)
                .ok_or_else(|| crate::TolError::Arithmetic("limit overflows".into()))
        };
        Self::build(
            Some(n),
            Some(Bound::inclusive(add(n, ld)?)),
            Some(Bound::inclusive(add(n, ud)?)),
            unit,
        )
    }

    /// Two explicit limits, `min ~ max`.
    pub fn limits(min: Decimal, max: Decimal, unit: Unit) -> TolResult<Self> {
        Self::build(
            None,
            Some(Bound::inclusive(to_canonical(min, unit)?)),
            Some(Bound::inclusive(to_canonical(max, unit)?)),
            unit,
        )
    }

    /// One-sided lower limit (`10 min`, `10以上`, `> 10` when `inclusive` is false).
    pub fn at_least(min: Decimal, inclusive: bool, unit: Unit) -> TolResult<Self> {
        Self::build(
            None,
            Some(Bound {
                value: to_canonical(min, unit)?,
                inclusive,
            }),
            None,
            unit,
        )
    }

    /// One-sided upper limit (`10 max`, `10以下`, `< 10` when `inclusive` is false).
    pub fn at_most(max: Decimal, inclusive: bool, unit: Unit) -> TolResult<Self> {
        Self::build(
            None,
            None,
            Some(Bound {
                value: to_canonical(max, unit)?,
                inclusive,
            }),
            unit,
        )
    }

    /// Limits of an ISO 286 fit (`25 H7`, `25 g6`); the nominal size is in millimetres.
    pub fn from_fit(nominal_mm: Decimal, fit: Fit) -> TolResult<Self> {
        let (upper, lower) = iso286::deviations(nominal_mm, fit)?;
        let mut s = Self::asymmetric(nominal_mm, upper, lower, Unit::Mm)?;
        s.fit = Some(fit);
        Ok(s)
    }

    /// Width of the tolerance zone (canonical unit), when both limits exist.
    pub fn tolerance(&self) -> Option<Decimal> {
        Some(self.upper?.value - self.lower?.value)
    }

    /// Mid-point of the zone (canonical unit), when both limits exist.
    pub fn midpoint(&self) -> Option<Decimal> {
        Some((self.upper?.value + self.lower?.value) / Decimal::TWO)
    }

    /// Human-readable rendering in the display unit, e.g. `10 +0.1/-0 mm` or `≥ 400 MPa`.
    pub fn describe(&self) -> String {
        let u = self.display_unit;
        let show = |v: Decimal| display_in(v, u);
        let fit = self.fit.map(|f| format!(" {f}")).unwrap_or_default();
        match (self.nominal, self.lower, self.upper) {
            (Some(n), Some(l), Some(h)) => {
                let (up, lo) = (show(h.value - n), show(l.value - n));
                if up == -lo {
                    format!("{}{fit} ±{} {}", show(n), up, u.symbol())
                } else {
                    format!("{}{fit} {:+}/{:+} {}", show(n), up, lo, u.symbol())
                }
            }
            (_, Some(l), Some(h)) => format!("{} ~ {} {}", show(l.value), show(h.value), u.symbol()),
            (_, Some(l), None) => format!(
                "{} {} {}",
                if l.inclusive { "≥" } else { ">" },
                show(l.value),
                u.symbol()
            ),
            (_, None, Some(h)) => format!(
                "{} {} {}",
                if h.inclusive { "≤" } else { "<" },
                show(h.value),
                u.symbol()
            ),
            (_, None, None) => "(no limits)".into(),
        }
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
    fn asymmetric_limits_are_exact() {
        let s = Spec::asymmetric(d("10"), d("0.1"), d("-0.05"), Unit::Mm).expect("spec");
        assert_eq!(s.lower.map(|b| b.value), Some(d("9.95")));
        assert_eq!(s.upper.map(|b| b.value), Some(d("10.1")));
        assert_eq!(s.tolerance(), Some(d("0.15")));
        assert_eq!(s.describe(), "10 +0.1/-0.05 mm");
    }

    #[test]
    fn mixed_units_and_inches() {
        let s = Spec::with_deviations(d("10"), Unit::Mm, d("50"), d("0"), Unit::Um).expect("spec");
        assert_eq!(s.upper.map(|b| b.value), Some(d("10.05")));
        let s = Spec::symmetric(d("0.5"), d("0.002"), Unit::Inch).expect("spec");
        assert_eq!(s.lower.map(|b| b.value), Some(d("12.6492")));
        assert_eq!(s.describe(), "0.5 ±0.002 in");
    }

    #[test]
    fn contradictions_are_refused() {
        assert!(Spec::limits(d("2"), d("1"), Unit::Mm).is_err());
        assert!(Spec::asymmetric(d("10"), d("-0.1"), d("0"), Unit::Mm).is_err());
        assert!(Spec::symmetric(d("10"), d("-0.1"), Unit::Mm).is_err());
        assert!(Spec::with_deviations(d("10"), Unit::Mm, d("1"), d("0"), Unit::MPa).is_err());
    }
}

//! Units of measure with exact decimal conversion to a base unit per dimension.

use rust_decimal::Decimal;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Length,
    Mass,
    Pressure,
    Temperature,
    Hardness,
    Count,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unit {
    pub symbol: &'static str,
    pub dim: Dimension,
    /// Factor to the dimension's base unit (mm, g, MPa). `None` = not linearly convertible (°C/°F, hardness scales).
    pub to_base: Option<&'static str>,
}

const UNITS: &[(&[&str], Unit)] = &[
    (
        &["mm", "millimeter", "millimetre"],
        Unit {
            symbol: "mm",
            dim: Dimension::Length,
            to_base: Some("1"),
        },
    ),
    (
        &["cm"],
        Unit {
            symbol: "cm",
            dim: Dimension::Length,
            to_base: Some("10"),
        },
    ),
    (
        &["m", "meter", "metre"],
        Unit {
            symbol: "m",
            dim: Dimension::Length,
            to_base: Some("1000"),
        },
    ),
    (
        &["um", "µm", "μm", "micron"],
        Unit {
            symbol: "um",
            dim: Dimension::Length,
            to_base: Some("0.001"),
        },
    ),
    (
        &["in", "inch", "inches", "\""],
        Unit {
            symbol: "in",
            dim: Dimension::Length,
            to_base: Some("25.4"),
        },
    ),
    (
        &["g", "gram"],
        Unit {
            symbol: "g",
            dim: Dimension::Mass,
            to_base: Some("1"),
        },
    ),
    (
        &["kg", "kilogram"],
        Unit {
            symbol: "kg",
            dim: Dimension::Mass,
            to_base: Some("1000"),
        },
    ),
    (
        &["lb", "lbs"],
        Unit {
            symbol: "lb",
            dim: Dimension::Mass,
            to_base: Some("453.59237"),
        },
    ),
    (
        &["mpa", "n/mm2", "n/mm²"],
        Unit {
            symbol: "MPa",
            dim: Dimension::Pressure,
            to_base: Some("1"),
        },
    ),
    (
        &["gpa"],
        Unit {
            symbol: "GPa",
            dim: Dimension::Pressure,
            to_base: Some("1000"),
        },
    ),
    (
        &["psi"],
        Unit {
            symbol: "psi",
            dim: Dimension::Pressure,
            to_base: Some("0.00689475729"),
        },
    ),
    (
        &["c", "°c", "degc"],
        Unit {
            symbol: "°C",
            dim: Dimension::Temperature,
            to_base: None,
        },
    ),
    (
        &["f", "°f", "degf"],
        Unit {
            symbol: "°F",
            dim: Dimension::Temperature,
            to_base: None,
        },
    ),
    (
        &["hrc"],
        Unit {
            symbol: "HRC",
            dim: Dimension::Hardness,
            to_base: None,
        },
    ),
    (
        &["hb", "hbw"],
        Unit {
            symbol: "HB",
            dim: Dimension::Hardness,
            to_base: None,
        },
    ),
    (
        &["hv"],
        Unit {
            symbol: "HV",
            dim: Dimension::Hardness,
            to_base: None,
        },
    ),
    (
        &["pcs", "pc", "ea", "each", "units"],
        Unit {
            symbol: "pcs",
            dim: Dimension::Count,
            to_base: Some("1"),
        },
    ),
];

pub fn parse_unit(s: &str) -> Option<Unit> {
    let k = s.trim().to_lowercase();
    UNITS
        .iter()
        .find(|(names, _)| names.contains(&k.as_str()))
        .map(|(_, u)| *u)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QtyCmp {
    Equal,
    Different { a_base: Decimal, b_base: Decimal },
    Incompatible,
    UnknownUnit,
}

pub fn to_base(v: Decimal, u: &Unit) -> Option<Decimal> {
    u.to_base
        .and_then(|f| Decimal::from_str(f).ok())
        .and_then(|f| v.checked_mul(f))
}

/// Compares two quantities. Missing units on both sides compare as plain numbers.
/// Inexact factors (psi, lb) compare with a relative tolerance of 1e-6.
pub fn compare(a: Decimal, ua: Option<&str>, b: Decimal, ub: Option<&str>) -> QtyCmp {
    let (pa, pb) = match (ua, ub) {
        (None, None) => {
            return if a == b {
                QtyCmp::Equal
            } else {
                QtyCmp::Different { a_base: a, b_base: b }
            };
        }
        (Some(x), Some(y)) => match (parse_unit(x), parse_unit(y)) {
            (Some(p), Some(q)) => (p, q),
            _ => return QtyCmp::UnknownUnit,
        },
        _ => return QtyCmp::UnknownUnit,
    };
    if pa.dim != pb.dim {
        return QtyCmp::Incompatible;
    }
    if pa.symbol == pb.symbol {
        return if a == b {
            QtyCmp::Equal
        } else {
            QtyCmp::Different { a_base: a, b_base: b }
        };
    }
    let (Some(x), Some(y)) = (to_base(a, &pa), to_base(b, &pb)) else {
        return QtyCmp::Incompatible; // e.g. HRC vs HB: no exact conversion
    };
    let tol = (x.abs().max(y.abs())) * Decimal::new(1, 6);
    if (x - y).abs() <= tol {
        QtyCmp::Equal
    } else {
        QtyCmp::Different { a_base: x, b_base: y }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn conversions_are_exact() {
        assert_eq!(compare(d("25.4"), Some("mm"), d("1"), Some("in")), QtyCmp::Equal);
        assert_eq!(compare(d("1.2"), Some("m"), d("1200"), Some("mm")), QtyCmp::Equal);
        assert_eq!(compare(d("120"), Some("mm"), d("2"), Some("kg")), QtyCmp::Incompatible);
        assert_eq!(
            compare(d("40"), Some("HRC"), d("380"), Some("HB")),
            QtyCmp::Incompatible
        );
        assert_eq!(
            compare(d("1"), Some("furlong"), d("1"), Some("mm")),
            QtyCmp::UnknownUnit
        );
        assert!(matches!(
            compare(d("120"), Some("mm"), d("121"), Some("mm")),
            QtyCmp::Different { .. }
        ));
        assert_eq!(compare(d("355"), Some("MPa"), d("355"), Some("N/mm2")), QtyCmp::Equal);
    }
}

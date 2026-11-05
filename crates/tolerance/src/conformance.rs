//! Conformity decision for a measured value with expanded measurement uncertainty U (ISO 14253-1:2017).
//!
//! The measurement is the interval [y − U, y + U]. ISO 14253-1 §5: conformance is proven only when the whole
//! interval lies inside the specification zone (equivalently: y lies in the *conformance zone*, the spec zone
//! shrunk by U on each limited side); nonconformance is proven only when the whole interval lies outside it
//! (y outside the spec zone widened by U). Everything else is indeterminate — neither party may claim a result.
//! All arithmetic is exact decimal.

use rust_decimal::Decimal;

use crate::error::{TolResult, invalid};
use crate::spec::{Bound, Spec};
use crate::units::{Quantity, display_in};

/// The three outcomes of ISO 14253-1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// Conformance proven: y ∈ conformance zone.
    Pass,
    /// Nonconformance proven: y outside the spec zone by more than U.
    Fail,
    /// Within U of a limit (or the conformance zone is empty).
    Indeterminate,
}

/// A decision with its exact margins and a one-line explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub outcome: Outcome,
    /// Conformance-zone limits (canonical unit); `None` on an unlimited side.
    pub conformance_lower: Option<Decimal>,
    pub conformance_upper: Option<Decimal>,
    /// True when 2U > the tolerance, so Pass is impossible whatever is measured.
    pub zone_empty: bool,
    pub explanation: String,
}

fn inside_low(b: &Bound, x: Decimal) -> bool {
    if b.inclusive { x >= b.value } else { x > b.value }
}

fn inside_high(b: &Bound, x: Decimal) -> bool {
    if b.inclusive { x <= b.value } else { x < b.value }
}

/// Decide conformance of `measured` (with expanded uncertainty `u`, coverage factor already applied) to `spec`.
pub fn judge(spec: &Spec, measured: Quantity, u: Quantity) -> TolResult<Verdict> {
    let y = measured.canonical_as(spec.dimension)?;
    let uu = u.canonical_as(spec.dimension)?;
    if uu.is_sign_negative() && !uu.is_zero() {
        return invalid(format!("expanded uncertainty {uu} is negative"));
    }
    let (lo_i, hi_i) = (y - uu, y + uu);
    let unit = spec.display_unit;
    let show = |v: Decimal| format!("{} {}", display_in(v, unit), unit.symbol());

    let pass_low = spec.lower.as_ref().is_none_or(|b| inside_low(b, lo_i));
    let pass_high = spec.upper.as_ref().is_none_or(|b| inside_high(b, hi_i));
    let fail_low = spec.lower.as_ref().is_some_and(|b| !inside_low(b, hi_i));
    let fail_high = spec.upper.as_ref().is_some_and(|b| !inside_high(b, lo_i));
    let conformance_lower = spec.lower.map(|b| b.value + uu);
    let conformance_upper = spec.upper.map(|b| b.value - uu);
    let zone_empty = matches!((conformance_lower, conformance_upper), (Some(l), Some(h)) if l > h);

    let interval = format!("{} ± {} (interval {} … {})", show(y), show(uu), show(lo_i), show(hi_i));
    let (outcome, explanation) = if fail_low || fail_high {
        let (side, lim) = if fail_low {
            ("below the lower", spec.lower)
        } else {
            ("above the upper", spec.upper)
        };
        let lim = lim.map(|b| show(b.value)).unwrap_or_default();
        (
            Outcome::Fail,
            format!("FAIL: {interval} lies entirely {side} limit {lim}; nonconformance proven per ISO 14253-1."),
        )
    } else if pass_low && pass_high {
        (
            Outcome::Pass,
            format!(
                "PASS: {interval} lies entirely inside {}; conformance proven per ISO 14253-1.",
                spec.describe()
            ),
        )
    } else if zone_empty {
        (
            Outcome::Indeterminate,
            format!(
                "INDETERMINATE: 2U = {} exceeds the tolerance {}, so the conformance zone is empty and no \
                 measurement with this uncertainty can prove conformance; reduce U.",
                show(uu + uu),
                spec.tolerance().map(show).unwrap_or_default()
            ),
        )
    } else {
        let lim = if !pass_low { spec.lower } else { spec.upper };
        let lim = lim.map(|b| show(b.value)).unwrap_or_default();
        (
            Outcome::Indeterminate,
            format!(
                "INDETERMINATE: {interval} straddles the limit {lim}; neither conformance nor nonconformance is proven (ISO 14253-1)."
            ),
        )
    };
    Ok(Verdict {
        outcome,
        conformance_lower,
        conformance_upper,
        zone_empty,
        explanation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::Unit;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).expect("test decimal")
    }

    fn q(s: &str, u: Unit) -> Quantity {
        Quantity::new(d(s), u).expect("q")
    }

    fn spec() -> Spec {
        Spec::symmetric(d("10"), d("0.05"), Unit::Mm).expect("spec") // 9.95 … 10.05
    }

    #[test]
    fn three_zones_hand_computed() {
        // U = 0.01 mm → conformance zone 9.96 … 10.04, nonconformance beyond 9.94 / 10.06.
        let u = q("10", Unit::Um);
        let cases = [
            ("10.00", Outcome::Pass),
            ("10.04", Outcome::Pass),
            ("10.041", Outcome::Indeterminate),
            ("10.05", Outcome::Indeterminate),
            ("10.06", Outcome::Indeterminate),
            ("10.0601", Outcome::Fail),
            ("9.96", Outcome::Pass),
            ("9.9599", Outcome::Indeterminate),
            ("9.9399", Outcome::Fail),
        ];
        for (y, want) in cases {
            let v = judge(&spec(), q(y, Unit::Mm), u).expect("verdict");
            assert_eq!(v.outcome, want, "{y}: {}", v.explanation);
        }
        let v = judge(&spec(), q("10", Unit::Mm), u).expect("v");
        assert_eq!(
            (v.conformance_lower, v.conformance_upper),
            (Some(d("9.96")), Some(d("10.04")))
        );
    }

    #[test]
    fn empty_conformance_zone_and_one_sided() {
        // 2U = T: the conformance zone is the single point 10.
        assert_eq!(
            judge(&spec(), q("10", Unit::Mm), q("0.05", Unit::Mm)).map(|v| v.outcome),
            Ok(Outcome::Pass)
        );
        let v = judge(&spec(), q("10", Unit::Mm), q("0.06", Unit::Mm)).expect("v");
        assert!(v.zone_empty && v.outcome == Outcome::Indeterminate, "{}", v.explanation);
        let tensile = Spec::at_least(d("400"), true, Unit::MPa).expect("s");
        assert_eq!(
            judge(&tensile, q("412", Unit::MPa), q("8", Unit::MPa)).map(|v| v.outcome),
            Ok(Outcome::Pass)
        );
        assert_eq!(
            judge(&tensile, q("0.405", Unit::GPa), q("8", Unit::MPa)).map(|v| v.outcome),
            Ok(Outcome::Indeterminate)
        );
        assert_eq!(
            judge(&tensile, q("391", Unit::NPerMm2), q("8", Unit::MPa)).map(|v| v.outcome),
            Ok(Outcome::Fail)
        );
        let lt = Spec::at_most(d("0.5"), false, Unit::Mm).expect("s");
        assert_eq!(
            judge(&lt, q("0.5", Unit::Mm), q("0", Unit::Mm)).map(|v| v.outcome),
            Ok(Outcome::Fail)
        );
        assert!(judge(&spec(), q("400", Unit::MPa), q("0", Unit::Mm)).is_err());
        assert!(judge(&spec(), q("10", Unit::Mm), q("-1", Unit::Um)).is_err());
    }

    #[test]
    fn explanation_names_the_numbers() {
        let v = judge(&spec(), q("10.055", Unit::Mm), q("10", Unit::Um)).expect("v");
        assert!(
            v.explanation.contains("10.05 mm") && v.explanation.starts_with("INDETERMINATE"),
            "{}",
            v.explanation
        );
    }
}

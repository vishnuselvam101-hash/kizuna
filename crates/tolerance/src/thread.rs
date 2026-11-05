//! ISO metric screw threads (`M8`, `M8×1.25 6g`).
//!
//! Coverage is deliberately narrow and table-backed:
//! * Coarse pitch when the pitch is omitted: ISO 261 / ISO 262 (M3–M24 series 1).
//! * Major-diameter limits of external threads of tolerance position g or h, grade 6 (the default class 6g / 6h):
//!   fundamental deviation es (ISO 965-1:2013 Table 3) and Td(6) (ISO 965-1:2013 Table 4).
//! * Basic pitch diameter d₂ = d − 0.649 519·P (ISO 724, H = 0.866 025·P, d₂ = d − ¾H, 6-decimal factor).
//!
//! Anything else (internal threads, other grades or positions, pitch-diameter tolerances) parses but returns
//! `Unsupported` for limits rather than a guessed value.

use rust_decimal::Decimal;

use crate::error::{TolResult, invalid, unsupported};
use crate::spec::Spec;
use crate::units::Unit;

/// (pitch in 0.01 mm, es for g in µm, Td grade 6 in µm).
const PITCH_TABLE: [(u16, i16, u16); 10] = [
    (50, -20, 106),
    (70, -22, 140),
    (80, -24, 150),
    (100, -26, 180),
    (125, -28, 212),
    (150, -32, 236),
    (175, -34, 265),
    (200, -38, 280),
    (250, -42, 335),
    (300, -48, 375),
];

/// ISO 261 coarse pitches (d in mm, P in 0.01 mm).
const COARSE: [(u8, u16); 10] = [
    (3, 50),
    (4, 70),
    (5, 80),
    (6, 100),
    (8, 125),
    (10, 150),
    (12, 175),
    (16, 200),
    (20, 250),
    (24, 300),
];

/// A parsed metric thread designation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThreadSpec {
    pub nominal_mm: Decimal,
    pub pitch_mm: Decimal,
    /// True when the pitch was not written and the ISO 261 coarse pitch was used.
    pub coarse_default: bool,
    /// Tolerance class as written, e.g. `6g` or `6H`; `None` when omitted.
    pub class: Option<String>,
}

impl ThreadSpec {
    /// Build from the written parts; looks up the coarse pitch when `pitch` is `None`.
    pub fn new(nominal_mm: Decimal, pitch: Option<Decimal>, class: Option<String>) -> TolResult<Self> {
        if nominal_mm <= Decimal::ZERO {
            return invalid("thread diameter must be positive");
        }
        let (pitch_mm, coarse_default) = match pitch {
            Some(p) if p > Decimal::ZERO => (p, false),
            Some(_) => return invalid("thread pitch must be positive"),
            None => {
                let p = COARSE
                    .iter()
                    .find(|(dd, _)| Decimal::from(*dd) == nominal_mm)
                    .map(|&(_, p)| Decimal::new(i64::from(p), 2));
                match p {
                    Some(p) => (p, true),
                    None => {
                        return unsupported(format!("no tabulated coarse pitch for M{nominal_mm}; write the pitch"));
                    }
                }
            }
        };
        Ok(ThreadSpec {
            nominal_mm,
            pitch_mm,
            coarse_default,
            class,
        })
    }

    /// Basic pitch diameter d₂ (mm, ISO 724 factor 0.649519).
    pub fn basic_pitch_diameter(&self) -> Decimal {
        self.nominal_mm - Decimal::new(649_519, 6) * self.pitch_mm
    }

    /// Major-diameter limits for external threads 6g / 6h (class defaults to 6g when omitted, ISO 965-1 §10).
    pub fn major_diameter_spec(&self) -> TolResult<Spec> {
        let class = self.class.as_deref().unwrap_or("6g");
        let es_g = match class {
            "6g" => true,
            "6h" => false,
            other => {
                return unsupported(format!(
                    "major-diameter limits are tabulated for 6g/6h only, not {other}"
                ));
            }
        };
        let p = (self.pitch_mm * Decimal::from(100)).normalize();
        let row = PITCH_TABLE
            .iter()
            .find(|(pp, _, _)| Decimal::from(*pp) == p)
            .map_or_else(
                || {
                    unsupported(format!(
                        "pitch {} mm is not in the ISO 965-1 table extract",
                        self.pitch_mm
                    ))
                },
                Ok,
            )?;
        let es = if es_g {
            Decimal::new(i64::from(row.1), 3)
        } else {
            Decimal::ZERO
        };
        let td = Decimal::new(i64::from(row.2), 3);
        Spec::asymmetric(self.nominal_mm, es, es - td, Unit::Mm)
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
    fn m8_6g_major_diameter() {
        // Published M8×1.25-6g major diameter: 7.972 / 7.760 mm.
        let t = ThreadSpec::new(d("8"), Some(d("1.25")), Some("6g".into())).expect("thread");
        let s = t.major_diameter_spec().expect("spec");
        assert_eq!(s.upper.map(|b| b.value), Some(d("7.972")));
        assert_eq!(s.lower.map(|b| b.value), Some(d("7.760")));
        assert_eq!(t.basic_pitch_diameter(), d("7.18810125"));
    }

    #[test]
    fn coarse_default_and_unsupported() {
        let t = ThreadSpec::new(d("10"), None, None).expect("thread");
        assert_eq!((t.pitch_mm, t.coarse_default), (d("1.5"), true));
        let s = t.major_diameter_spec().expect("6g default");
        assert_eq!(s.upper.map(|b| b.value), Some(d("9.968")));
        assert_eq!(s.lower.map(|b| b.value), Some(d("9.732")));
        assert!(
            ThreadSpec::new(d("8"), Some(d("1.25")), Some("6H".into()))
                .expect("t")
                .major_diameter_spec()
                .is_err()
        );
        assert!(ThreadSpec::new(d("7"), None, None).is_err());
    }
}

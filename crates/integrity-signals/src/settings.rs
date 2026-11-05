//! Per-tenant thresholds with hard bounds. Lower bounds keep every signal able to fire on a clear pattern; upper
//! bounds stop a tenant (or a stolen admin session) from switching a signal off with absurd values.

use std::collections::BTreeMap;
use std::str::FromStr;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Gauge / method resolution for one characteristic family: values of predicates starting with `prefix` and
/// reported in `unit` are expected to be whole multiples of `step`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resolution {
    pub prefix: String,
    #[serde(default)]
    pub unit: Option<String>,
    pub step: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Identical values across two lots: at least this many identical characteristics.
    pub min_identical_values: u32,
    /// ... and the chance of that many coincidences, under the supplier's own value history, at most this (ppm).
    pub max_coincidence_ppm: u32,
    /// A whole-table copy needs at least this many characteristics, all identical.
    pub min_table_values: u32,
    /// Narrow band / at-limit need at least this many values of one characteristic.
    pub min_samples: u32,
    /// Narrow band: spread of all values at most this percent of the tolerance width.
    pub narrow_band_pct: u32,
    /// At limit: at least this percent of values at (or within `limit_steps` resolution steps of) a spec limit.
    pub at_limit_pct: u32,
    pub limit_steps: u32,
    /// Excess precision: at least this many values, and this percent of values, finer than the resolution.
    pub min_excess_values: u32,
    pub excess_pct: u32,
    /// Chemistry: a composition summing to more than 100 % + this (thousandths of a percent) is impossible.
    pub chem_sum_tolerance_milli: u32,
    /// Dates: differences up to this many days are not reported.
    pub date_grace_days: u32,
    /// Lot sequence: only inversions between certificates at least this many days apart.
    pub lot_min_days: u32,
    /// Revision drift: this many fields present in only one version are treated as an extraction gap.
    pub drift_extraction_gap: u32,
    /// Revision drift: both versions need at least this many extracted fields to be compared.
    pub drift_min_fields: u32,
    /// Revision drift: also report a changed file whose extracted content is identical (re-scans; off by default).
    pub drift_report_file_only: bool,
    pub resolutions: Vec<Resolution>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            min_identical_values: 4,
            max_coincidence_ppm: 1000,
            min_table_values: 5,
            min_samples: 10,
            narrow_band_pct: 5,
            at_limit_pct: 30,
            limit_steps: 1,
            min_excess_values: 3,
            excess_pct: 50,
            chem_sum_tolerance_milli: 500,
            date_grace_days: 0,
            lot_min_days: 14,
            drift_extraction_gap: 1,
            drift_min_fields: 3,
            drift_report_file_only: false,
            resolutions: vec![
                Resolution {
                    prefix: "qa.measured.".into(),
                    unit: Some("mm".into()),
                    step: "0.001".into(),
                },
                Resolution {
                    prefix: "qa.chem.".into(),
                    unit: Some("%".into()),
                    step: "0.001".into(),
                },
                Resolution {
                    prefix: "qa.property.tensile_strength".into(),
                    unit: Some("MPa".into()),
                    step: "1".into(),
                },
                Resolution {
                    prefix: "qa.property.yield_strength".into(),
                    unit: Some("MPa".into()),
                    step: "1".into(),
                },
            ],
        }
    }
}

/// (name, low, high, "higher is less sensitive").
const BOUNDS: [(&str, u32, u32, bool); 14] = [
    ("min_identical_values", 2, 20, true),
    ("max_coincidence_ppm", 1, 50_000, false),
    ("min_table_values", 3, 50, true),
    ("min_samples", 5, 200, true),
    ("narrow_band_pct", 1, 25, false),
    ("at_limit_pct", 10, 90, true),
    ("limit_steps", 0, 10, false),
    ("min_excess_values", 1, 50, true),
    ("excess_pct", 10, 100, true),
    ("chem_sum_tolerance_milli", 0, 3000, true),
    ("date_grace_days", 0, 30, true),
    ("lot_min_days", 1, 365, true),
    ("drift_extraction_gap", 0, 5, true),
    ("drift_min_fields", 1, 20, true),
];

pub const MAX_RESOLUTIONS: usize = 50;

impl Settings {
    pub fn bounds() -> BTreeMap<&'static str, [u32; 2]> {
        BOUNDS.iter().map(|(k, lo, hi, _)| (*k, [*lo, *hi])).collect()
    }

    fn values(&self) -> [u32; 14] {
        [
            self.min_identical_values,
            self.max_coincidence_ppm,
            self.min_table_values,
            self.min_samples,
            self.narrow_band_pct,
            self.at_limit_pct,
            self.limit_steps,
            self.min_excess_values,
            self.excess_pct,
            self.chem_sum_tolerance_milli,
            self.date_grace_days,
            self.lot_min_days,
            self.drift_extraction_gap,
            self.drift_min_fields,
        ]
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        for (v, (_, lo, hi, _)) in self.values().iter().zip(BOUNDS) {
            if *v < lo || *v > hi {
                return Err("threshold_out_of_bounds");
            }
        }
        if self.resolutions.len() > MAX_RESOLUTIONS {
            return Err("too_many_resolutions");
        }
        for r in &self.resolutions {
            let ok_prefix = (2..=80).contains(&r.prefix.len())
                && r.prefix
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
            let ok_unit = r.unit.as_ref().is_none_or(|u| (1..=16).contains(&u.chars().count()));
            let step = Decimal::from_str(&r.step).map_err(|_| "invalid_resolution")?;
            if !ok_prefix || !ok_unit || step <= Decimal::ZERO || step > Decimal::from(1000) {
                return Err("invalid_resolution");
            }
        }
        Ok(())
    }

    /// Less sensitive than `old` in any threshold, or file-only drift switched off, or a resolution removed.
    pub fn weaker_than(&self, old: &Settings) -> bool {
        let raised = self
            .values()
            .iter()
            .zip(old.values())
            .zip(BOUNDS)
            .any(|((new, old), (_, _, _, higher_weaker))| if higher_weaker { *new > old } else { *new < old });
        let removed = old.resolutions.iter().any(|r| !self.resolutions.contains(r));
        raised || removed || (old.drift_report_file_only && !self.drift_report_file_only)
    }

    /// Resolution for a predicate and unit: the longest matching prefix with the same unit.
    pub fn resolution_for(&self, predicate: &str, unit: Option<&str>) -> Option<Decimal> {
        self.resolutions
            .iter()
            .filter(|r| predicate.starts_with(&r.prefix) && r.unit.as_deref() == unit)
            .max_by_key(|r| r.prefix.len())
            .and_then(|r| Decimal::from_str(&r.step).ok())
            .filter(|s| *s > Decimal::ZERO)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_within_bounds() {
        assert_eq!(Settings::default().validate(), Ok(()));
        assert_eq!(Settings::bounds().len(), BOUNDS.len());
    }

    #[test]
    fn out_of_bounds_and_bad_resolutions_are_refused() {
        let s = Settings {
            at_limit_pct: 95,
            ..Settings::default()
        };
        assert_eq!(s.validate(), Err("threshold_out_of_bounds"));
        let mut s = Settings::default();
        s.resolutions[0].step = "0".into();
        assert_eq!(s.validate(), Err("invalid_resolution"));
        s.resolutions[0].step = "x".into();
        assert_eq!(s.validate(), Err("invalid_resolution"));
        let mut s = Settings::default();
        s.resolutions[0].prefix = "QA; drop".into();
        assert_eq!(s.validate(), Err("invalid_resolution"));
    }

    #[test]
    fn unknown_keys_are_refused_and_missing_keys_default() {
        assert!(serde_json::from_str::<Settings>(r#"{"disable_all": true}"#).is_err());
        let s: Settings = serde_json::from_str(r#"{"min_samples": 12}"#).unwrap();
        assert_eq!(s.min_samples, 12);
        assert_eq!(s.at_limit_pct, 30);
    }

    #[test]
    fn weakening_is_detected_in_the_right_direction() {
        let d = Settings::default();
        let looser = Settings {
            min_samples: 50,
            ..d.clone()
        };
        let stricter = Settings {
            min_samples: 6,
            narrow_band_pct: 10,
            ..d.clone()
        };
        let looser_band = Settings {
            narrow_band_pct: 1,
            ..d.clone()
        };
        assert!(looser.weaker_than(&d));
        assert!(!stricter.weaker_than(&d));
        assert!(looser_band.weaker_than(&d));
        let mut fewer = d.clone();
        fewer.resolutions.pop();
        assert!(fewer.weaker_than(&d));
    }

    #[test]
    fn resolution_uses_longest_prefix_and_unit() {
        let mut s = Settings::default();
        s.resolutions.push(Resolution {
            prefix: "qa.measured.diameter".into(),
            unit: Some("mm".into()),
            step: "0.01".into(),
        });
        assert_eq!(
            s.resolution_for("qa.measured.diameter", Some("mm")),
            Decimal::from_str("0.01").ok()
        );
        assert_eq!(
            s.resolution_for("qa.measured.length", Some("mm")),
            Decimal::from_str("0.001").ok()
        );
        assert_eq!(s.resolution_for("qa.measured.length", Some("in")), None);
    }
}

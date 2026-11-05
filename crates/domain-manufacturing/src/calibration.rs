//! Confidence calibration (`cal@1`): maps a mapped value's provenance (text layer or OCR, rule / tenant synonym /
//! fallback label, OCR word confidence) to the empirical probability that the value is correct, and decides
//! whether it must be confirmed by a person before it can back a decision.
//!
//! The table (`calibration.json`) is fitted offline by `services/extraction/tests/ja_calibrate.py`: per group,
//! isotonic regression (pool-adjacent-violators, deterministic) of correctness against OCR confidence on the
//! train + dev splits of the Japanese corpus, then the review gate is the smallest threshold that flags at least
//! 90 % of the wrong values on dev. The independent holdout is never used for fitting. Lookups are pure.

use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Table {
    pub version: String,
    /// Values with a calibrated probability below this need confirmation.
    pub gate_bps: u32,
    /// group → steps `[upper OCR confidence bps (inclusive), p_correct bps]`, ascending.
    pub groups: std::collections::BTreeMap<String, Vec<[u32; 2]>>,
}

pub fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| serde_json::from_str(include_str!("calibration.json")).expect("calibration.json is valid"))
}

/// Calibration group of a value: `text|ocr` / `rule|tenant_synonym|fallback` (a guessed subject counts as fallback).
pub fn group(ocr: bool, label_basis: &str, subject_fallback: bool) -> String {
    let basis = if subject_fallback { "fallback" } else { label_basis };
    format!("{}/{basis}", if ocr { "ocr" } else { "text" })
}

/// Calibrated probability (bps) that a value of `group` with OCR confidence `conf_bps` is correct. Unknown groups
/// fall back to `…/fallback` of the same source, then 0 (always reviewed).
pub fn p_correct_bps(group: &str, conf_bps: Option<u32>) -> u32 {
    let t = table();
    let steps = t.groups.get(group).or_else(|| {
        let src = group.split('/').next().unwrap_or("ocr");
        t.groups.get(&format!("{src}/fallback"))
    });
    let Some(steps) = steps else { return 0 };
    let c = conf_bps.unwrap_or(10_000).min(10_000);
    steps
        .iter()
        .find(|[upper, _]| c <= *upper)
        .or(steps.last())
        .map_or(0, |[_, p]| (*p).min(10_000))
}

pub fn needs_review(p_bps: u32) -> bool {
    p_bps < table().gate_bps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_monotone_and_complete() {
        let t = table();
        assert!(t.version.starts_with("cal@"));
        assert!(t.gate_bps <= 10_000);
        for (g, steps) in &t.groups {
            assert!(!steps.is_empty(), "{g}");
            assert_eq!(steps.last().unwrap()[0], 10_000, "{g}: last step covers confidence 1.0");
            for w in steps.windows(2) {
                assert!(w[0][0] < w[1][0], "{g}: ascending confidence");
                assert!(w[0][1] <= w[1][1], "{g}: isotonic (non-decreasing) probability");
            }
        }
        for src in ["text", "ocr"] {
            assert!(t.groups.contains_key(&format!("{src}/rule")), "{src}/rule");
            assert!(t.groups.contains_key(&format!("{src}/fallback")), "{src}/fallback");
        }
    }

    #[test]
    fn lookups_are_monotone_in_confidence() {
        for g in table().groups.keys() {
            let mut prev = 0;
            for c in (0..=10_000).step_by(250) {
                let p = p_correct_bps(g, Some(c));
                assert!(p >= prev, "{g} at {c}");
                prev = p;
            }
        }
        assert_eq!(group(true, "rule", true), "ocr/fallback");
        assert_eq!(p_correct_bps("nonsense", None), 0);
    }
}

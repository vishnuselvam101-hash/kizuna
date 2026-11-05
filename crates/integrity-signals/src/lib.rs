//! Certificate integrity signals (market research I3) and revision drift radar (I2).
//!
//! Both engines are deterministic and pure: they read already-extracted claims and return findings with an
//! explanation, the evidence locations and a *strength* (how pronounced the pattern is). A strength is not a
//! probability of wrongdoing. Every finding is routed to human review; nothing here rejects evidence, changes a
//! claim or decides anything. Wording is neutral: it describes the data pattern and names benign causes.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub mod cert;
pub mod drift;
pub mod settings;
pub mod synth;

pub use settings::Settings;

pub const ENGINE_VERSION: &str = "integrity-signals@1";

/// How pronounced a pattern is. Ordering: weak < moderate < strong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    Weak,
    Moderate,
    Strong,
}

impl Strength {
    pub fn as_str(self) -> &'static str {
        match self {
            Strength::Weak => "weak",
            Strength::Moderate => "moderate",
            Strength::Strong => "strong",
        }
    }
}

/// One place in the evidence that a finding rests on.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvidenceRef {
    /// Artifact (document) id.
    pub artifact: String,
    pub document: String,
    pub claim: Option<String>,
    pub field: Option<String>,
    pub value: Option<String>,
    pub location: Value,
}

/// Stable id of a finding (kind + the things it is about), used to deduplicate across runs.
pub fn fingerprint(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0x1f]);
    }
    hex::encode(h.finalize())
}

/// Days since 1970-01-01 of an ISO date `YYYY-MM-DD` (proleptic Gregorian); None if it is not a real date.
pub fn day_number(date: &str) -> Option<i64> {
    let b = date.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y: i64 = date.get(0..4)?.parse().ok()?;
    let m: i64 = date.get(5..7)?.parse().ok()?;
    let d: i64 = date.get(8..10)?.parse().ok()?;
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let dim = match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1..=12 => 31,
        _ => return None,
    };
    if !(1..=dim).contains(&d) {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_numbers() {
        assert_eq!(day_number("1970-01-01"), Some(0));
        assert_eq!(day_number("2026-11-05"), Some(20_762));
        assert_eq!(
            day_number("2028-02-29")
                .zip(day_number("2028-03-01"))
                .map(|(a, b)| b - a),
            Some(1)
        );
        for bad in ["2026-02-29", "2026-13-01", "2026-1-01", "20261105", ""] {
            assert_eq!(day_number(bad), None, "{bad}");
        }
    }

    #[test]
    fn fingerprints_separate_fields() {
        assert_ne!(fingerprint(&["ab", "c"]), fingerprint(&["a", "bc"]));
        assert_eq!(fingerprint(&["x"]).len(), 64);
    }
}

//! Generic, deterministic conflict detection machinery: facts, quantities with units, revision ordering,
//! findings and the rule trait. Domain packs (e.g. `domain-manufacturing`) supply the rules.
//! Conflicting facts are never dropped or merged: a finding only references them and explains why.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::Value;

pub mod units;

/// A current (not superseded, not rejected) claim, reduced to what rules need.
#[derive(Debug, Clone, PartialEq)]
pub struct Fact {
    pub id: String,
    pub subject: String,
    pub predicate: String,
    pub text: Option<String>,
    pub num: Option<Decimal>,
    pub unit: Option<String>,
    pub revision: Option<String>,
    pub scope: BTreeMap<String, String>,
    /// Artifact kind the claim came from (`spec_document`, `purchase_order`, ...) or `manual`.
    pub source_kind: String,
}

impl Fact {
    pub fn display_value(&self) -> String {
        match (&self.num, &self.text, &self.unit) {
            (Some(n), _, Some(u)) => format!("{n} {u}"),
            (Some(n), _, None) => n.to_string(),
            (None, Some(t), _) => t.clone(),
            _ => String::new(),
        }
    }

    pub fn text_norm(&self) -> Option<String> {
        self.text
            .as_ref()
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub kind: &'static str,
    pub subject: String,
    pub claim_a: String,
    pub claim_b: Option<String>,
    pub explanation: Value,
}

pub struct Ctx {
    /// Reference date `YYYY-MM-DD` for expiry checks.
    pub today: String,
}

pub trait Rule: Send + Sync {
    fn name(&self) -> &'static str;
    fn detect(&self, subject: &str, facts: &[&Fact], ctx: &Ctx) -> Vec<Finding>;
}

/// Runs every rule per subject; findings are de-duplicated on (kind, unordered claim pair) and sorted.
pub fn run(rules: &[&dyn Rule], facts: &[Fact], ctx: &Ctx) -> Vec<Finding> {
    let mut by_subject: BTreeMap<&str, Vec<&Fact>> = BTreeMap::new();
    for f in facts {
        by_subject.entry(f.subject.as_str()).or_default().push(f);
    }
    let mut out: Vec<Finding> = Vec::new();
    for (subject, mut fs) in by_subject {
        fs.sort_by(|a, b| a.id.cmp(&b.id));
        for r in rules {
            for mut f in r.detect(subject, &fs, ctx) {
                if let Some(b) = f.claim_b.clone()
                    && b < f.claim_a
                {
                    f.claim_b = Some(std::mem::replace(&mut f.claim_a, b));
                }
                if !out
                    .iter()
                    .any(|o| o.kind == f.kind && o.claim_a == f.claim_a && o.claim_b == f.claim_b)
                {
                    out.push(f);
                }
            }
        }
    }
    out.sort_by(|a, b| (a.subject.as_str(), a.kind, &a.claim_a).cmp(&(b.subject.as_str(), b.kind, &b.claim_a)));
    out
}

/// Orders revision labels when both are numeric or both alphabetic (A < B < ... < Z < AA). Otherwise unknown.
pub fn cmp_revision(a: &str, b: &str) -> Option<Ordering> {
    let (a, b) = (a.trim().to_uppercase(), b.trim().to_uppercase());
    if a.is_empty() || b.is_empty() {
        return None;
    }
    if let (Ok(x), Ok(y)) = (a.parse::<u64>(), b.parse::<u64>()) {
        return Some(x.cmp(&y));
    }
    if a.chars().all(|c| c.is_ascii_uppercase()) && b.chars().all(|c| c.is_ascii_uppercase()) {
        return Some(a.len().cmp(&b.len()).then(a.cmp(&b)));
    }
    (a == b).then_some(Ordering::Equal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_ordering() {
        assert_eq!(cmp_revision("B", "C"), Some(Ordering::Less));
        assert_eq!(cmp_revision("Z", "AA"), Some(Ordering::Less));
        assert_eq!(cmp_revision("10", "9"), Some(Ordering::Greater));
        assert_eq!(cmp_revision("c", "C"), Some(Ordering::Equal));
        assert_eq!(cmp_revision("B2", "C"), None);
    }
}

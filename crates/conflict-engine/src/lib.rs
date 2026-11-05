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
    run_parallel(rules, facts, ctx, 1)
}

/// Facts grouped by subject (ordered), each group sorted by claim id: the unit of work of a run.
pub fn by_subject(facts: &[Fact]) -> Vec<(&str, Vec<&Fact>)> {
    let mut m: BTreeMap<&str, Vec<&Fact>> = BTreeMap::new();
    for f in facts {
        m.entry(f.subject.as_str()).or_default().push(f);
    }
    m.into_iter()
        .map(|(s, mut fs)| {
            fs.sort_by(|a, b| a.id.cmp(&b.id));
            (s, fs)
        })
        .collect()
}

fn detect_subject(rules: &[&dyn Rule], subject: &str, fs: &[&Fact], ctx: &Ctx, out: &mut Vec<Finding>) {
    for r in rules {
        for mut f in r.detect(subject, fs, ctx) {
            if let Some(b) = f.claim_b.clone()
                && b < f.claim_a
            {
                f.claim_b = Some(std::mem::replace(&mut f.claim_a, b));
            }
            out.push(f);
        }
    }
}

/// Same output as [`run`], with subjects evaluated on up to `threads` worker threads (contiguous chunks of the
/// ordered subject list; results are merged in subject order, so the output does not depend on `threads`).
pub fn run_parallel(rules: &[&dyn Rule], facts: &[Fact], ctx: &Ctx, threads: usize) -> Vec<Finding> {
    let groups = by_subject(facts);
    let threads = threads.clamp(1, groups.len().max(1));
    let chunk = groups.len().div_ceil(threads).max(1);
    let eval = |part: &[(&str, Vec<&Fact>)]| {
        let mut out = Vec::new();
        for (s, fs) in part {
            detect_subject(rules, s, fs, ctx, &mut out);
        }
        out
    };
    let parts: Vec<Vec<Finding>> = if threads == 1 {
        vec![eval(&groups)]
    } else {
        std::thread::scope(|sc| {
            let hs: Vec<_> = groups.chunks(chunk).map(|c| sc.spawn(move || eval(c))).collect();
            hs.into_iter()
                .map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
                .collect()
        })
    };
    // First occurrence wins, exactly as a sequential pass in subject order.
    let mut seen: std::collections::HashSet<(&'static str, String, Option<String>)> = std::collections::HashSet::new();
    let mut out: Vec<Finding> = Vec::new();
    for f in parts.into_iter().flatten() {
        if seen.insert((f.kind, f.claim_a.clone(), f.claim_b.clone())) {
            out.push(f);
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

    /// A missing revision is never ordered or equal (mutation-testing kill).
    #[test]
    fn empty_revisions_are_incomparable() {
        assert_eq!(cmp_revision("", ""), None);
        assert_eq!(cmp_revision("  ", "A"), None);
        assert_eq!(cmp_revision("7", ""), None);
    }

    struct Twice;
    impl Rule for Twice {
        fn name(&self) -> &'static str {
            "twice"
        }
        fn detect(&self, subject: &str, _: &[&Fact], _: &Ctx) -> Vec<Finding> {
            let f = |a: &str, b: &str| Finding {
                kind: "k",
                subject: subject.into(),
                claim_a: a.into(),
                claim_b: Some(b.into()),
                explanation: Value::Null,
            };
            vec![f("c1", "c2"), f("c2", "c1"), f("c1", "c2")]
        }
    }

    /// The same pair reported twice (in either order) is one finding, normalised to claim_a < claim_b.
    #[test]
    fn duplicate_findings_are_merged() {
        let fact = Fact {
            id: "c1".into(),
            subject: "P-1".into(),
            predicate: "p".into(),
            text: None,
            num: None,
            unit: None,
            revision: None,
            scope: BTreeMap::new(),
            source_kind: "s".into(),
        };
        let out = run(
            &[&Twice],
            &[fact],
            &Ctx {
                today: "2026-11-05".into(),
            },
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!((out[0].claim_a.as_str(), out[0].claim_b.as_deref()), ("c1", Some("c2")));
    }
}

//! Selective invalidation (directive §8). Given one semantic change to evidence, find the decisions that depend on
//! it, decide materiality per dependency, and propagate only material impacts through decision→decision edges.
//! Nothing is invalidated by blanket downstream traversal; decisions are flagged, never re-decided.

use std::collections::{BTreeMap, VecDeque};
use std::str::FromStr;

use conflict_engine::units::{QtyCmp, compare, parse_unit, to_base};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub mod cover;

pub const ENGINE_VERSION: &str = "dep-engine@1";

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub text: Option<String>,
    pub num: Option<String>,
    pub unit: Option<String>,
    pub revision: Option<String>,
    #[serde(default)]
    pub scope: BTreeMap<String, String>,
}

impl Snapshot {
    fn display(&self) -> String {
        match (&self.num, &self.text, &self.unit) {
            (Some(n), _, Some(u)) => format!("{n} {u}"),
            (Some(n), _, None) => n.clone(),
            (None, Some(t), _) => t.clone(),
            _ => String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Materiality {
    /// Any change of value (unit-aware: 25.4 mm == 1 in) is material.
    AnyChange,
    /// Numeric changes up to `threshold` (in the snapshot's unit) are immaterial.
    NumericAbs { threshold: String },
    /// Changes are recorded but never flag the decision.
    Informational,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dep {
    pub decision: String,
    pub claim: Option<String>,
    pub on_decision: Option<String>,
    pub subject: String,
    pub predicate: Option<String>,
    pub applicability: BTreeMap<String, String>,
    pub materiality: Materiality,
    pub snapshot: Snapshot,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// A claim was corrected by a newer claim.
    Superseded {
        old_claim: String,
        new_claim: String,
        new: Snapshot,
    },
    /// A claim is no longer usable evidence (e.g. rejected in review).
    Withdrawn { old_claim: String, reason: String },
    /// A new claim about a subject/predicate appeared (not a correction).
    NewEvidence {
        new_claim: String,
        subject: String,
        predicate: String,
        new: Snapshot,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Impact {
    pub decision: String,
    pub material: bool,
    pub cause_kind: &'static str,
    pub reason: String,
    pub path: Vec<String>,
    pub semantic_change: Value,
}

fn norm_text(s: &Option<String>) -> Option<String> {
    s.as_ref()
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase())
}

/// Returns (material, explanation).
pub fn materiality(m: &Materiality, old: &Snapshot, new: &Snapshot) -> (bool, String) {
    if *m == Materiality::Informational {
        return (false, "informational dependency".into());
    }
    let dec = |s: &Option<String>| s.as_deref().and_then(|v| Decimal::from_str(v).ok());
    match (dec(&old.num), dec(&new.num)) {
        (Some(a), Some(b)) => match compare(a, old.unit.as_deref(), b, new.unit.as_deref()) {
            QtyCmp::Equal => (false, "same quantity (unit-aware)".into()),
            QtyCmp::Different { a_base, b_base } => match m {
                Materiality::NumericAbs { threshold } => {
                    let t = Decimal::from_str(threshold).unwrap_or(Decimal::ZERO);
                    let t_base = match old.unit.as_deref().and_then(parse_unit) {
                        Some(u) => to_base(t, &u).unwrap_or(t),
                        None => t,
                    };
                    let diff = (a_base - b_base).abs();
                    if diff > t_base {
                        (true, format!("change exceeds threshold {threshold}"))
                    } else {
                        (false, format!("change within threshold {threshold}"))
                    }
                }
                _ => (true, "value changed".into()),
            },
            QtyCmp::Incompatible | QtyCmp::UnknownUnit => (true, "units no longer comparable".into()),
        },
        (None, None) if norm_text(&old.text) == norm_text(&new.text) => (false, "same value".into()),
        _ => (true, "value changed".into()),
    }
}

fn applicable(app: &BTreeMap<String, String>, new: &Snapshot) -> bool {
    app.iter().all(|(k, v)| {
        if k == "revision" {
            new.revision.as_ref().is_none_or(|r| r.eq_ignore_ascii_case(v))
        } else {
            new.scope.get(k).is_none_or(|x| x.eq_ignore_ascii_case(v))
        }
    })
}

pub fn assess(deps: &[Dep], change: &Change) -> Vec<Impact> {
    let mut direct: Vec<Impact> = Vec::new();
    for d in deps {
        let imp = match change {
            Change::Superseded {
                old_claim,
                new_claim,
                new,
            } if d.claim.as_deref() == Some(old_claim) => {
                // A correction that moves the claim to another revision or applicability scope is material
                // regardless of the value (review finding M1).
                let up = |r: &Option<String>| r.as_deref().map(str::to_uppercase);
                let moved = up(&d.snapshot.revision) != up(&new.revision)
                    || d.snapshot.scope != new.scope
                    || !applicable(&d.applicability, new);
                let (material, why) = if moved && d.materiality != Materiality::Informational {
                    (true, "applicability changed (revision or scope)".to_string())
                } else {
                    materiality(&d.materiality, &d.snapshot, new)
                };
                Some(Impact {
                    decision: d.decision.clone(),
                    material,
                    cause_kind: "superseded",
                    reason: format!("evidence corrected: {why}"),
                    path: vec![old_claim.clone(), new_claim.clone(), d.decision.clone()],
                    semantic_change: json!({ "from": d.snapshot.display(), "to": new.display(), "predicate": d.predicate }),
                })
            }
            Change::Withdrawn { old_claim, reason } if d.claim.as_deref() == Some(old_claim) => Some(Impact {
                decision: d.decision.clone(),
                material: d.materiality != Materiality::Informational,
                cause_kind: "withdrawn",
                reason: format!("evidence withdrawn: {reason}"),
                path: vec![old_claim.clone(), d.decision.clone()],
                semantic_change: json!({ "from": d.snapshot.display(), "to": null, "predicate": d.predicate }),
            }),
            Change::NewEvidence {
                new_claim,
                subject,
                predicate,
                new,
            } if d.claim.is_some()
                && d.claim.as_deref() != Some(new_claim)
                && &d.subject == subject
                && d.predicate.as_deref() == Some(predicate)
                && applicable(&d.applicability, new) =>
            {
                let (material, why) = materiality(&d.materiality, &d.snapshot, new);
                // Confirming evidence (same value) is not a change at all.
                (why != "same quantity (unit-aware)" && why != "same value").then(|| Impact {
                    decision: d.decision.clone(),
                    material,
                    cause_kind: "new_evidence",
                    reason: format!("new applicable evidence disagrees: {why}"),
                    path: vec![new_claim.clone(), d.decision.clone()],
                    semantic_change: json!({ "from": d.snapshot.display(), "to": new.display(), "predicate": predicate }),
                })
            }
            _ => None,
        };
        if let Some(i) = imp {
            direct.push(i);
        }
    }
    // One impact per decision: material wins over immaterial.
    let mut by_decision: BTreeMap<String, Impact> = BTreeMap::new();
    for i in direct {
        match by_decision.get(&i.decision) {
            Some(prev) if prev.material || !i.material => {}
            _ => {
                by_decision.insert(i.decision.clone(), i);
            }
        }
    }
    propagate(deps, by_decision)
}

/// Decisions that depend on a revoked/withdrawn decision.
pub fn upstream_withdrawn(deps: &[Dep], decision: &str, reason: &str) -> Vec<Impact> {
    let mut seeds = BTreeMap::new();
    for d in deps.iter().filter(|d| d.on_decision.as_deref() == Some(decision)) {
        let material = d.materiality != Materiality::Informational;
        if seeds.get(&d.decision).is_some_and(|p: &Impact| p.material || !material) {
            continue;
        }
        seeds.insert(
            d.decision.clone(),
            Impact {
                decision: d.decision.clone(),
                material,
                cause_kind: "upstream_decision",
                reason: format!("upstream decision {decision} {reason}"),
                path: vec![decision.to_string(), d.decision.clone()],
                semantic_change: json!({ "upstream_decision": decision }),
            },
        );
    }
    propagate(deps, seeds)
}

/// Propagates material impacts breadth-first through decision→decision dependencies.
fn propagate(deps: &[Dep], mut by_decision: BTreeMap<String, Impact>) -> Vec<Impact> {
    let mut queue: VecDeque<(String, Vec<String>)> = by_decision
        .values()
        .filter(|i| i.material)
        .map(|i| (i.decision.clone(), i.path.clone()))
        .collect();
    while let Some((dec, path)) = queue.pop_front() {
        for d in deps.iter().filter(|d| d.on_decision.as_deref() == Some(&dec)) {
            let material = d.materiality != Materiality::Informational;
            // Each decision is queued at most once (when it first becomes material), so cycles terminate.
            if by_decision
                .get(&d.decision)
                .is_some_and(|prev| prev.material || !material)
            {
                continue;
            }
            let mut p = path.clone();
            p.push(d.decision.clone());
            by_decision.insert(
                d.decision.clone(),
                Impact {
                    decision: d.decision.clone(),
                    material,
                    cause_kind: "upstream_decision",
                    reason: format!("depends on potentially affected decision {dec}"),
                    path: p.clone(),
                    semantic_change: json!({ "upstream_decision": dec }),
                },
            );
            if material {
                queue.push_back((d.decision.clone(), p));
            }
        }
    }
    by_decision.into_values().collect()
}

#[cfg(test)]
mod tests;

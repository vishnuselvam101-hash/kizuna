//! Frozen v1 selective-invalidation algorithm (dep-engine@1, linear scan per queued decision: O(queue x deps)).
//! Kept only as the oracle for the equivalence property test and the old-vs-new benchmark; not used by the server.

use std::collections::{BTreeMap, VecDeque};

use kizuna_messages::Msg;
use serde_json::json;

use crate::{Change, Dep, Impact, Materiality, applicable, materiality};

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
                    (true, Msg::new("materiality.applicability_changed"))
                } else {
                    materiality(&d.materiality, &d.snapshot, new)
                };
                Some(Impact {
                    decision: d.decision.clone(),
                    material,
                    cause_kind: "superseded",
                    reason_msg: Msg::new("invalidation.superseded").with_msg("why", why.clone()),
                    reason: String::new(),
                    path: vec![old_claim.clone(), new_claim.clone(), d.decision.clone()],
                    semantic_change: json!({ "from": d.snapshot.display(), "to": new.display(), "predicate": d.predicate }),
                })
            }
            Change::Withdrawn { old_claim, reason } if d.claim.as_deref() == Some(old_claim) => Some(Impact {
                decision: d.decision.clone(),
                material: d.materiality != Materiality::Informational,
                cause_kind: "withdrawn",
                reason_msg: Msg::new("invalidation.withdrawn").with_msg("reason", reason.clone()),
                reason: String::new(),
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
                (why.code != "materiality.same_quantity" && why.code != "materiality.same_value").then(|| Impact {
                    decision: d.decision.clone(),
                    material,
                    cause_kind: "new_evidence",
                    reason_msg: Msg::new("invalidation.new_evidence").with_msg("why", why.clone()),
                reason: String::new(),
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
pub fn upstream_withdrawn(deps: &[Dep], decision: &str, reason: &Msg) -> Vec<Impact> {
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
                reason_msg: Msg::new("invalidation.upstream")
                    .with("decision", decision)
                    .with_msg("verb", reason.clone()),
                reason: String::new(),
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
                    reason_msg: Msg::new("invalidation.depends_on").with("decision", dec.as_str()),
                    reason: String::new(),
                    path: p.clone(),
                    semantic_change: json!({ "upstream_decision": dec }),
                },
            );
            if material {
                queue.push_back((d.decision.clone(), p));
            }
        }
    }
    by_decision.into_values().map(Impact::worded).collect()
}

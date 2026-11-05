#![allow(clippy::unwrap_used)]
//! Property: incremental re-evaluation (only the subjects a change touched) composed over a change sequence gives
//! exactly the findings of a full run, and the parallel runner equals the sequential one.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use conflict_engine::{Ctx, Fact, Finding, Rule, run, run_parallel};
use domain_manufacturing::rules::all_rules;
use proptest::prelude::*;
use rust_decimal::Decimal;

const PREDS: &[(&str, &[&str])] = &[
    ("mfg.revision", &["B", "C", "D"]),
    ("mfg.dimension.length", &["120", "125", "4.72"]),
    ("mfg.tolerance.length.plus", &["0.2", "0.5"]),
    ("mfg.tolerance.length.minus", &["0.2"]),
    ("mfg.approval.status", &["approved", "rejected"]),
    ("mfg.certificate.expiry", &["2024-01-01", "2031-01-01"]),
    ("mfg.material", &["S355JR", "SUS304"]),
    ("po.revision_ordered", &["B", "C", "D"]),
    ("po.supplier", &["ACME", "Fuji"]),
    ("qa.revision_inspected", &["C", "D"]),
    ("qa.measured.length", &["120.05", "120.9", "119.1"]),
    ("qa.result", &["pass", "fail"]),
];
const KINDS: &[&str] = &[
    "spec_document",
    "drawing",
    "purchase_order",
    "inspection_report",
    "manual",
];

type Spec = (u8, u8, u8, u8, u8, u8, u8);

fn fact(id: usize, s: Spec) -> Fact {
    let (subj, p, v, unit, rev, kind, lot) = s;
    let (pred, vals) = PREDS[p as usize % PREDS.len()];
    let val = vals[v as usize % vals.len()];
    let num = Decimal::from_str(val)
        .ok()
        .filter(|_| !pred.ends_with("expiry") && !pred.contains("revision"));
    let mut scope = BTreeMap::from([("supplier".to_string(), "ACME".to_string())]);
    if pred.starts_with("qa.") {
        scope.insert("lot".into(), format!("L{}", lot % 3));
    }
    Fact {
        id: format!("c{id:05}"),
        subject: format!("P-{}", subj % 6),
        predicate: pred.into(),
        text: if num.is_some() { None } else { Some(val.into()) },
        num,
        unit: num.map(|_| ["mm", "in", "cm"][unit as usize % 3].to_string()),
        revision: ["B", "C", ""][rev as usize % 3].to_string().into(),
        scope,
        source_kind: KINDS[kind as usize % KINDS.len()].into(),
    }
}

fn spec() -> impl Strategy<Value = Spec> {
    (
        any::<u8>(),
        any::<u8>(),
        any::<u8>(),
        any::<u8>(),
        any::<u8>(),
        any::<u8>(),
        any::<u8>(),
    )
}

fn key(f: &Finding) -> (String, &'static str, String, Option<String>, String) {
    (
        f.subject.clone(),
        f.kind,
        f.claim_a.clone(),
        f.claim_b.clone(),
        f.explanation.to_string(),
    )
}

fn ctx() -> Ctx {
    Ctx {
        today: "2026-11-05".into(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn incremental_composed_with_changes_equals_full(
        initial in prop::collection::vec(spec(), 0..60),
        // Each step: facts added, and indices of existing facts removed (superseded / rejected).
        steps in prop::collection::vec((prop::collection::vec(spec(), 0..8), prop::collection::vec(any::<u16>(), 0..4)), 1..8),
        threads in 1usize..5,
    ) {
        let rules = all_rules();
        let refs: Vec<&dyn Rule> = rules.iter().map(|r| r.as_ref()).collect();
        let mut next = 0usize;
        let mut facts: Vec<Fact> = initial.into_iter().map(|s| { next += 1; fact(next, s) }).collect();
        let mut state: Vec<Finding> = run(&refs, &facts, &ctx());
        for (added, removed) in steps {
            let mut dirty: BTreeSet<String> = BTreeSet::new();
            for r in removed {
                if !facts.is_empty() {
                    let f = facts.remove(r as usize % facts.len());
                    dirty.insert(f.subject);
                }
            }
            for s in added {
                next += 1;
                let f = fact(next, s);
                dirty.insert(f.subject.clone());
                facts.push(f);
            }
            // Incremental: drop the dirty subjects' findings, re-run only their facts (in parallel).
            let scoped: Vec<Fact> = facts.iter().filter(|f| dirty.contains(&f.subject)).cloned().collect();
            state.retain(|f| !dirty.contains(&f.subject));
            state.extend(run_parallel(&refs, &scoped, &ctx(), threads));
            let full = run(&refs, &facts, &ctx());
            let a: BTreeSet<_> = state.iter().map(key).collect();
            let b: BTreeSet<_> = full.iter().map(key).collect();
            prop_assert_eq!(a, b);
            prop_assert_eq!(run_parallel(&refs, &facts, &ctx(), threads), full);
        }
    }
}

use super::*;
use proptest::prelude::*;
use std::collections::BTreeSet;

fn snap(num: Option<&str>, unit: Option<&str>, text: Option<&str>) -> Snapshot {
    Snapshot {
        num: num.map(Into::into),
        unit: unit.map(Into::into),
        text: text.map(Into::into),
        ..Default::default()
    }
}

fn claim_dep(decision: &str, claim: &str, m: Materiality, s: Snapshot) -> Dep {
    Dep {
        decision: decision.into(),
        claim: Some(claim.into()),
        on_decision: None,
        subject: "BRK-100".into(),
        predicate: Some("mfg.dimension.length".into()),
        applicability: BTreeMap::from([("revision".into(), "C".into())]),
        materiality: m,
        snapshot: s,
    }
}

fn dec_dep(decision: &str, on: &str) -> Dep {
    Dep {
        decision: decision.into(),
        claim: None,
        on_decision: Some(on.into()),
        subject: "BRK-100".into(),
        predicate: None,
        applicability: BTreeMap::new(),
        materiality: Materiality::AnyChange,
        snapshot: Snapshot::default(),
    }
}

#[test]
fn materiality_rules() {
    let old = snap(Some("120"), Some("mm"), None);
    assert!(
        !materiality(&Materiality::AnyChange, &old, &snap(Some("12"), Some("cm"), None)).0,
        "unit-aware equality"
    );
    assert!(materiality(&Materiality::AnyChange, &old, &snap(Some("120.01"), Some("mm"), None)).0);
    let t = Materiality::NumericAbs {
        threshold: "0.05".into(),
    };
    assert!(!materiality(&t, &old, &snap(Some("120.04"), Some("mm"), None)).0);
    assert!(materiality(&t, &old, &snap(Some("120.06"), Some("mm"), None)).0);
    assert!(
        materiality(&t, &old, &snap(Some("120"), Some("kg"), None)).0,
        "incomparable units are material"
    );
    assert!(!materiality(&Materiality::Informational, &old, &snap(Some("999"), Some("mm"), None)).0);
    assert!(
        !materiality(
            &Materiality::AnyChange,
            &snap(None, None, Some("S355 ")),
            &snap(None, None, Some("s355"))
        )
        .0
    );
}

#[test]
fn selective_not_blanket() {
    let deps = vec![
        claim_dep(
            "D1",
            "c1",
            Materiality::NumericAbs {
                threshold: "0.1".into(),
            },
            snap(Some("120"), Some("mm"), None),
        ),
        claim_dep("D2", "c2", Materiality::AnyChange, snap(Some("120"), Some("mm"), None)),
        dec_dep("D3", "D1"),
        dec_dep("D4", "D2"),
        dec_dep("D5", "D4"),
    ];
    // Immaterial correction of c1: D1 recorded as immaterial, nothing propagates.
    let r = assess(
        &deps,
        &Change::Superseded {
            old_claim: "c1".into(),
            new_claim: "c9".into(),
            new: snap(Some("120.05"), Some("mm"), None),
        },
    );
    assert_eq!(r.len(), 1);
    assert!(!r[0].material);
    // Material correction of c2: D2, D4, D5 flagged; D1/D3 untouched.
    let r = assess(
        &deps,
        &Change::Superseded {
            old_claim: "c2".into(),
            new_claim: "c9".into(),
            new: snap(Some("121"), Some("mm"), None),
        },
    );
    let mut flagged: Vec<&str> = r.iter().filter(|i| i.material).map(|i| i.decision.as_str()).collect();
    flagged.sort();
    assert_eq!(flagged, ["D2", "D4", "D5"]);
    assert_eq!(
        r.iter().find(|i| i.decision == "D5").unwrap().path,
        ["c2", "c9", "D2", "D4", "D5"]
    );
}

#[test]
fn new_evidence_respects_applicability() {
    let deps = vec![claim_dep(
        "D1",
        "c1",
        Materiality::AnyChange,
        snap(Some("120"), Some("mm"), None),
    )];
    let mut other_rev = snap(Some("125"), Some("mm"), None);
    other_rev.revision = Some("D".into());
    let ch = |s: Snapshot| Change::NewEvidence {
        new_claim: "n1".into(),
        subject: "BRK-100".into(),
        predicate: "mfg.dimension.length".into(),
        new: s,
    };
    assert!(
        assess(&deps, &ch(other_rev)).is_empty(),
        "revision D evidence does not apply to a revision C decision"
    );
    let mut same_rev = snap(Some("125"), Some("mm"), None);
    same_rev.revision = Some("C".into());
    assert!(assess(&deps, &ch(same_rev.clone()))[0].material);
    let mut confirming = snap(Some("120"), Some("mm"), None);
    confirming.revision = Some("C".into());
    assert!(
        assess(&deps, &ch(confirming)).is_empty(),
        "confirming evidence is not a change"
    );
    let r = assess(
        &deps,
        &Change::Withdrawn {
            old_claim: "c1".into(),
            reason: "rejected in review".into(),
        },
    );
    assert!(r[0].material);
}

#[test]
fn regression_review_m1_scope_or_revision_change_is_material() {
    let old = Snapshot {
        revision: Some("C".into()),
        ..snap(Some("10"), Some("mm"), None)
    };
    let deps = vec![claim_dep("D1", "c1", Materiality::AnyChange, old)];
    let mut new = snap(Some("10"), Some("mm"), None);
    new.revision = Some("D".into());
    let r = assess(
        &deps,
        &Change::Superseded {
            old_claim: "c1".into(),
            new_claim: "c2".into(),
            new: new.clone(),
        },
    );
    assert!(r[0].material, "same value, new revision");
    new.revision = Some("C".into());
    new.scope.insert("supplier".into(), "S2".into());
    let r = assess(
        &deps,
        &Change::Superseded {
            old_claim: "c1".into(),
            new_claim: "c2".into(),
            new,
        },
    );
    assert!(r[0].material, "same value, new supplier scope");
}

#[test]
fn cycles_terminate() {
    let deps = vec![
        claim_dep("D1", "c1", Materiality::AnyChange, snap(Some("1"), None, None)),
        dec_dep("D2", "D1"),
        dec_dep("D1", "D2"),
    ];
    let r = assess(
        &deps,
        &Change::Withdrawn {
            old_claim: "c1".into(),
            reason: "x".into(),
        },
    );
    assert_eq!(r.len(), 2);
}

proptest! {
    /// On random DAGs, the flagged set equals exactly the decisions reachable from the changed claim through
    /// material dependencies (no false positives, no false negatives).
    #[test]
    fn flagged_set_equals_ground_truth(n in 2usize..25, edges in proptest::collection::vec((0usize..25, 0usize..25), 0..60),
                                       claim_users in proptest::collection::vec(0usize..25, 1..5), info in proptest::collection::vec(0usize..25, 0..12)) {
        let mut deps = Vec::new();
        for &u in &claim_users {
            if u < n { deps.push(claim_dep(&format!("D{u}"), "c1", Materiality::AnyChange, snap(Some("1"), None, None))); }
        }
        // Edges only from lower to higher index: a DAG.
        for (i, &(a, b)) in edges.iter().enumerate() {
            let (a, b) = (a % n, b % n);
            if a < b {
                let mut d = dec_dep(&format!("D{b}"), &format!("D{a}"));
                if info.contains(&(i % 25)) { d.materiality = Materiality::Informational; }
                deps.push(d);
            }
        }
        let r = assess(&deps, &Change::Withdrawn { old_claim: "c1".into(), reason: "t".into() });
        let got: BTreeSet<String> = r.iter().filter(|i| i.material).map(|i| i.decision.clone()).collect();
        // Ground truth by fixpoint iteration.
        let mut truth: BTreeSet<String> = claim_users.iter().filter(|&&u| u < n).map(|u| format!("D{u}")).collect();
        loop {
            let before = truth.len();
            for d in deps.iter().filter(|d| d.on_decision.is_some() && d.materiality != Materiality::Informational) {
                if truth.contains(d.on_decision.as_ref().unwrap()) { truth.insert(d.decision.clone()); }
            }
            if truth.len() == before { break; }
        }
        prop_assert_eq!(got, truth);
    }
}

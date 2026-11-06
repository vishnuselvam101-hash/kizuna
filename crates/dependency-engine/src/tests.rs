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
            reason: kizuna_messages::Msg::new("withdraw.rejected_in_review"),
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
            reason: kizuna_messages::Msg::new("withdraw.rejected_in_review"),
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
        let r = assess(&deps, &Change::Withdrawn { old_claim: "c1".into(), reason: kizuna_messages::Msg::new("withdraw.rejected_in_review") });
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

// Mutation-testing kills (docs/testing/CHAOS_SOAK_2026-11-05.md).

/// Withdrawing evidence behind an informational dependency is reported but never material.
#[test]
fn withdrawal_of_informational_evidence_is_not_material() {
    let deps = [claim_dep(
        "D",
        "c1",
        Materiality::Informational,
        snap(Some("10"), Some("mm"), None),
    )];
    let out = assess(
        &deps,
        &Change::Withdrawn {
            old_claim: "c1".into(),
            reason: kizuna_messages::Msg::new("withdraw.rejected_in_review"),
        },
    );
    assert_eq!(out.len(), 1);
    assert!(!out[0].material);
}

/// A change exactly equal to the threshold is within it (strictly greater is material).
#[test]
fn change_equal_to_threshold_is_not_material() {
    let m = Materiality::NumericAbs {
        threshold: "0.05".into(),
    };
    let (mat, _) = materiality(
        &m,
        &snap(Some("10.00"), Some("mm"), None),
        &snap(Some("10.05"), Some("mm"), None),
    );
    assert!(!mat);
    let (mat, _) = materiality(
        &m,
        &snap(Some("10.00"), Some("mm"), None),
        &snap(Some("10.06"), Some("mm"), None),
    );
    assert!(mat);
}

/// One decision with a material and an immaterial dependency hit by the same change: material wins, whichever
/// order the dependencies are listed in.
#[test]
fn material_impact_wins_over_immaterial_for_the_same_decision() {
    let old = snap(Some("10"), Some("mm"), None);
    for order in [false, true] {
        let mut deps = vec![
            claim_dep("D", "c1", Materiality::Informational, old.clone()),
            claim_dep("D", "c1", Materiality::AnyChange, old.clone()),
        ];
        if order {
            deps.reverse();
        }
        let mut new = snap(Some("11"), Some("mm"), None);
        new.revision = Some("C".into());
        let out = assess(
            &deps,
            &Change::Superseded {
                old_claim: "c1".into(),
                new_claim: "c2".into(),
                new,
            },
        );
        assert_eq!(out.len(), 1);
        assert!(out[0].material, "order {order}: {out:?}");
    }
}

fn arb_materiality() -> impl Strategy<Value = Materiality> {
    prop_oneof![
        Just(Materiality::AnyChange),
        Just(Materiality::Informational),
        prop_oneof![Just("0"), Just("0.5"), Just("2"), Just("bad")]
            .prop_map(|t| Materiality::NumericAbs { threshold: t.into() }),
    ]
}

fn arb_snapshot() -> impl Strategy<Value = Snapshot> {
    (
        proptest::option::of(prop_oneof![Just("1"), Just("1.5"), Just("25.4"), Just("3")]),
        proptest::option::of(prop_oneof![Just("mm"), Just("in"), Just("kg"), Just("zz")]),
        proptest::option::of(prop_oneof![Just("Steel"), Just(" steel "), Just("Brass")]),
        proptest::option::of(prop_oneof![Just("C"), Just("c"), Just("D")]),
        proptest::option::of(prop_oneof![Just("acme"), Just("ACME"), Just("other")]),
    )
        .prop_map(|(num, unit, text, revision, supplier)| Snapshot {
            num: num.map(Into::into),
            unit: unit.map(Into::into),
            text: text.map(Into::into),
            revision: revision.map(Into::into),
            scope: supplier
                .map(|s| BTreeMap::from([("supplier".to_string(), s.to_string())]))
                .unwrap_or_default(),
        })
}

/// (decision, kind, materiality, snapshot, applicability revision). kind < 4: claim c{kind}; else on decision.
type EdgeSpec = (usize, usize, usize, Materiality, Snapshot, Option<&'static str>);

fn build_deps(n: usize, edges: &[EdgeSpec], dag: bool) -> Vec<Dep> {
    let mut deps = Vec::new();
    for (dec, kind, on, m, s, rev) in edges {
        let (dec, on) = (dec % n, on % n);
        let app: BTreeMap<String, String> = rev
            .map(|r| BTreeMap::from([("revision".into(), r.into())]))
            .unwrap_or_default();
        if *kind < 4 {
            deps.push(Dep {
                decision: format!("D{dec}"),
                claim: Some(format!("c{kind}")),
                on_decision: None,
                subject: "BRK-100".into(),
                predicate: Some("len".into()),
                applicability: app,
                materiality: m.clone(),
                snapshot: s.clone(),
            });
        } else if !dag || on < dec {
            deps.push(Dep {
                decision: format!("D{dec}"),
                claim: None,
                on_decision: Some(format!("D{on}")),
                subject: "BRK-100".into(),
                predicate: None,
                applicability: BTreeMap::new(),
                materiality: m.clone(),
                snapshot: Snapshot::default(),
            });
        }
    }
    deps
}

fn arb_change() -> impl Strategy<Value = Change> {
    (0usize..5, 0usize..3, arb_snapshot()).prop_map(|(c, kind, new)| match kind {
        0 => Change::Superseded {
            old_claim: format!("c{c}"),
            new_claim: "cN".into(),
            new,
        },
        1 => Change::Withdrawn {
            old_claim: format!("c{c}"),
            reason: kizuna_messages::Msg::new("withdraw.rejected_in_review"),
        },
        _ => Change::NewEvidence {
            new_claim: if c == 4 { "cN".into() } else { format!("c{c}") },
            subject: "BRK-100".into(),
            predicate: "len".into(),
            new,
        },
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]
    /// New (indexed, dirty-bit) algorithm == frozen v1 algorithm: same impacts, order, materiality, reasons, paths
    /// and semantic changes, on random DAGs and on graphs with cycles, for every change kind and for revocation.
    #[test]
    fn new_equals_reference(n in 1usize..30, dag in any::<bool>(),
                            edges in proptest::collection::vec((0usize..30, 0usize..8, 0usize..30, arb_materiality(),
                                arb_snapshot(), proptest::option::of(prop_oneof![Just("C"), Just("D")])), 0..120),
                            change in arb_change(), revoked in 0usize..30) {
        let deps = build_deps(n, &edges, dag);
        let new = assess(&deps, &change);
        prop_assert_eq!(&new, &reference::assess(&deps, &change));
        let r = format!("D{}", revoked % n);
        prop_assert_eq!(upstream_withdrawn(&deps, &r, &kizuna_messages::Msg::new("invalidation.verb.revoked")), reference::upstream_withdrawn(&deps, &r, &kizuna_messages::Msg::new("invalidation.verb.revoked")));
        // The bound is exact: Ok (and identical) up to the true fan-out, an explicit review-batch result beyond it.
        let len = new.len();
        prop_assert_eq!(assess_bounded(&deps, &change, len), Ok(new.clone()));
        if len > 0 {
            prop_assert_eq!(assess_bounded(&deps, &change, len - 1), Err(NeedsReviewBatch { limit: len - 1, reached: len }));
        }
    }
}

#[test]
fn bounded_fan_out_reports_review_batch() {
    let mut deps = vec![claim_dep(
        "D0",
        "c1",
        Materiality::AnyChange,
        snap(Some("1"), None, None),
    )];
    for i in 1..1000 {
        deps.push(dec_dep(&format!("D{i}"), "D0"));
    }
    let ch = Change::Withdrawn {
        old_claim: "c1".into(),
        reason: kizuna_messages::Msg::new("withdraw.rejected_in_review"),
    };
    let e = assess_bounded(&deps, &ch, 100).unwrap_err();
    assert_eq!(
        e,
        NeedsReviewBatch {
            limit: 100,
            reached: 101
        }
    );
    assert_eq!(
        e.to_string(),
        "invalidation fan-out exceeds 100 decisions: needs a review batch"
    );
    assert_eq!(assess_bounded(&deps, &ch, 1000).unwrap().len(), 1000);
    assert_eq!(
        upstream_withdrawn_bounded(
            &deps,
            "D0",
            &kizuna_messages::Msg::new("invalidation.verb.revoked"),
            998
        )
        .unwrap_err()
        .reached,
        999
    );
}

#[test]
fn seeded_impacts_propagate_downstream_only() {
    let deps = vec![
        claim_dep("D1", "C1", Materiality::AnyChange, Snapshot::default()),
        dec_dep("D2", "D1"),
        dec_dep("D3", "OTHER"),
    ];
    let seed = Impact {
        decision: "D1".into(),
        material: true,
        cause_kind: "regulation_list_changed",
        reason: String::new(),
        reason_msg: Msg::new("invalidation.reglist"),
        path: vec!["C1".into(), "D1".into()],
        semantic_change: json!({ "predicate": "reg.list.svhc" }),
    };
    let ghost = Impact {
        decision: "NOPE".into(),
        ..seed.clone()
    };
    let out = propagate_seeds(&deps, vec![seed, ghost], usize::MAX).unwrap();
    let ids: Vec<&str> = out.iter().map(|i| i.decision.as_str()).collect();
    assert_eq!(ids, ["D1", "D2"]);
    assert_eq!(out[0].cause_kind, "regulation_list_changed");
    assert!(out.iter().all(|i| i.material));
    assert!(propagate_seeds(&deps, vec![out[0].clone()], 0).is_err());
}

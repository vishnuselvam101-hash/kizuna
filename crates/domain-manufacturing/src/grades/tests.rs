use super::*;

const EXAMPLE: &str = include_str!("../../../../docs/examples/grade-equivalences.example.csv");

fn side(std: Option<&str>, g: &str) -> GradeSide {
    GradeSide {
        standard: std.map(Into::into),
        grade: g.into(),
    }
}

fn eq(id: &str, a: GradeSide, b: GradeSide, kind: EquivalenceKind) -> Equivalence {
    Equivalence {
        id: id.into(),
        entry: format!("entry-{id}"),
        version: 1,
        a,
        b,
        kind,
        conditions: (kind == EquivalenceKind::AcceptableWithConditions).then(|| "impact test".into()),
        source_checked: false,
    }
}

#[test]
fn notation_rules_split_system_document_and_designation() {
    let g = parse_grade("JIS G4303 SUS 304");
    assert_eq!(
        (g.standard.as_deref(), g.document.as_deref(), g.key.as_str()),
        (Some("JIS"), Some("G4303"), "SUS304")
    );
    let g = parse_grade("JIS G 4303 XQ101");
    assert_eq!(
        (g.standard.as_deref(), g.document.as_deref(), g.key.as_str()),
        (Some("JIS"), Some("G4303"), "XQ101")
    );
    let g = parse_grade("ASTM A240 Type 304");
    assert_eq!(
        (g.standard.as_deref(), g.document.as_deref(), g.key.as_str()),
        (Some("ASTM"), Some("A240"), "304")
    );
    let g = parse_grade("IS 2062:2011 E250");
    assert_eq!(
        (g.standard.as_deref(), g.document.as_deref(), g.key.as_str()),
        (Some("IS"), Some("2062:2011"), "E250")
    );
    let g = parse_grade("IS:2062 Grade E250");
    assert_eq!((g.standard.as_deref(), g.key.as_str()), (Some("IS"), "E250"));
    let g = parse_grade("EN 10025-2 S355JR");
    assert_eq!(
        (g.standard.as_deref(), g.document.as_deref(), g.key.as_str()),
        (Some("EN"), Some("10025-2"), "S355JR")
    );
    // A lettered document number has two or more digits; "K9" stays part of the designation.
    assert_eq!(parse_grade("IS K9 220").key, "K9220");
    let g = parse_grade("IS K12 220");
    assert_eq!((g.document.as_deref(), g.key.as_str()), (Some("K12"), "220"));
    assert!(g.designated_by("K12220"), "the document + key reading is also accepted");
    // The only token after the body is both document and designation.
    assert_eq!(parse_grade("ASTM A36").key, "A36");
    // Shape rule: one letter + five digits is a UNS number; no list of numbers is involved.
    assert_eq!(parse_grade("S30400").standard.as_deref(), Some("UNS"));
    assert_eq!(parse_grade("UNS S30400").key, "S30400");
    // Presentation is not identity; full-width digits are folded.
    assert_eq!(parse_grade("sus-304").key, parse_grade("ＳＵＳ３０４").key);
    // No system named: none is invented (no grade-family content).
    assert_eq!(parse_grade("SUS304").standard, None);
    assert_eq!(parse_grade("304").standard, None);
    assert_eq!(parse_grade("S355JR").standard, None);
}

#[test]
fn same_and_cross_standard() {
    let p = parse_grade;
    assert!(same_grade(&p("SUS 304"), &p("JIS SUS304")));
    assert!(
        !same_grade(&p("JIS 304"), &p("ASTM 304")),
        "two systems named: not the same by notation"
    );
    assert!(cross_standard(&p("SUS304"), &p("ASTM A240 304")));
    assert!(cross_standard(&p("SUS304"), &p("S30400")));
    assert!(
        !cross_standard(&p("S355JR"), &p("SUS304")),
        "no system on either side: a plain mismatch"
    );
    assert!(!cross_standard(&p("JIS XQ101"), &p("JIS XQ202")));
}

#[test]
fn judge_uses_only_the_tenant_table() {
    let ctx = GradeCtx {
        equivalences: vec![
            eq(
                "e1",
                side(Some("JIS"), "XQ101"),
                side(Some("ASTM"), "ZR-9901"),
                EquivalenceKind::Identical,
            ),
            eq(
                "e2",
                side(Some("JIS"), "XQ202"),
                side(Some("IS"), "K9 220"),
                EquivalenceKind::AcceptableWithConditions,
            ),
            eq(
                "e3",
                side(Some("JIS"), "XQ303"),
                side(Some("ASTM"), "ZR-9933"),
                EquivalenceKind::NotEquivalent,
            ),
        ],
        substitutions: vec![ApprovedSubstitution {
            decision: "d1".into(),
            certificate_claim: "cert-ok".into(),
            reference_claim: "ref".into(),
            equivalence: "e2".into(),
        }],
        ..GradeCtx::default()
    };
    let j = |r: &str, c: &str, cc: &str| judge(&ctx, r, c, "ref", cc);
    assert_eq!(j("XQ 101", "XQ101", "x"), GradeVerdict::Same);
    // Either direction, blank standard on the value side matches the entry's system.
    assert!(matches!(j("XQ101", "ASTM ZR-9901", "x"), GradeVerdict::Equivalent(e) if e.id == "e1"));
    assert!(matches!(
        j("ASTM ZR9901", "JIS XQ101", "x"),
        GradeVerdict::Equivalent(_)
    ));
    // A value naming a *different* system than the entry is not covered.
    assert_eq!(j("XQ101", "EN ZR-9901", "x"), GradeVerdict::NeedsDecision(None));
    assert!(matches!(j("XQ202", "IS K9-220", "x"), GradeVerdict::NeedsDecision(Some(e)) if e.id == "e2"));
    assert!(
        matches!(j("XQ202", "IS K9-220", "cert-ok"), GradeVerdict::Substituted(e, s) if e.id == "e2" && s.decision == "d1")
    );
    assert!(matches!(j("XQ303", "ASTM ZR-9933", "x"), GradeVerdict::Mismatch(Some(e)) if e.id == "e3"));
    // Without an entry: cross-standard needs a decision, same-or-no standard is a wrong material.
    assert_eq!(j("XQ101", "ASTM ZR-7777", "x"), GradeVerdict::NeedsDecision(None));
    assert_eq!(j("XQ101", "XQ999", "x"), GradeVerdict::Mismatch(None));
    // An empty table (what KIZUNA ships) never declares anything equivalent.
    let empty = GradeCtx::default();
    assert_eq!(
        judge(&empty, "XQ101", "ASTM ZR-9901", "r", "c"),
        GradeVerdict::NeedsDecision(None)
    );
}

#[test]
fn most_restrictive_entry_wins_on_overlap() {
    let ctx = GradeCtx {
        equivalences: vec![
            eq(
                "e1",
                side(None, "XQ101"),
                side(None, "ZR9901"),
                EquivalenceKind::Identical,
            ),
            eq(
                "e2",
                side(Some("JIS"), "XQ101"),
                side(Some("ASTM"), "ZR9901"),
                EquivalenceKind::NotEquivalent,
            ),
        ],
        ..GradeCtx::default()
    };
    assert!(
        matches!(judge(&ctx, "JIS XQ101", "ASTM ZR9901", "r", "c"), GradeVerdict::Mismatch(Some(e)) if e.id == "e2")
    );
}

#[test]
fn example_file_is_fictional_and_parses() {
    let (rows, errs) = parse_csv(EXAMPLE).unwrap();
    assert!(errs.is_empty(), "{errs:?}");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1].kind, EquivalenceKind::AcceptableWithConditions);
    assert!(rows[1].conditions.as_deref().unwrap().contains("impact test"));
    assert!(EXAMPLE.contains("EXAMPLE ONLY") && EXAMPLE.contains("FICTIONAL"));
}

#[test]
fn csv_rejects_bad_rows_with_codes() {
    let h = CSV_HEADER.join(",");
    let csv = format!(
        "{h}\nJIS,XQ1,ASTM,ZR1,identical,,\nJIS,XQ2,ASTM,ZR2,acceptable_with_conditions,,\nJIS,XQ1,ASTM,ZR1,identical,,dup\n\
         JIS,XQ3,ASTM,ZR3,maybe,,\nJIS,XQ4,JIS,XQ 4,identical,,\nJIS,,ASTM,ZR5,identical,,\nonly,three,cols\n"
    );
    let (ok, errs) = parse_csv(&csv).unwrap();
    assert_eq!(ok.len(), 1);
    let codes: Vec<_> = errs.iter().map(|e| (e.row, e.code)).collect();
    assert_eq!(
        codes,
        [
            (3, "conditions_required"),
            (4, "duplicate_pair"),
            (5, "invalid_equivalence"),
            (6, "same_grade"),
            (7, "grade_required"),
            (8, "wrong_column_count")
        ]
    );
    assert_eq!(parse_csv("a,b\n1,2\n"), Err("invalid_header"));
    assert_eq!(parse_csv(&format!("{h}\n\"open")), Err("unterminated_quote"));
    // Quoted commas and doubled quotes survive.
    let (ok, _) = parse_csv(&format!(
        "{h}\r\nJIS,XQ1,ASTM,ZR1,acceptable_with_conditions,\"a, \"\"b\"\"\",\r\n"
    ))
    .unwrap();
    assert_eq!(ok[0].conditions.as_deref(), Some("a, \"b\""));
}

#[test]
fn pair_key_is_order_independent() {
    let (a, b) = (side(Some("jis"), "XQ 1"), side(None, "ASTM ZR-1"));
    assert_eq!(pair_key(&a, &b), pair_key(&b, &a));
    assert_eq!(pair_key(&a, &b), "ASTM|ZR1~JIS|XQ1");
}

mod props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn presentation_never_changes_the_key(core in "[QXZ][A-Z]{0,3}[0-9]{1,5}[A-Z]{0,2}", sep in "[ ._-]{0,2}") {
            let spaced: String = core.chars().map(|c| format!("{c}{sep}")).collect();
            prop_assert_eq!(parse_grade(&spaced.to_lowercase()).key, parse_grade(&core).key);
        }

        #[test]
        fn parse_never_panics(s in "\\PC{0,60}") {
            let _ = parse_grade(&s);
            let _ = parse_csv(&s);
        }
    }
}

#[test]
fn every_statement_is_labelled_unverified_until_the_tenant_checks_it() {
    let mut e = eq(
        "e1",
        side(Some("JIS"), "XQ101"),
        side(Some("ASTM"), "ZR-9901"),
        EquivalenceKind::Identical,
    );
    assert_eq!(e.verification(), "unverified");
    let j = e.to_json();
    assert_eq!(j["verification"], "unverified");
    assert_eq!(j["note"], UNVERIFIED_NOTE);
    assert!(UNVERIFIED_NOTE.contains("unverified — check against the standard"));
    assert!(UNVERIFIED_NOTE.contains("not an authoritative"));
    e.source_checked = true;
    let j = e.to_json();
    assert_eq!(j["verification"], "tenant_checked");
    // Even a checked entry is the tenant's statement, never presented as the standard itself.
    assert!(j["note"].as_str().unwrap().contains("not an authoritative"));
}

#[test]
fn limits_apply_to_the_required_grade_only() {
    let lim = |id: &str, std: Option<&str>, g: &str, pr: &str| GradeLimit {
        id: id.into(),
        entry: format!("entry-{id}"),
        version: 1,
        grade: side(std, g),
        property: pr.into(),
        min: None,
        max: Some(rust_decimal::Decimal::from(100)),
        unit: Some("mpa".into()),
        source_checked: false,
    };
    let ctx = GradeCtx {
        limits: vec![
            lim("l1", Some("JIS"), "XQ101", "tensile_strength"),
            lim("l2", None, "XQ101", "hardness"),
            lim("l3", Some("ASTM"), "XQ101", "elongation"),
            lim("l4", Some("JIS"), "XQ999", "yield_strength"),
        ],
        ..GradeCtx::default()
    };
    let ids = |g: &str| {
        ctx.limits_for(&parse_grade(g))
            .iter()
            .map(|l| l.id.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids("JIS G4303 XQ-101"), ["l2", "l1"], "ordered by property");
    assert_eq!(
        ids("XQ101"),
        ["l3", "l2", "l1"],
        "no system named: every entry for the designation applies"
    );
    assert_eq!(ids("XQ555"), Vec::<&str>::new());
}

#[test]
fn limit_validation() {
    let ok = validate_limit("JIS", "XQ101", "tensile_strength", "400", "", "MPa", "memo").unwrap();
    assert_eq!(ok.property, "tensile_strength");
    assert_eq!(ok.min, Some(rust_decimal::Decimal::from(400)));
    assert_eq!(ok.unit.as_deref(), Some("mpa"));
    assert_eq!(validate_limit("", "XQ101", "c", "", "", "%", ""), Err("bound_required"));
    assert_eq!(
        validate_limit("", "XQ101", "c", "0.5", "0.1", "%", ""),
        Err("min_above_max")
    );
    assert_eq!(
        validate_limit("", "XQ101", "Bad Prop", "1", "", "", ""),
        Err("invalid_property")
    );
    assert_eq!(
        validate_limit("", "XQ101", "c", "abc", "", "", ""),
        Err("invalid_number")
    );
    assert_eq!(validate_limit("", "", "c", "1", "", "", ""), Err("grade_required"));
}

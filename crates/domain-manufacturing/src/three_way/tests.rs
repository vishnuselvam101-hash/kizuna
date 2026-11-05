use std::str::FromStr;

use super::*;

struct B {
    facts: Vec<Fact>,
    docs: BTreeMap<String, String>,
}

impl B {
    fn new() -> Self {
        B {
            facts: vec![],
            docs: BTreeMap::new(),
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        doc: &str,
        kind: &str,
        pred: &str,
        text: Option<&str>,
        num: Option<&str>,
        unit: Option<&str>,
        rev: Option<&str>,
        scope: &[(&str, &str)],
    ) -> String {
        let id = format!("c{:03}", self.facts.len());
        self.facts.push(Fact {
            id: id.clone(),
            subject: "MX-1001".into(),
            predicate: pred.into(),
            text: text.map(Into::into),
            num: num.map(|n| Decimal::from_str(n).unwrap_or_default()),
            unit: unit.map(Into::into),
            revision: rev.map(Into::into),
            scope: scope.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            source_kind: kind.into(),
        });
        self.docs.insert(id.clone(), doc.into());
        id
    }
    fn po(&mut self, pred: &str, text: Option<&str>, num: Option<&str>) -> String {
        self.add(
            "po",
            "purchase_order",
            pred,
            text,
            num,
            None,
            None,
            &[
                ("purchase_order", "PO-1"),
                ("po_line", "1"),
                ("supplier", "NAGOYA FORGE"),
            ],
        )
    }
    fn dw(&mut self, rev: &str, pred: &str, text: Option<&str>, num: Option<&str>, unit: Option<&str>) -> String {
        self.add(&format!("dwg-{rev}"), "drawing", pred, text, num, unit, Some(rev), &[])
    }
    fn cert(
        &mut self,
        doc: &str,
        pred: &str,
        text: Option<&str>,
        num: Option<&str>,
        unit: Option<&str>,
        scope: &[(&str, &str)],
    ) -> String {
        self.add(doc, "inspection_report", pred, text, num, unit, None, scope)
    }
    fn eval(&self) -> MatchSet {
        let line = PoLine {
            purchase_order: "PO-1".into(),
            line: "1".into(),
            part: "MX-1001".into(),
        };
        evaluate(&line, &self.facts, &self.docs)
    }
}

const IR: &[(&str, &str)] = &[("purchase_order", "PO-1"), ("lot", "L1")];
const MS: &[(&str, &str)] = &[("purchase_order", "PO-1"), ("heat", "H9")];

/// A clean PO line: PO, drawing rev C, a mill sheet and an inspection report that agree.
fn clean() -> B {
    let mut b = B::new();
    b.po("po.revision_ordered", Some("C"), None);
    b.po("po.drawing_number", Some("DM-100"), None);
    b.po("po.material", Some("SUS 304"), None);
    b.po("po.certificate_type", Some("EN 10204 3.1"), None);
    b.po("po.supplier", Some("NAGOYA FORGE"), None);
    b.dw("C", "mfg.revision", Some("C"), None, None);
    b.dw("C", "mfg.drawing_number", Some("DM-100"), None, None);
    b.dw("C", "mfg.material", Some("SUS304"), None, None);
    b.dw("C", "mfg.approval.status", Some("approved"), None, None);
    b.dw("C", "mfg.dimension.length", None, Some("120"), Some("mm"));
    b.dw("C", "mfg.tolerance.length.plus", None, Some("0.1"), Some("mm"));
    b.dw("C", "mfg.tolerance.length.minus", None, Some("0.1"), Some("mm"));
    b.dw("C", "mfg.property.tensile_strength.min", None, Some("520"), Some("mpa"));
    b.cert("ms", "qa.material", Some("SUS304"), None, None, MS);
    b.cert("ms", "qa.certificate_type", Some("3.1"), None, None, MS);
    b.cert("ms", "qa.property.tensile_strength", None, Some("560"), Some("mpa"), MS);
    b.cert("ir", "qa.revision_inspected", Some("C"), None, None, IR);
    b.cert("ir", "qa.measured.length", None, Some("120.05"), Some("mm"), IR);
    b
}

fn kinds(m: &MatchSet) -> Vec<&'static str> {
    m.findings.iter().map(|f| f.kind).collect()
}

fn set(b: &mut B, pred: &str, f: impl Fn(&mut Fact)) {
    for x in b.facts.iter_mut().filter(|x| x.predicate == pred) {
        f(x);
    }
}

#[test]
fn clean_line_matches_on_every_check() {
    let m = clean().eval();
    assert!(m.findings.is_empty(), "{:?}", m.findings);
    assert_eq!(m.drawing_revision.as_deref(), Some("C"));
    assert_eq!(m.supplier.as_deref(), Some("NAGOYA FORGE"));
    assert_eq!(m.certificates.len(), 2);
    for r in &m.checks {
        assert!(matches!(r.status, "match" | "not_required"), "{r:?}");
    }
    assert_eq!(m.po_claims.len(), 5);
    assert_eq!(m.drawing_claims.len(), 8);
    assert_eq!(m.certificate_claims.len(), 5);
}

#[test]
fn every_injected_mismatch_is_found_with_three_sided_citations() {
    let mut b = clean();
    set(&mut b, "qa.measured.length", |f| {
        f.num = Decimal::from_str("120.2").ok()
    });
    let m = b.eval();
    assert_eq!(kinds(&m), ["measurement_out_of_tolerance"]);
    let f = &m.findings[0];
    assert_eq!(f.severity, Severity::Critical);
    assert_eq!(f.drawing_claims.len(), 3, "nominal + both tolerances");
    assert_eq!(f.certificate_claims.len(), 1);
    assert_eq!(f.certificate_doc.as_deref(), Some("ir"));
    assert_eq!(f.explanation["detail"]["upper"], "120.1");

    let mut b = clean();
    set(&mut b, "qa.material", |f| f.text = Some("SUS316".into()));
    let m = b.eval();
    assert_eq!(kinds(&m), ["certificate_material_mismatch"]);
    assert!(!m.findings[0].po_claims.is_empty() && !m.findings[0].drawing_claims.is_empty());

    let mut b = clean();
    set(&mut b, "po.material", |f| f.text = Some("SUS316".into()));
    assert_eq!(kinds(&b.eval()), ["po_drawing_material_mismatch"]);

    let mut b = clean();
    set(&mut b, "po.certificate_type", |f| f.text = Some("EN 10204 3.2".into()));
    assert_eq!(kinds(&b.eval()), ["certificate_type_insufficient"]);

    let mut b = clean();
    set(&mut b, "qa.revision_inspected", |f| f.text = Some("B".into()));
    assert_eq!(kinds(&b.eval()), ["certificate_revision_mismatch"]);

    let mut b = clean();
    set(&mut b, "po.drawing_number", |f| f.text = Some("DM-101".into()));
    assert_eq!(kinds(&b.eval()), ["drawing_number_mismatch"]);

    let mut b = clean();
    set(&mut b, "qa.property.tensile_strength", |f| {
        f.num = Decimal::from_str("505").ok()
    });
    assert_eq!(kinds(&b.eval()), ["property_out_of_spec"]);

    let mut b = clean();
    set(&mut b, "mfg.approval.status", |f| f.text = Some("pending".into()));
    assert_eq!(kinds(&b.eval()), ["drawing_not_approved"]);
}

#[test]
fn stale_unknown_and_missing_revisions() {
    let mut b = clean();
    b.dw("D", "mfg.revision", Some("D"), None, None);
    b.dw("D", "mfg.dimension.length", None, Some("125"), Some("mm"));
    let m = b.eval();
    assert_eq!(kinds(&m), ["po_revision_stale"]);
    assert_eq!(
        m.drawing_revision.as_deref(),
        Some("C"),
        "still evaluated against what was ordered"
    );
    assert_eq!(m.latest_drawing_revision.as_deref(), Some("D"));

    let mut b = clean();
    set(&mut b, "po.revision_ordered", |f| f.text = Some("Q".into()));
    let m = b.eval();
    assert!(kinds(&m).contains(&"po_revision_unknown"));
    assert!(
        m.drawing_claims.is_empty(),
        "no design side without a resolvable revision"
    );
    assert!(
        !kinds(&m).contains(&"measurement_missing"),
        "no cascade from an unresolved revision"
    );

    let mut b = clean();
    b.facts.retain(|f| f.predicate != "po.revision_ordered");
    assert!(kinds(&b.eval()).contains(&"po_revision_missing"));
}

#[test]
fn missing_evidence_findings() {
    let mut b = clean();
    b.facts.retain(|f| f.source_kind != "inspection_report");
    assert_eq!(
        kinds(&b.eval()),
        ["certificate_missing"],
        "no cascade without certificates"
    );

    let mut b = clean();
    b.facts.retain(|f| f.predicate != "qa.measured.length");
    assert_eq!(kinds(&b.eval()), ["measurement_missing"]);

    let mut b = clean();
    for f in b.facts.iter_mut().filter(|f| f.source_kind == "inspection_report") {
        f.scope.remove("heat");
        if f.predicate.starts_with("qa.material")
            || f.predicate.starts_with("qa.property")
            || f.predicate.starts_with("qa.certificate")
        {
            f.scope.remove("lot");
        }
    }
    let m = b.eval();
    assert_eq!(kinds(&m), ["traceability_missing"]);
    assert_eq!(m.findings[0].certificate_doc.as_deref(), Some("ms"));

    let mut b = clean();
    b.facts.retain(|f| f.source_kind != "drawing");
    assert_eq!(kinds(&b.eval()), ["drawing_missing"]);
}

#[test]
fn certificates_of_other_orders_are_not_linked_and_units_convert() {
    let mut b = clean();
    b.cert(
        "other",
        "qa.measured.length",
        None,
        Some("99"),
        Some("mm"),
        &[("purchase_order", "PO-2"), ("lot", "Z")],
    );
    b.cert(
        "other-line",
        "qa.measured.length",
        None,
        Some("99"),
        Some("mm"),
        &[("purchase_order", "PO-1"), ("po_line", "2"), ("lot", "Z")],
    );
    let m = b.eval();
    assert!(m.findings.is_empty(), "{:?}", m.findings);
    assert_eq!(m.certificates.len(), 2);

    let mut b = clean();
    set(&mut b, "qa.measured.length", |f| {
        f.num = Decimal::from_str("12.004").ok();
        f.unit = Some("cm".into());
    });
    assert!(b.eval().findings.is_empty(), "12.004 cm = 120.04 mm");
    set(&mut b, "qa.measured.length", |f| f.unit = Some("kg".into()));
    assert_eq!(kinds(&b.eval()), ["measurement_unit_incompatible"]);
}

#[test]
fn en10204_and_grade_normalisation() {
    assert_eq!(en10204_rank("EN 10204 3.1"), Some(31));
    assert_eq!(en10204_rank("3.1.B"), Some(31));
    assert_eq!(en10204_rank("EN10204-3.2"), Some(32));
    assert_eq!(en10204_rank("type 2.2"), Some(22));
    assert_eq!(en10204_rank("ミルシート"), None);
    assert_eq!(en10204_rank("3.15"), None);
    assert_eq!(grade_key("sus-304"), grade_key("ＳＵＳ３０４"));
    assert_ne!(grade_key("S355JR"), grade_key("S355J2"));
    assert_eq!(norm_line("001"), "1");
}

#[test]
fn evaluation_is_deterministic_under_input_order() {
    let b = clean();
    let a = b.eval();
    let mut r = B {
        facts: b.facts.clone(),
        docs: b.docs.clone(),
    };
    r.facts.reverse();
    assert_eq!(a, r.eval());
}

#[test]
fn po_lines_are_listed_once() {
    let b = clean();
    let l = po_lines(&b.facts);
    assert_eq!(
        l,
        vec![PoLine {
            purchase_order: "PO-1".into(),
            line: "1".into(),
            part: "MX-1001".into()
        }]
    );
}

//! Manufacturing conflict rules (`mfg-rules@2`). Deterministic; every finding explains itself.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use conflict_engine::units::{QtyCmp, compare, parse_unit, to_base};
use conflict_engine::{Ctx, Fact, Finding, Rule, cmp_revision};
use entity_resolution::safe_normalize;
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::json;

pub const RULE_VERSION: &str = "mfg-rules@2";

fn finding(kind: &'static str, a: &Fact, b: Option<&Fact>, explanation: serde_json::Value) -> Finding {
    Finding {
        kind,
        subject: a.subject.clone(),
        claim_a: a.id.clone(),
        claim_b: b.map(|f| f.id.clone()),
        explanation,
    }
}

fn spec_like(f: &Fact) -> bool {
    !matches!(f.source_kind.as_str(), "purchase_order" | "inspection_report")
}

/// Specification values that disagree for the same predicate and revision (and compatible applicability).
pub struct SpecValueRule;

impl Rule for SpecValueRule {
    fn name(&self) -> &'static str {
        "spec_value"
    }
    fn detect(&self, _s: &str, facts: &[&Fact], _c: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        let mut groups: BTreeMap<(&str, Option<String>), Vec<&Fact>> = BTreeMap::new();
        for f in facts
            .iter()
            .filter(|f| f.predicate.starts_with("mfg.") && f.predicate != "mfg.revision")
        {
            groups
                .entry((f.predicate.as_str(), f.revision.as_ref().map(|r| r.to_uppercase())))
                .or_default()
                .push(f);
        }
        for ((pred, rev), fs) in groups {
            let supplier_record = pred == "mfg.supplier";
            for i in 0..fs.len() {
                for j in i + 1..fs.len() {
                    let (a, b) = (fs[i], fs[j]);
                    // Different applicability (supplier-specific specs) is not a contradiction.
                    if !supplier_record
                        && a.scope.contains_key("supplier")
                        && b.scope.contains_key("supplier")
                        && a.scope.get("supplier") != b.scope.get("supplier")
                    {
                        continue;
                    }
                    let base =
                        json!({ "predicate": pred, "revision": rev, "a": a.display_value(), "b": b.display_value() });
                    match (a.num, b.num) {
                        (Some(x), Some(y)) => match compare(x, a.unit.as_deref(), y, b.unit.as_deref()) {
                            QtyCmp::Equal => {}
                            QtyCmp::Different { .. } => out.push(finding("value_mismatch", a, Some(b), json!({ "rule": "spec_value", "why": "same predicate and revision, different values", "detail": base }))),
                            QtyCmp::Incompatible | QtyCmp::UnknownUnit => out.push(finding("unit_incompatible", a, Some(b), json!({ "rule": "spec_value", "why": "units cannot be compared", "units": [a.unit, b.unit], "detail": base }))),
                        },
                        _ if a.text_norm() != b.text_norm() => {
                            let kind = if supplier_record { "conflicting_supplier_record" } else { "value_mismatch" };
                            out.push(finding(kind, a, Some(b), json!({ "rule": "spec_value", "why": "sources state different values", "detail": base })));
                        }
                        _ => {}
                    }
                }
            }
        }
        out
    }
}

fn latest_rev<'a>(revs: &[&'a Fact]) -> Option<&'a Fact> {
    let mut best: Option<&Fact> = None;
    for f in revs {
        let Some(t) = &f.text else { continue };
        best = match best {
            None => Some(f),
            Some(b) => match cmp_revision(t, b.text.as_deref().unwrap_or("")) {
                Some(Ordering::Greater) => Some(f),
                _ => Some(b),
            },
        };
    }
    best
}

/// Ordered/inspected revision has no specification evidence, or a newer specification revision exists.
pub struct RevisionRule;

impl Rule for RevisionRule {
    fn name(&self) -> &'static str {
        "revision"
    }
    fn detect(&self, _s: &str, facts: &[&Fact], _c: &Ctx) -> Vec<Finding> {
        let spec_revs: Vec<&Fact> = facts
            .iter()
            .copied()
            .filter(|f| f.predicate == "mfg.revision")
            .collect();
        let latest = latest_rev(&spec_revs);
        let mut out = Vec::new();
        for f in facts
            .iter()
            .filter(|f| matches!(f.predicate.as_str(), "po.revision_ordered" | "qa.revision_inspected"))
        {
            let Some(r) = f.text.as_deref() else { continue };
            let known = spec_revs
                .iter()
                .any(|s| s.text.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(r)));
            let role = if f.predicate.starts_with("po.") {
                "ordered"
            } else {
                "inspected"
            };
            match latest {
                None => out.push(finding("revision_mismatch", f, None, json!({ "rule": "revision", "why": format!("{role} revision {r} has no specification or drawing evidence") }))),
                Some(l) if !known => out.push(finding("revision_mismatch", f, Some(l), json!({ "rule": "revision", "why": format!("{role} revision {r} does not exist in specification evidence"), "latest_known": l.text }))),
                Some(l) if cmp_revision(l.text.as_deref().unwrap_or(""), r) == Some(Ordering::Greater) => out.push(finding("revision_mismatch", f, Some(l), json!({ "rule": "revision", "why": format!("{role} revision {r} is older than revision {}", l.text.as_deref().unwrap_or("")), "latest_known": l.text }))),
                _ => {}
            }
        }
        out
    }
}

/// The ordered revision must have an `approved` approval claim for that revision.
pub struct ApprovalRule;

impl Rule for ApprovalRule {
    fn name(&self) -> &'static str {
        "approval"
    }
    fn detect(&self, _s: &str, facts: &[&Fact], _c: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        for f in facts.iter().filter(|f| f.predicate == "po.revision_ordered") {
            let Some(r) = f.text.as_deref() else { continue };
            let approvals: Vec<&&Fact> = facts
                .iter()
                .filter(|a| {
                    a.predicate == "mfg.approval.status"
                        && a.revision.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(r))
                })
                .collect();
            if approvals.is_empty() {
                out.push(finding(
                    "missing_approval",
                    f,
                    None,
                    json!({ "rule": "approval", "why": format!("no approval evidence for ordered revision {r}") }),
                ));
            } else if !approvals.iter().any(|a| a.text_norm().as_deref() == Some("approved")) {
                let a = approvals[0];
                out.push(finding("missing_approval", f, Some(a), json!({ "rule": "approval", "why": format!("revision {r} approval status is '{}'", a.display_value()) })));
            }
        }
        out
    }
}

/// Certificates/approvals past their expiry date.
pub struct ExpiryRule;

impl Rule for ExpiryRule {
    fn name(&self) -> &'static str {
        "expiry"
    }
    fn detect(&self, _s: &str, facts: &[&Fact], c: &Ctx) -> Vec<Finding> {
        facts
            .iter()
            .filter(|f| f.predicate == "mfg.certificate.expiry")
            .filter(|f| f.text.as_deref().is_some_and(|d| d.len() == 10 && d < c.today.as_str()))
            .map(|f| finding("expired_evidence", f, None, json!({ "rule": "expiry", "why": "evidence expired", "expired_on": f.text, "reference_date": c.today })))
            .collect()
    }
}

/// Inspection measurements outside nominal ± tolerance of the matching revision.
pub struct ToleranceRule;

pub(crate) fn in_unit(v: Decimal, from: Option<&str>, to: Option<&str>) -> Option<Decimal> {
    match (from, to) {
        (None, None) => Some(v),
        (Some(a), Some(b)) if a == b => Some(v),
        (Some(a), Some(b)) => {
            let (ua, ub) = (parse_unit(a)?, parse_unit(b)?);
            if ua.dim != ub.dim {
                return None;
            }
            to_base(v, &ua)?.checked_div(to_base(Decimal::ONE, &ub)?)
        }
        (None, Some(_)) => Some(v), // tolerance without unit inherits the nominal unit
        _ => None,
    }
}

impl Rule for ToleranceRule {
    fn name(&self) -> &'static str {
        "tolerance"
    }
    fn detect(&self, _s: &str, facts: &[&Fact], _c: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        let find = |pred: &str, rev: Option<&str>| -> Option<&Fact> {
            let cands: Vec<&Fact> = facts
                .iter()
                .copied()
                .filter(|f| f.predicate == pred && f.num.is_some())
                .collect();
            match rev {
                Some(r) => cands
                    .into_iter()
                    .find(|f| f.revision.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(r))),
                None => {
                    let mut revs: Vec<Option<String>> = cands.iter().map(|f| f.revision.clone()).collect();
                    revs.sort();
                    revs.dedup();
                    if revs.len() == 1 {
                        cands.into_iter().next()
                    } else {
                        None
                    }
                }
            }
        };
        for m in facts
            .iter()
            .filter(|f| f.predicate.starts_with("qa.measured.") && f.num.is_some())
        {
            let dim = &m.predicate["qa.measured.".len()..];
            let rev = m.revision.as_deref();
            let Some(nom) = find(&format!("mfg.dimension.{dim}"), rev) else {
                continue;
            };
            let (Some(plus), Some(minus)) = (
                find(&format!("mfg.tolerance.{dim}.plus"), nom.revision.as_deref()),
                find(&format!("mfg.tolerance.{dim}.minus"), nom.revision.as_deref()),
            ) else {
                continue;
            };
            let nu = nom.unit.as_deref();
            let (Some(n), Some(p), Some(mi), Some(meas)) = (
                nom.num,
                plus.num.and_then(|v| in_unit(v, plus.unit.as_deref(), nu)),
                minus.num.and_then(|v| in_unit(v, minus.unit.as_deref(), nu)),
                m.num.and_then(|v| in_unit(v, m.unit.as_deref(), nu)),
            ) else {
                out.push(finding("unit_incompatible", m, Some(nom), json!({ "rule": "tolerance", "why": "measurement and specification units cannot be compared", "units": [m.unit, nom.unit] })));
                continue;
            };
            let (lo, hi) = (n - mi, n + p);
            if meas < lo || meas > hi {
                out.push(finding("tolerance_violation", m, Some(nom), json!({
                    "rule": "tolerance", "why": "measured value outside nominal ± tolerance",
                    "measured": meas.normalize().to_string(), "nominal": n.normalize().to_string(),
                    "lower": lo.normalize().to_string(), "upper": hi.normalize().to_string(), "unit": nu, "revision": nom.revision,
                })));
            }
        }
        out
    }
}

/// A purchase order placed with a supplier that the specification/approval evidence does not cover.
pub struct ApplicabilityRule;

impl Rule for ApplicabilityRule {
    fn name(&self) -> &'static str {
        "applicability"
    }
    fn detect(&self, _s: &str, facts: &[&Fact], _c: &Ctx) -> Vec<Finding> {
        let spec_suppliers: Vec<&Fact> = facts
            .iter()
            .copied()
            .filter(|f| spec_like(f) && f.scope.contains_key("supplier"))
            .collect();
        if spec_suppliers.is_empty() {
            return vec![];
        }
        let mut out = Vec::new();
        for po in facts.iter().filter(|f| f.predicate == "po.revision_ordered") {
            let Some(s) = po.scope.get("supplier") else { continue };
            if !spec_suppliers
                .iter()
                .any(|f| f.scope.get("supplier").map(|x| safe_normalize(x)) == Some(safe_normalize(s)))
            {
                let covered: Vec<&String> = spec_suppliers.iter().filter_map(|f| f.scope.get("supplier")).collect();
                out.push(finding("wrong_applicability", po, Some(spec_suppliers[0]), json!({ "rule": "applicability", "why": format!("order placed with {s}; specification evidence applies to {covered:?}") })));
            }
        }
        out
    }
}

/// Inspection evidence for the same lot that disagrees; supplier fields that disagree within one purchase order.
pub struct RecordConsistencyRule;

impl Rule for RecordConsistencyRule {
    fn name(&self) -> &'static str {
        "record_consistency"
    }
    fn detect(&self, _s: &str, facts: &[&Fact], _c: &Ctx) -> Vec<Finding> {
        let mut out = Vec::new();
        let mut by_lot: BTreeMap<(&str, &str), Vec<&Fact>> = BTreeMap::new();
        for f in facts
            .iter()
            .filter(|f| f.predicate == "qa.result" || f.predicate.starts_with("qa.measured."))
        {
            if let Some(l) = f.scope.get("lot") {
                by_lot.entry((l.as_str(), f.predicate.as_str())).or_default().push(f);
            }
        }
        for ((lot, pred), fs) in by_lot {
            for j in 1..fs.len() {
                let (a, b) = (fs[0], fs[j]);
                let differs = match (a.num, b.num) {
                    (Some(x), Some(y)) => !matches!(compare(x, a.unit.as_deref(), y, b.unit.as_deref()), QtyCmp::Equal),
                    _ => a.text_norm() != b.text_norm(),
                };
                if differs {
                    out.push(finding("conflicting_inspection", a, Some(b), json!({ "rule": "record_consistency", "why": format!("inspection records for lot {lot} disagree on {pred}"), "a": a.display_value(), "b": b.display_value() })));
                }
            }
        }
        let mut by_po: BTreeMap<&str, Vec<&Fact>> = BTreeMap::new();
        for f in facts.iter().filter(|f| f.predicate == "po.supplier") {
            if let Some(po) = f.scope.get("purchase_order") {
                by_po.entry(po.as_str()).or_default().push(f);
            }
        }
        for (po, fs) in by_po {
            for j in 1..fs.len() {
                if fs[0].text_norm() != fs[j].text_norm() {
                    out.push(finding("conflicting_supplier_record", fs[0], Some(fs[j]), json!({ "rule": "record_consistency", "why": format!("purchase order {po} names different suppliers") })));
                }
            }
        }
        out
    }
}

/// The material an inspection certificate or mill sheet certifies differs from the specified material of the
/// same revision (a supplier shipping another grade is the classic ミルシート finding).
pub struct CertifiedMaterialRule;

impl Rule for CertifiedMaterialRule {
    fn name(&self) -> &'static str {
        "certified_material"
    }
    fn detect(&self, _s: &str, facts: &[&Fact], _c: &Ctx) -> Vec<Finding> {
        let rev = |f: &Fact| f.revision.as_ref().map(|r| r.to_uppercase());
        let norm = |f: &Fact| f.text_norm().map(|t| t.replace(' ', ""));
        let specs: Vec<&Fact> = facts
            .iter()
            .copied()
            .filter(|f| f.predicate == "mfg.material")
            .collect();
        let mut out = Vec::new();
        for q in facts.iter().copied().filter(|f| f.predicate == "qa.material") {
            let same_rev: Vec<&Fact> = specs.iter().copied().filter(|s| rev(s) == rev(q)).collect();
            // Only when every specification of that revision disagrees (one agreeing spec means the
            // disagreement is between specifications, which SpecValueRule reports).
            if !same_rev.is_empty() && same_rev.iter().all(|s| norm(s) != norm(q)) {
                let s = same_rev[0];
                out.push(finding(
                    "value_mismatch",
                    s,
                    Some(q),
                    json!({ "rule": "certified_material", "why": "certified material differs from the specification",
                            "detail": { "specified": s.display_value(), "certified": q.display_value(), "revision": rev(q) } }),
                ));
            }
        }
        out
    }
}

pub fn all_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(SpecValueRule),
        Box::new(CertifiedMaterialRule),
        Box::new(RevisionRule),
        Box::new(ApprovalRule),
        Box::new(ExpiryRule),
        Box::new(ToleranceRule),
        Box::new(ApplicabilityRule),
        Box::new(RecordConsistencyRule),
    ]
}

#[derive(Debug, Serialize, PartialEq)]
pub struct RevisionDiff {
    pub predicate: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub change: &'static str,
}

/// Compares specification claims of two revisions of one subject.
pub fn compare_revisions(facts: &[Fact], from: &str, to: &str) -> Vec<RevisionDiff> {
    let pick = |rev: &str| -> BTreeMap<String, String> {
        facts
            .iter()
            .filter(|f| {
                f.predicate.starts_with("mfg.")
                    && f.predicate != "mfg.revision"
                    && f.revision.as_deref().is_some_and(|r| r.eq_ignore_ascii_case(rev))
            })
            .map(|f| (f.predicate.clone(), f.display_value()))
            .collect()
    };
    let (a, b) = (pick(from), pick(to));
    let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter_map(|k| {
            let (x, y) = (a.get(k).cloned(), b.get(k).cloned());
            let change = match (&x, &y) {
                (Some(p), Some(q)) if p == q => return None,
                (Some(_), Some(_)) => "changed",
                (None, Some(_)) => "added",
                (Some(_), None) => "removed",
                (None, None) => return None,
            };
            Some(RevisionDiff {
                predicate: k.clone(),
                from: x,
                to: y,
                change,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[allow(clippy::too_many_arguments)]
    fn fact(
        id: &str,
        pred: &str,
        text: Option<&str>,
        num: Option<&str>,
        unit: Option<&str>,
        rev: Option<&str>,
        kind: &str,
        scope: &[(&str, &str)],
    ) -> Fact {
        Fact {
            id: id.into(),
            subject: "BRK-100".into(),
            predicate: pred.into(),
            text: text.map(Into::into),
            num: num.map(|n| Decimal::from_str(n).unwrap()),
            unit: unit.map(Into::into),
            revision: rev.map(Into::into),
            scope: scope.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            source_kind: kind.into(),
        }
    }

    fn run(facts: &[Fact]) -> Vec<Finding> {
        let rules = all_rules();
        let refs: Vec<&dyn Rule> = rules.iter().map(|r| r.as_ref()).collect();
        conflict_engine::run(
            &refs,
            facts,
            &Ctx {
                today: "2026-11-05".into(),
            },
        )
    }

    fn kinds(f: &[Finding]) -> Vec<&'static str> {
        f.iter().map(|f| f.kind).collect()
    }

    fn clean_spec() -> Vec<Fact> {
        vec![
            fact(
                "s1",
                "mfg.revision",
                Some("C"),
                None,
                None,
                Some("C"),
                "spec_document",
                &[("supplier", "ACME")],
            ),
            fact(
                "s2",
                "mfg.dimension.length",
                None,
                Some("120"),
                Some("mm"),
                Some("C"),
                "spec_document",
                &[("supplier", "ACME")],
            ),
            fact(
                "s3",
                "mfg.tolerance.length.plus",
                None,
                Some("0.2"),
                Some("mm"),
                Some("C"),
                "spec_document",
                &[("supplier", "ACME")],
            ),
            fact(
                "s4",
                "mfg.tolerance.length.minus",
                None,
                Some("0.2"),
                Some("mm"),
                Some("C"),
                "spec_document",
                &[("supplier", "ACME")],
            ),
            fact(
                "s5",
                "mfg.approval.status",
                Some("approved"),
                None,
                None,
                Some("C"),
                "spec_document",
                &[("supplier", "ACME")],
            ),
            fact(
                "s6",
                "mfg.certificate.expiry",
                Some("2027-01-01"),
                None,
                None,
                Some("C"),
                "spec_document",
                &[("supplier", "ACME")],
            ),
        ]
    }

    #[test]
    fn regression_qa_mill_sheet_material_differs_from_specification() {
        let mut f = clean_spec();
        f.push(fact(
            "m0",
            "mfg.material",
            Some("SUS304"),
            None,
            None,
            Some("C"),
            "spec_document",
            &[],
        ));
        // A mill sheet certifying the specified material (case/space differences only) is consistent.
        f.push(fact(
            "q0",
            "qa.material",
            Some("sus 304"),
            None,
            None,
            Some("C"),
            "inspection_report",
            &[],
        ));
        assert_eq!(kinds(&run(&f)), Vec::<&str>::new());
        // One certifying another grade is a finding naming both documents' claims.
        f.push(fact(
            "q1",
            "qa.material",
            Some("SUS316"),
            None,
            None,
            Some("C"),
            "inspection_report",
            &[],
        ));
        let r = run(&f);
        assert_eq!(kinds(&r), vec!["value_mismatch"]);
        assert_eq!(r[0].explanation["rule"], "certified_material");
    }

    #[test]
    fn consistent_evidence_has_no_findings() {
        let mut f = clean_spec();
        f.push(fact(
            "p1",
            "po.revision_ordered",
            Some("C"),
            None,
            None,
            None,
            "purchase_order",
            &[("supplier", "ACME"), ("purchase_order", "PO-1")],
        ));
        f.push(fact(
            "q1",
            "qa.measured.length",
            None,
            Some("120.1"),
            Some("mm"),
            Some("C"),
            "inspection_report",
            &[("lot", "L1")],
        ));
        f.push(fact(
            "x1",
            "mfg.dimension.length",
            None,
            Some("12"),
            Some("cm"),
            Some("C"),
            "drawing",
            &[],
        ));
        assert_eq!(run(&f), vec![], "same value in other units is not a conflict");
    }

    #[test]
    fn each_injected_fault_is_detected() {
        let mut f = clean_spec();
        f.push(fact(
            "p1",
            "po.revision_ordered",
            Some("B"),
            None,
            None,
            None,
            "purchase_order",
            &[("supplier", "BETA"), ("purchase_order", "PO-1")],
        ));
        f.push(fact(
            "q1",
            "qa.measured.length",
            None,
            Some("120.5"),
            Some("mm"),
            Some("C"),
            "inspection_report",
            &[("lot", "L1")],
        ));
        f.push(fact(
            "q2",
            "qa.result",
            Some("pass"),
            None,
            None,
            Some("C"),
            "inspection_report",
            &[("lot", "L1")],
        ));
        f.push(fact(
            "q3",
            "qa.result",
            Some("fail"),
            None,
            None,
            Some("C"),
            "inspection_report",
            &[("lot", "L1")],
        ));
        f.push(fact(
            "d1",
            "mfg.dimension.length",
            None,
            Some("125"),
            Some("mm"),
            Some("C"),
            "drawing",
            &[],
        ));
        f.push(fact(
            "d2",
            "mfg.material",
            Some("S355"),
            None,
            None,
            Some("C"),
            "drawing",
            &[],
        ));
        f.push(fact(
            "d3",
            "mfg.material",
            None,
            Some("355"),
            Some("kg"),
            Some("C"),
            "spec_document",
            &[],
        ));
        f.push(fact(
            "d4",
            "mfg.property.hardness",
            None,
            Some("40"),
            Some("HRC"),
            Some("C"),
            "drawing",
            &[],
        ));
        f.push(fact(
            "d5",
            "mfg.property.hardness",
            None,
            Some("380"),
            Some("HB"),
            Some("C"),
            "spec_document",
            &[],
        ));
        f[5] = fact(
            "s6",
            "mfg.certificate.expiry",
            Some("2026-01-31"),
            None,
            None,
            Some("C"),
            "spec_document",
            &[("supplier", "ACME")],
        );
        let k = kinds(&run(&f));
        for want in [
            "revision_mismatch",
            "missing_approval",
            "expired_evidence",
            "tolerance_violation",
            "wrong_applicability",
            "conflicting_inspection",
            "value_mismatch",
            "unit_incompatible",
        ] {
            assert!(k.contains(&want), "missing {want} in {k:?}");
        }
    }

    #[test]
    fn pending_approval_and_unknown_revision() {
        let mut f = clean_spec();
        f[4] = fact(
            "s5",
            "mfg.approval.status",
            Some("pending"),
            None,
            None,
            Some("C"),
            "spec_document",
            &[],
        );
        f.push(fact(
            "p1",
            "po.revision_ordered",
            Some("C"),
            None,
            None,
            None,
            "purchase_order",
            &[],
        ));
        f.push(fact(
            "p2",
            "po.revision_ordered",
            Some("X9"),
            None,
            None,
            None,
            "purchase_order",
            &[],
        ));
        let r = run(&f);
        assert!(r.iter().any(|x| x.kind == "missing_approval" && x.claim_a == "p1"));
        assert!(r.iter().any(|x| x.kind == "revision_mismatch" && x.claim_a == "p2"));
    }

    #[test]
    fn revision_diff() {
        let mut f = clean_spec();
        f.push(fact(
            "b1",
            "mfg.dimension.length",
            None,
            Some("118"),
            Some("mm"),
            Some("B"),
            "spec_document",
            &[],
        ));
        f.push(fact(
            "b2",
            "mfg.material",
            Some("S275"),
            None,
            None,
            Some("B"),
            "spec_document",
            &[],
        ));
        let d = compare_revisions(&f, "B", "C");
        assert!(
            d.iter()
                .any(|x| x.predicate == "mfg.dimension.length" && x.change == "changed")
        );
        assert!(d.iter().any(|x| x.predicate == "mfg.material" && x.change == "removed"));
        assert!(
            d.iter()
                .any(|x| x.predicate == "mfg.approval.status" && x.change == "added")
        );
    }
}

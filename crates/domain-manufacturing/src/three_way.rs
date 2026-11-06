//! Quality three-way match (`mfg-match@2`; @2 adds the cross-standard grade guard, `grades.rs`): purchase-order line ↔ drawing/specification ↔ supplier certificates
//! (mill sheets, inspection reports). Deterministic and explainable: every finding cites the claims it rests on, on
//! each of the three sides, so a reviewer can open the exact source location of every value.
//!
//! Sides are decided by the artifact kind a claim came from: `purchase_order` → PO side, `drawing`/`spec_document`
//! → design side, `inspection_report` → certificate side. Manual claims never fill a side (they are not documents).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use conflict_engine::{Fact, cmp_revision};
use entity_resolution::safe_normalize;
use kizuna_messages::Msg;
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::{Value, json};

use crate::grades::{GradeCtx, GradeVerdict, judge, parse_grade, same_grade};
use crate::rules::in_unit;

pub const MATCH_RULE_VERSION: &str = "mfg-match@2";

/// How much a finding matters for accepting the PO line. Critical and major findings block an `approved` outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    Major,
    Minor,
}

impl Severity {
    pub fn blocks_approval(self) -> bool {
        self != Severity::Minor
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Critical => "critical",
            Severity::Major => "major",
            Severity::Minor => "minor",
        }
    }
}

/// Fixed severity per finding kind (the materiality table of `mfg-match@2`).
pub const KINDS: &[(&str, Severity)] = &[
    ("drawing_missing", Severity::Critical),
    ("po_revision_unknown", Severity::Critical),
    ("drawing_number_mismatch", Severity::Critical),
    ("drawing_not_approved", Severity::Critical),
    ("certificate_missing", Severity::Critical),
    ("certificate_revision_mismatch", Severity::Critical),
    ("po_drawing_material_mismatch", Severity::Critical),
    ("certificate_material_mismatch", Severity::Critical),
    ("substitution_requires_decision", Severity::Critical),
    ("certificate_type_insufficient", Severity::Critical),
    ("measurement_out_of_tolerance", Severity::Critical),
    ("property_out_of_spec", Severity::Critical),
    ("po_revision_stale", Severity::Major),
    ("po_revision_missing", Severity::Major),
    ("certificate_type_missing", Severity::Major),
    ("certificate_type_unverifiable", Severity::Major),
    ("certificate_material_missing", Severity::Major),
    ("measurement_missing", Severity::Major),
    ("measurement_unit_incompatible", Severity::Major),
    ("property_missing", Severity::Major),
    ("grade_property_out_of_limits", Severity::Major),
    ("traceability_missing", Severity::Major),
    ("certificate_revision_missing", Severity::Minor),
];

pub fn severity_of(kind: &str) -> Severity {
    KINDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, s)| *s)
        .unwrap_or(Severity::Minor)
}

/// One purchase-order line of one part.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct PoLine {
    pub purchase_order: String,
    pub line: String,
    pub part: String,
}

/// "001" and "1" are the same line; non-numeric labels are compared after safe normalisation.
pub fn norm_line(s: &str) -> String {
    let t = safe_normalize(s);
    if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
        let z = t.trim_start_matches('0');
        return if z.is_empty() { "0".into() } else { z.into() };
    }
    t
}

fn line_of(f: &Fact) -> String {
    f.scope.get("po_line").map_or_else(|| "1".to_string(), |l| norm_line(l))
}

/// Distinct PO lines named by purchase-order evidence.
pub fn po_lines(facts: &[Fact]) -> Vec<PoLine> {
    facts
        .iter()
        .filter(|f| f.source_kind == "purchase_order")
        .filter_map(|f| {
            f.scope.get("purchase_order").map(|po| PoLine {
                purchase_order: safe_normalize(po),
                line: line_of(f),
                part: f.subject.clone(),
            })
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Cell {
    pub value: Option<String>,
    pub claims: Vec<String>,
}

impl Cell {
    fn of(fs: &[&Fact]) -> Cell {
        let mut vals: Vec<String> = fs.iter().map(|f| f.display_value()).collect();
        vals.dedup();
        Cell {
            value: (!vals.is_empty()).then(|| vals.join(" / ")),
            claims: fs.iter().map(|f| f.id.clone()).collect(),
        }
    }
    fn text(v: String, fs: &[&Fact]) -> Cell {
        Cell {
            value: Some(v),
            claims: fs.iter().map(|f| f.id.clone()).collect(),
        }
    }
}

/// One row of the match matrix. `status`: match | mismatch | missing | not_required | not_evaluated.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CheckRow {
    pub check: String,
    pub status: &'static str,
    pub po: Cell,
    pub drawing: Cell,
    pub certificate: Cell,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MatchFinding {
    pub kind: &'static str,
    pub severity: Severity,
    pub check: String,
    pub po_claims: Vec<String>,
    pub drawing_claims: Vec<String>,
    pub certificate_claims: Vec<String>,
    /// Certificate document the finding is about (when it concerns one document).
    pub certificate_doc: Option<String>,
    pub explanation: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CertificateDoc {
    pub doc: String,
    /// `purchase_order` when the certificate names this PO, `part` when it only names the part.
    pub linked_by: &'static str,
    pub lots: Vec<String>,
    pub heats: Vec<String>,
    pub claims: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MatchSet {
    pub rule_version: &'static str,
    pub line: PoLine,
    pub supplier: Option<String>,
    pub ordered_revision: Option<String>,
    pub drawing_revision: Option<String>,
    pub latest_drawing_revision: Option<String>,
    pub certificates: Vec<CertificateDoc>,
    pub checks: Vec<CheckRow>,
    pub findings: Vec<MatchFinding>,
    pub po_claims: Vec<String>,
    pub drawing_claims: Vec<String>,
    pub certificate_claims: Vec<String>,
    /// Certificate grades accepted through the tenant's equivalence table (`mfg-grade@1`): what each rests on.
    pub material_basis: Vec<MaterialBasis>,
}

/// A certificate grade that differs by notation from the required grade but is accepted: through an approved
/// `identical` equivalence, or a valid substitution decision under a conditional one.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MaterialBasis {
    pub certificate_claim: String,
    pub reference_claim: String,
    pub equivalence: String,
    pub equivalence_entry: String,
    pub equivalence_kind: &'static str,
    pub substitution_decision: Option<String>,
    /// `unverified` until the tenant records that it checked the entry against the standard.
    pub verification: &'static str,
}

impl MatchSet {
    pub fn blocking(&self) -> usize {
        self.findings.iter().filter(|f| f.severity.blocks_approval()).count()
    }
}

fn ids(fs: &[&Fact]) -> Vec<String> {
    fs.iter().map(|f| f.id.clone()).collect()
}

/// Material grade key: case, width, spaces, hyphens, underscores and dots are presentation ("SUS 304" = "sus-304").
pub fn grade_key(s: &str) -> String {
    safe_normalize(s)
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_' | '.'))
        .collect()
}

/// EN 10204 inspection document type as an ordered rank (2.1 < 2.2 < 3.1 < 3.2). The legacy "3.1.B"/"3.1B" is 3.1.
pub fn en10204_rank(s: &str) -> Option<u8> {
    let t = safe_normalize(s).replace(' ', "");
    let t = t
        .strip_prefix("EN10204")
        .unwrap_or(&t)
        .trim_start_matches([':', '-', '/']);
    let t = t.strip_prefix("TYPE").unwrap_or(t);
    let core: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let core = core.trim_end_matches('.');
    let rest = t[core.len()..].trim_start_matches('.');
    if !(rest.is_empty() || (core == "3.1" && rest == "B")) {
        return None;
    }
    match core {
        "2.1" => Some(21),
        "2.2" => Some(22),
        "3.1" => Some(31),
        "3.2" => Some(32),
        _ => None,
    }
}

struct Ctx {
    findings: Vec<MatchFinding>,
    rows: Vec<CheckRow>,
}

impl Ctx {
    #[allow(clippy::too_many_arguments)]
    fn find(
        &mut self,
        kind: &'static str,
        check: &str,
        po: &[&Fact],
        dw: &[&Fact],
        cert: &[&Fact],
        doc: Option<&str>,
        why: Msg,
        detail: Value,
    ) {
        self.findings.push(MatchFinding {
            kind,
            severity: severity_of(kind),
            check: check.to_string(),
            po_claims: ids(po),
            drawing_claims: ids(dw),
            certificate_claims: ids(cert),
            certificate_doc: doc.map(str::to_string),
            explanation: json!({ "rule": MATCH_RULE_VERSION, "why": why.en(), "msg": why, "detail": detail }),
        });
    }
    fn row(&mut self, check: &str, status: &'static str, po: Cell, drawing: Cell, certificate: Cell) {
        self.rows.push(CheckRow {
            check: check.to_string(),
            status,
            po,
            drawing,
            certificate,
        });
    }
}

fn pick<'a>(fs: &[&'a Fact], pred: &str) -> Vec<&'a Fact> {
    fs.iter().copied().filter(|f| f.predicate == pred).collect()
}

fn same_rev(a: Option<&str>, b: &str) -> bool {
    a.is_some_and(|x| x.trim().eq_ignore_ascii_case(b.trim()))
}

fn fmt_dec(d: Decimal) -> String {
    d.normalize().to_string()
}

/// Evaluates one PO line against the current facts of its part. `doc_of` maps claim id → source document id.
/// No tenant equivalence table: every cross-standard grade is a substitution to decide.
pub fn evaluate(line: &PoLine, facts: &[Fact], doc_of: &BTreeMap<String, String>) -> MatchSet {
    evaluate_with(line, facts, doc_of, &GradeCtx::default())
}

fn grade_json(s: &str) -> Value {
    let g = parse_grade(s);
    json!({ "value": s, "standard": g.standard, "document": g.document, "key": g.key })
}

fn equivalence_json(e: Option<&crate::grades::Equivalence>) -> Value {
    e.map_or(Value::Null, crate::grades::Equivalence::to_json)
}

/// [`evaluate`] with the tenant's approved grade-equivalence table and valid substitution decisions.
pub fn evaluate_with(line: &PoLine, facts: &[Fact], doc_of: &BTreeMap<String, String>, grades: &GradeCtx) -> MatchSet {
    let mut part: Vec<&Fact> = facts.iter().filter(|f| f.subject == line.part).collect();
    part.sort_by(|a, b| a.id.cmp(&b.id));
    let po_key = safe_normalize(&line.purchase_order);
    let po: Vec<&Fact> = part
        .iter()
        .copied()
        .filter(|f| {
            f.source_kind == "purchase_order"
                && f.scope.get("purchase_order").map(|p| safe_normalize(p)).as_deref() == Some(po_key.as_str())
                && line_of(f) == line.line
        })
        .collect();
    let supplier_f = pick(&po, "po.supplier");
    let supplier = supplier_f
        .first()
        .and_then(|f| f.text.clone())
        .or_else(|| po.iter().find_map(|f| f.scope.get("supplier").cloned()));
    let sup_key = supplier.as_deref().map(safe_normalize);
    // Design evidence: drawings first, then specifications; supplier-specific specs only for this PO's supplier.
    let mut design: Vec<&Fact> = part
        .iter()
        .copied()
        .filter(|f| matches!(f.source_kind.as_str(), "drawing" | "spec_document") && f.predicate.starts_with("mfg."))
        .filter(|f| match (f.scope.get("supplier"), &sup_key) {
            (Some(s), Some(k)) => safe_normalize(s) == *k,
            _ => true,
        })
        .collect();
    design.sort_by_key(|f| (f.source_kind != "drawing", f.id.clone()));
    let mut c = Ctx {
        findings: Vec::new(),
        rows: Vec::new(),
    };

    // --- Revision: what was ordered, what the drawing evidence knows. ---
    let ordered_f = pick(&po, "po.revision_ordered");
    let ordered = ordered_f.first().and_then(|f| f.text.clone());
    let mut revs: Vec<String> = design
        .iter()
        .filter_map(|f| f.revision.clone())
        .chain(pick(&design, "mfg.revision").iter().filter_map(|f| f.text.clone()))
        .map(|r| r.trim().to_uppercase())
        .collect();
    revs.sort();
    revs.dedup();
    let latest = revs.iter().fold(None::<String>, |best, r| match best {
        None => Some(r.clone()),
        Some(b) => match cmp_revision(r, &b) {
            Some(Ordering::Greater) => Some(r.clone()),
            _ => Some(b),
        },
    });
    let rev_claims = |r: &str| -> Vec<&Fact> {
        pick(&design, "mfg.revision")
            .into_iter()
            .filter(|f| same_rev(f.text.as_deref(), r))
            .collect()
    };
    let resolved: Option<String> = if design.is_empty() {
        c.find(
            "drawing_missing",
            "revision",
            &ordered_f,
            &[],
            &[],
            None,
            Msg::new("match.no_design_evidence").with("part", line.part.as_str()),
            json!({ "part": line.part }),
        );
        None
    } else {
        match &ordered {
            Some(r) if revs.iter().any(|x| x.eq_ignore_ascii_case(r)) => {
                if let Some(l) = &latest
                    && cmp_revision(l, r) == Some(Ordering::Greater)
                {
                    c.find(
                        "po_revision_stale",
                        "revision",
                        &ordered_f,
                        &rev_claims(l),
                        &[],
                        None,
                        Msg::new("match.revision_older")
                            .with("ordered", r.as_str())
                            .with("latest", l.as_str()),
                        json!({ "ordered": r, "latest": l }),
                    );
                }
                Some(r.trim().to_uppercase())
            }
            Some(r) => {
                c.find(
                    "po_revision_unknown",
                    "revision",
                    &ordered_f,
                    &rev_claims(latest.as_deref().unwrap_or("")),
                    &[],
                    None,
                    Msg::new("match.revision_unknown").with("ordered", r.as_str()),
                    json!({ "ordered": r, "known": revs }),
                );
                None
            }
            None => {
                c.find(
                    "po_revision_missing",
                    "revision",
                    &[],
                    &rev_claims(latest.as_deref().unwrap_or("")),
                    &[],
                    None,
                    Msg::new("match.po_revision_missing"),
                    json!({ "latest": latest }),
                );
                latest.clone()
            }
        }
    };
    let dw: Vec<&Fact> = match &resolved {
        Some(r) => design
            .iter()
            .copied()
            .filter(|f| f.revision.as_deref().is_none_or(|x| x.trim().eq_ignore_ascii_case(r)))
            .collect(),
        None => Vec::new(),
    };
    let design_ok = resolved.is_some();

    // --- Certificates linked to this line: same part; PO/line scope must match when stated. ---
    let mut certs: BTreeMap<String, Vec<&Fact>> = BTreeMap::new();
    let mut linked_by: BTreeMap<String, &'static str> = BTreeMap::new();
    for f in part.iter().copied().filter(|f| f.source_kind == "inspection_report") {
        let by = match f.scope.get("purchase_order") {
            Some(p) if safe_normalize(p) != po_key => continue,
            Some(_) => {
                if f.scope.get("po_line").is_some_and(|l| norm_line(l) != line.line) {
                    continue;
                }
                "purchase_order"
            }
            None => "part",
        };
        let doc = doc_of.get(&f.id).cloned().unwrap_or_else(|| format!("claim:{}", f.id));
        let e = linked_by.entry(doc.clone()).or_insert(by);
        if by == "purchase_order" {
            *e = by;
        }
        certs.entry(doc).or_default().push(f);
    }
    let cert_all: Vec<&Fact> = certs.values().flatten().copied().collect();
    let has_cert = !certs.is_empty();

    let ordered_cell = Cell::of(&ordered_f);
    let rev_cell = match &resolved {
        Some(r) => Cell::text(r.clone(), &rev_claims(r)),
        None => Cell::default(),
    };
    let insp: Vec<&Fact> = pick(&cert_all, "qa.revision_inspected");
    let rev_status = if !design_ok {
        "not_evaluated"
    } else if c
        .findings
        .iter()
        .any(|f| f.kind == "po_revision_stale" || f.kind == "po_revision_missing")
    {
        "mismatch"
    } else {
        "match"
    };
    if !has_cert {
        c.find(
            "certificate_missing",
            "certificate",
            &po,
            &[],
            &[],
            None,
            Msg::new("match.no_certificate")
                .with("po", line.purchase_order.as_str())
                .with("line", line.line.to_string()),
            json!({ "part": line.part }),
        );
    }
    let want_rev = ordered.clone().or(resolved.clone());
    let mut cert_rev_bad = false;
    for (doc, fs) in &certs {
        let ri = pick(fs, "qa.revision_inspected");
        match (ri.first(), &want_rev) {
            (Some(i), Some(w)) if !same_rev(i.text.as_deref(), w) => {
                cert_rev_bad = true;
                c.find(
                    "certificate_revision_mismatch",
                    "revision",
                    &ordered_f,
                    &rev_claims(w),
                    &ri,
                    Some(doc),
                    Msg::new("match.certificate_revision_mismatch")
                        .with("inspected", i.display_value())
                        .with("ordered", w.as_str()),
                    json!({ "inspected": i.text, "ordered": w }),
                );
            }
            (None, _) if fs.iter().any(|f| f.predicate.starts_with("qa.measured.")) => {
                c.find(
                    "certificate_revision_missing",
                    "revision",
                    &ordered_f,
                    &[],
                    fs,
                    Some(doc),
                    Msg::new("match.certificate_revision_missing"),
                    json!({}),
                );
            }
            _ => {}
        }
    }
    let rev_status = if cert_rev_bad { "mismatch" } else { rev_status };
    c.row("revision", rev_status, ordered_cell, rev_cell, Cell::of(&insp));

    // --- Drawing number and approval of the resolved revision. ---
    let po_dn = pick(&po, "po.drawing_number");
    let dw_dn = pick(&dw, "mfg.drawing_number");
    let dn_status = match (po_dn.first(), dw_dn.first()) {
        (None, _) => "not_required",
        (Some(_), None) if !design_ok => "not_evaluated",
        (Some(_), None) => "missing",
        (Some(a), Some(b)) => {
            let key = |f: &Fact| entity_resolution::loose_key(f.text.as_deref().unwrap_or(""));
            if key(a) == key(b) {
                "match"
            } else {
                c.find(
                    "drawing_number_mismatch",
                    "drawing_number",
                    &po_dn,
                    &dw_dn,
                    &[],
                    None,
                    Msg::new("match.drawing_number_mismatch")
                        .with("po", a.display_value())
                        .with("drawing", b.display_value()),
                    json!({ "po": a.text, "drawing": b.text }),
                );
                "mismatch"
            }
        }
    };
    c.row(
        "drawing_number",
        dn_status,
        Cell::of(&po_dn),
        Cell::of(&dw_dn),
        Cell::of(&pick(&cert_all, "qa.drawing_number")),
    );
    let appr = pick(&dw, "mfg.approval.status");
    let appr_status = if !design_ok {
        "not_evaluated"
    } else if appr.is_empty() {
        "missing"
    } else if appr.iter().any(|a| a.text_norm().as_deref() == Some("approved")) {
        "match"
    } else {
        c.find(
            "drawing_not_approved",
            "approval",
            &ordered_f,
            &appr,
            &[],
            None,
            Msg::new("match.drawing_not_approved")
                .with("revision", resolved.as_deref().unwrap_or(""))
                .with("status", appr[0].display_value()),
            json!({ "status": appr[0].text }),
        );
        "mismatch"
    };
    c.row(
        "approval",
        appr_status,
        Cell::default(),
        Cell::of(&appr),
        Cell::default(),
    );

    // --- Material grade: PO ↔ drawing, then certificates against the design (or the PO when no design grade). ---
    let po_m = pick(&po, "po.material");
    let dw_m = pick(&dw, "mfg.material");
    let mut mat_status = "match";
    if let (Some(a), Some(b)) = (po_m.first(), dw_m.first())
        && !same_grade(&parse_grade(&a.display_value()), &parse_grade(&b.display_value()))
    {
        mat_status = "mismatch";
        c.find(
            "po_drawing_material_mismatch",
            "material",
            &po_m,
            &dw_m,
            &[],
            None,
            Msg::new("match.po_drawing_material_mismatch")
                .with("po", a.display_value())
                .with("drawing", b.display_value()),
            json!({ "po": a.text, "drawing": b.text }),
        );
    }
    let reference: Option<&Fact> = dw_m.first().or(po_m.first()).copied();
    let cert_m = pick(&cert_all, "qa.material");
    let mut basis: Vec<MaterialBasis> = Vec::new();
    // Certificates whose stated grade is not accepted as the required one (their values are not held to its limits).
    let mut not_accepted: BTreeSet<&str> = BTreeSet::new();
    match reference {
        None => mat_status = if cert_m.is_empty() { "not_required" } else { "missing" },
        Some(_) if !has_cert => mat_status = "not_evaluated",
        Some(r) if cert_m.is_empty() => {
            mat_status = "missing";
            c.find(
                "certificate_material_missing",
                "material",
                &po_m,
                &dw_m,
                &[],
                None,
                Msg::new("match.certificate_material_missing").with("required", r.display_value()),
                json!({}),
            );
        }
        Some(r) => {
            for (doc, fs) in &certs {
                for m in pick(fs, "qa.material") {
                    let (req, got) = (r.display_value(), m.display_value());
                    let detail = |e: Option<&crate::grades::Equivalence>| {
                        json!({ "certificate": m.text, "required": r.text, "rule": crate::grades::GRADE_RULE_VERSION,
                                "certificate_grade": grade_json(&got), "required_grade": grade_json(&req),
                                "certificate_claim": m.id, "reference_claim": r.id, "equivalence": equivalence_json(e) })
                    };
                    match judge(grades, &req, &got, &r.id, &m.id) {
                        GradeVerdict::Same => {}
                        GradeVerdict::Equivalent(e) => basis.push(MaterialBasis {
                            certificate_claim: m.id.clone(),
                            reference_claim: r.id.clone(),
                            equivalence: e.id.clone(),
                            equivalence_entry: e.entry.clone(),
                            equivalence_kind: e.kind.as_str(),
                            substitution_decision: None,
                            verification: e.verification(),
                        }),
                        GradeVerdict::Substituted(e, s) => basis.push(MaterialBasis {
                            certificate_claim: m.id.clone(),
                            reference_claim: r.id.clone(),
                            equivalence: e.id.clone(),
                            equivalence_entry: e.entry.clone(),
                            equivalence_kind: e.kind.as_str(),
                            substitution_decision: Some(s.decision.clone()),
                            verification: e.verification(),
                        }),
                        GradeVerdict::NeedsDecision(e) => {
                            mat_status = "mismatch";
                            not_accepted.insert(doc.as_str());
                            let msg = match e {
                                Some(e) => Msg::new("match.substitution_conditional")
                                    .with("certified", got.as_str())
                                    .with("required", req.as_str())
                                    .with("conditions", e.conditions.as_deref().unwrap_or("")),
                                None => Msg::new("match.substitution_requires_decision")
                                    .with("certified", got.as_str())
                                    .with("required", req.as_str()),
                            };
                            c.find(
                                "substitution_requires_decision",
                                "material",
                                &po_m,
                                &dw_m,
                                &[m],
                                Some(doc),
                                msg,
                                detail(e),
                            );
                        }
                        GradeVerdict::Mismatch(e) => {
                            mat_status = "mismatch";
                            not_accepted.insert(doc.as_str());
                            let msg = match e {
                                Some(_) => Msg::new("match.material_not_equivalent")
                                    .with("certified", got.as_str())
                                    .with("required", req.as_str()),
                                None => Msg::new("match.certificate_material_mismatch")
                                    .with("certified", got.as_str())
                                    .with("required", req.as_str()),
                            };
                            c.find(
                                "certificate_material_mismatch",
                                "material",
                                &po_m,
                                &dw_m,
                                &[m],
                                Some(doc),
                                msg,
                                detail(e),
                            );
                        }
                    }
                }
            }
        }
    }
    c.row(
        "material",
        mat_status,
        Cell::of(&po_m),
        Cell::of(&dw_m),
        Cell::of(&cert_m),
    );

    // --- Inspection document type (EN 10204): the supplied type must be at least the required one. ---
    let po_t = pick(&po, "po.certificate_type");
    let dw_t = pick(&dw, "mfg.certificate_type");
    let req_t: Vec<&Fact> = if po_t.is_empty() { dw_t.clone() } else { po_t.clone() };
    let cert_t = pick(&cert_all, "qa.certificate_type");
    let t_status = match req_t.first() {
        None => "not_required",
        Some(_) if !has_cert => "not_evaluated",
        Some(r) if cert_t.is_empty() => {
            c.find(
                "certificate_type_missing",
                "certificate_type",
                &po_t,
                &dw_t,
                &[],
                None,
                Msg::new("match.certificate_type_missing").with("required", r.display_value()),
                json!({}),
            );
            "missing"
        }
        Some(r) => {
            let mut st = "match";
            for (doc, fs) in &certs {
                for t in pick(fs, "qa.certificate_type") {
                    let (need, got) = (r.text.as_deref().unwrap_or(""), t.text.as_deref().unwrap_or(""));
                    match (en10204_rank(need), en10204_rank(got)) {
                        (Some(n), Some(g)) if g < n => {
                            st = "mismatch";
                            c.find(
                                "certificate_type_insufficient",
                                "certificate_type",
                                &po_t,
                                &dw_t,
                                &[t],
                                Some(doc),
                                Msg::new("match.certificate_type_insufficient")
                                    .with("supplied", got)
                                    .with("required", need),
                                json!({ "required": need, "supplied": got }),
                            );
                        }
                        (Some(_), Some(_)) => {}
                        _ if grade_key(need) != grade_key(got) => {
                            st = "mismatch";
                            c.find(
                                "certificate_type_unverifiable",
                                "certificate_type",
                                &po_t,
                                &dw_t,
                                &[t],
                                Some(doc),
                                Msg::new("match.certificate_type_unverifiable")
                                    .with("supplied", got)
                                    .with("required", need),
                                json!({ "required": need, "supplied": got }),
                            );
                        }
                        _ => {}
                    }
                }
            }
            st
        }
    };
    c.row(
        "certificate_type",
        t_status,
        Cell::of(&po_t),
        Cell::of(&dw_t),
        Cell::of(&cert_t),
    );

    // --- Dimensions with tolerances on the drawing ↔ measured values on certificates. ---
    let mut dims: Vec<&str> = dw
        .iter()
        .filter_map(|f| f.predicate.strip_prefix("mfg.dimension."))
        .collect();
    dims.sort();
    dims.dedup();
    for d in dims {
        let check = format!("dimension.{d}");
        let nom = pick(&dw, &format!("mfg.dimension.{d}"));
        let plus = pick(&dw, &format!("mfg.tolerance.{d}.plus"));
        let minus = pick(&dw, &format!("mfg.tolerance.{d}.minus"));
        let meas = pick(&cert_all, &format!("qa.measured.{d}"));
        let (Some(n), Some(p), Some(m)) = (nom.first(), plus.first(), minus.first()) else {
            c.row(&check, "not_required", Cell::default(), Cell::of(&nom), Cell::of(&meas));
            continue;
        };
        let nu = n.unit.as_deref();
        let (Some(nv), Some(pv), Some(mv)) = (
            n.num,
            p.num.and_then(|v| in_unit(v, p.unit.as_deref(), nu)),
            m.num.and_then(|v| in_unit(v, m.unit.as_deref(), nu)),
        ) else {
            c.row(
                &check,
                "not_evaluated",
                Cell::default(),
                Cell::of(&nom),
                Cell::of(&meas),
            );
            continue;
        };
        let (lo, hi) = (nv - mv, nv + pv);
        let spec_facts = [*n, *p, *m];
        let spec = format!(
            "{} +{}/-{}{}",
            fmt_dec(nv),
            fmt_dec(pv),
            fmt_dec(mv),
            nu.map(|u| format!(" {u}")).unwrap_or_default()
        );
        let status = if !has_cert {
            "not_evaluated"
        } else if meas.is_empty() {
            c.find(
                "measurement_missing",
                &check,
                &[],
                &spec_facts,
                &[],
                None,
                Msg::new("match.measurement_missing")
                    .with("dimension", d)
                    .with("spec", spec.as_str()),
                json!({ "dimension": d }),
            );
            "missing"
        } else {
            let mut st = "match";
            for x in &meas {
                let doc = doc_of.get(&x.id).map(String::as_str);
                match x.num.and_then(|v| in_unit(v, x.unit.as_deref(), nu)) {
                    None => {
                        st = "mismatch";
                        c.find(
                            "measurement_unit_incompatible",
                            &check,
                            &[],
                            &spec_facts,
                            &[x],
                            doc,
                            Msg::new("match.measurement_unit_incompatible")
                                .with("measured", x.display_value())
                                .with("spec", spec.as_str()),
                            json!({ "units": [x.unit, n.unit] }),
                        );
                    }
                    Some(v) if v < lo || v > hi => {
                        st = "mismatch";
                        c.find("measurement_out_of_tolerance", &check, &[], &spec_facts, &[x], doc,
                            Msg::new("match.measurement_out_of_tolerance").with("measured", x.display_value()).with("spec", spec.as_str()),
                            json!({ "measured": fmt_dec(v), "lower": fmt_dec(lo), "upper": fmt_dec(hi), "unit": nu, "lot": x.scope.get("lot") }));
                    }
                    Some(_) => {}
                }
            }
            st
        };
        c.row(
            &check,
            status,
            Cell::default(),
            Cell::text(spec, &spec_facts),
            Cell::of(&meas),
        );
    }

    // --- Material properties with one-sided limits (e.g. tensile strength min) ↔ certificate values. ---
    let mut props: Vec<&str> = dw
        .iter()
        .filter_map(|f| f.predicate.strip_prefix("mfg.property."))
        .filter_map(|p| p.strip_suffix(".min").or_else(|| p.strip_suffix(".max")))
        .collect();
    props.sort();
    props.dedup();
    for pr in props {
        let check = format!("property.{pr}");
        let mn = pick(&dw, &format!("mfg.property.{pr}.min"));
        let mx = pick(&dw, &format!("mfg.property.{pr}.max"));
        let limits: Vec<&Fact> = mn.iter().chain(mx.iter()).copied().collect();
        let got = pick(&cert_all, &format!("qa.property.{pr}"));
        let spec = limits
            .iter()
            .map(|f| {
                format!(
                    "{} {}",
                    if f.predicate.ends_with(".min") { "≥" } else { "≤" },
                    f.display_value()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let status = if !has_cert {
            "not_evaluated"
        } else if got.is_empty() {
            c.find(
                "property_missing",
                &check,
                &[],
                &limits,
                &[],
                None,
                Msg::new("match.property_missing")
                    .with("property", pr)
                    .with("spec", spec.as_str()),
                json!({ "property": pr }),
            );
            "missing"
        } else {
            let mut st = "match";
            for x in &got {
                let doc = doc_of.get(&x.id).map(String::as_str);
                for l in &limits {
                    let is_min = l.predicate.ends_with(".min");
                    match (
                        l.num,
                        x.num.and_then(|v| in_unit(v, x.unit.as_deref(), l.unit.as_deref())),
                    ) {
                        (Some(lim), Some(v)) if (is_min && v < lim) || (!is_min && v > lim) => {
                            st = "mismatch";
                            c.find("property_out_of_spec", &check, &[], &[l], &[x], doc,
                                Msg::new("match.property_out_of_spec").with("certified", x.display_value()).with("property", pr).with("bound", if is_min { "≥" } else { "≤" }).with("limit", l.display_value()),
                                json!({ "value": fmt_dec(v), "limit": fmt_dec(lim), "bound": if is_min { "min" } else { "max" } }));
                        }
                        (Some(_), None) => {
                            st = "mismatch";
                            c.find(
                                "measurement_unit_incompatible",
                                &check,
                                &[],
                                &[l],
                                &[x],
                                doc,
                                Msg::new("match.property_unit_incompatible")
                                    .with("certified", x.display_value())
                                    .with("limit", l.display_value()),
                                json!({ "units": [x.unit, l.unit] }),
                            );
                        }
                        _ => {}
                    }
                }
            }
            st
        };
        c.row(
            &check,
            status,
            Cell::default(),
            Cell::text(spec, &limits),
            Cell::of(&got),
        );
    }

    // --- Tenant grade limits (`mfg-grade@1`): values on certificates whose grade is accepted as the required grade
    // are held to the limits the tenant recorded for that grade. The drawing governs: a property it limits itself is
    // not checked twice, and a value the certificate does not report is not a finding here.
    if let Some(r) = reference {
        let drawn: BTreeSet<&str> = dw
            .iter()
            .filter_map(|f| f.predicate.strip_prefix("mfg.property."))
            .filter_map(|p| p.strip_suffix(".min").or_else(|| p.strip_suffix(".max")))
            .collect();
        let req_grade = parse_grade(&r.display_value());
        let limits: Vec<&crate::grades::GradeLimit> = grades
            .limits_for(&req_grade)
            .into_iter()
            .filter(|l| !drawn.contains(l.property.as_str()))
            .collect();
        let mut props: Vec<&str> = limits.iter().map(|l| l.property.as_str()).collect();
        props.dedup();
        for pr in props {
            let check = format!("grade_limit.{pr}");
            let mine: Vec<&&crate::grades::GradeLimit> = limits.iter().filter(|l| l.property == pr).collect();
            let spec = mine
                .iter()
                .map(|l| {
                    let u = l.unit.as_deref().map(|u| format!(" {u}")).unwrap_or_default();
                    match (l.min, l.max) {
                        (Some(a), Some(b)) => format!("{}–{}{u}", fmt_dec(a), fmt_dec(b)),
                        (Some(a), None) => format!("≥ {}{u}", fmt_dec(a)),
                        (None, Some(b)) => format!("≤ {}{u}", fmt_dec(b)),
                        (None, None) => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            let got: Vec<&Fact> = certs
                .iter()
                .filter(|(d, _)| !not_accepted.contains(d.as_str()))
                .flat_map(|(_, fs)| pick(fs, &format!("qa.property.{pr}")))
                .collect();
            let status = if !has_cert || got.is_empty() {
                "not_evaluated"
            } else {
                let mut st = "match";
                for x in &got {
                    let doc = doc_of.get(&x.id).map(String::as_str);
                    for l in &mine {
                        let Some(v) = x.num.and_then(|v| in_unit(v, x.unit.as_deref(), l.unit.as_deref())) else {
                            st = "mismatch";
                            c.find(
                                "measurement_unit_incompatible",
                                &check,
                                &[],
                                &[r],
                                &[x],
                                doc,
                                Msg::new("match.property_unit_incompatible")
                                    .with("certified", x.display_value())
                                    .with("limit", spec.as_str()),
                                json!({ "units": [x.unit, l.unit], "limit": l.to_json() }),
                            );
                            continue;
                        };
                        let below = l.min.filter(|m| v < *m).map(|m| ("min", "≥", m));
                        let above = l.max.filter(|m| v > *m).map(|m| ("max", "≤", m));
                        if let Some((bound, sym, lim)) = below.or(above) {
                            st = "mismatch";
                            c.find(
                                "grade_property_out_of_limits",
                                &check,
                                &[],
                                &[r],
                                &[x],
                                doc,
                                Msg::new("match.grade_property_out_of_limits")
                                    .with("certified", x.display_value())
                                    .with("property", pr)
                                    .with("grade", r.display_value())
                                    .with("bound", sym)
                                    .with("limit", fmt_dec(lim))
                                    .with("verification", l.verification()),
                                json!({ "value": fmt_dec(v), "limit_value": fmt_dec(lim), "bound": bound,
                                        "rule": crate::grades::GRADE_RULE_VERSION, "required_grade": grade_json(&r.display_value()),
                                        "limit": l.to_json() }),
                            );
                        }
                    }
                }
                st
            };
            c.row(&check, status, Cell::default(), Cell::text(spec, &[r]), Cell::of(&got));
        }
    }

    // --- Traceability: every certificate must identify the material lot or heat. ---
    let mut cert_docs = Vec::new();
    let mut trace_bad = false;
    for (doc, fs) in &certs {
        let set = |k: &str| -> Vec<String> {
            let mut v: Vec<String> = fs.iter().filter_map(|f| f.scope.get(k).cloned()).collect();
            v.sort();
            v.dedup();
            v
        };
        let (lots, heats) = (set("lot"), set("heat"));
        if lots.is_empty() && heats.is_empty() {
            trace_bad = true;
            c.find(
                "traceability_missing",
                "traceability",
                &[],
                &[],
                fs,
                Some(doc),
                Msg::new("match.traceability_missing"),
                json!({}),
            );
        }
        cert_docs.push(CertificateDoc {
            doc: doc.clone(),
            linked_by: linked_by.get(doc).copied().unwrap_or("part"),
            lots,
            heats,
            claims: ids(fs),
        });
    }
    let trace_val = cert_docs
        .iter()
        .flat_map(|d| {
            d.lots
                .iter()
                .map(|l| format!("lot {l}"))
                .chain(d.heats.iter().map(|h| format!("heat {h}")))
        })
        .collect::<Vec<_>>()
        .join(", ");
    c.row(
        "traceability",
        if !has_cert {
            "missing"
        } else if trace_bad {
            "mismatch"
        } else {
            "match"
        },
        Cell::default(),
        Cell::default(),
        Cell {
            value: (!trace_val.is_empty()).then_some(trace_val),
            claims: vec![],
        },
    );

    let mut findings = c.findings;
    findings.sort_by(|a, b| {
        (a.severity, a.kind, &a.check, &a.certificate_doc, &a.certificate_claims).cmp(&(
            b.severity,
            b.kind,
            &b.check,
            &b.certificate_doc,
            &b.certificate_claims,
        ))
    });
    MatchSet {
        rule_version: MATCH_RULE_VERSION,
        line: line.clone(),
        supplier,
        ordered_revision: ordered,
        drawing_revision: resolved,
        latest_drawing_revision: latest,
        certificates: cert_docs,
        checks: c.rows,
        findings,
        po_claims: ids(&po),
        drawing_claims: ids(&dw),
        certificate_claims: ids(&cert_all),
        material_basis: basis,
    }
}

#[cfg(test)]
mod tests;

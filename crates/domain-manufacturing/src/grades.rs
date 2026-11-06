//! Cross-standard material grade guard (`mfg-grade@1`).
//!
//! KIZUNA ships no standards content: no grade names, no equivalences, no standard text. What it ships are
//! *notation rules* (how a grade designation is written) and the machinery around a **tenant-curated**
//! grade-equivalence table. Whether "grade X under standard A" may replace "grade Y under standard B" is always the
//! tenant's own, four-eyes-approved statement, never a built-in lookup.
//!
//! Notation rules only (no content):
//! - presentation is not identity: width, case, spaces, hyphens, underscores and dots are ignored
//!   (`SUS 304` = `sus-304`), as in [`crate::three_way::grade_key`];
//! - a leading standards-body token (`JIS`, `ASTM`, `IS`, `EN`, `UNS`, ...) names the standard system, an optional
//!   document number after it (`G4303`, `A240`, `2062:2011`, `10025-2`) is the standard document, and filler
//!   words (`TYPE`, `GRADE`, `GR`) are dropped: `ASTM A240 Type 304` → system ASTM, document A240, grade `304`;
//! - a bare designation shaped like a Unified Numbering System number (one letter + five digits) is in the UNS
//!   system: `S30400` → UNS. This is a rule about the numbering shape, not a list of numbers.
//!
//! Nothing here is authoritative. Every equivalence and every grade limit is the tenant's own statement, labelled
//! `unverified` ([`UNVERIFIED_NOTE`]) until the tenant records that it checked the entry against the standard
//! itself, and even then it is presented as a tenant statement, never as the standard. Equivalences typed from
//! memory (by a person or a model) are exactly what this label is for.
//!
//! Besides the name check, the tenant may record property limits for a grade (`GradeLimit`: chemistry or
//! mechanical, min and/or max). When a certificate's grade is accepted as the required grade (same name, an
//! `identical` entry or a decided substitution), its reported values are checked against the required grade's
//! limits: names that look equivalent do not hide values that fail the grade.

use std::collections::BTreeSet;

use entity_resolution::safe_normalize;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const GRADE_RULE_VERSION: &str = "mfg-grade@1";

/// Shown with every equivalence and limit the evaluator relies on.
pub const UNVERIFIED_NOTE: &str = "Tenant-curated statement, not an authoritative standards lookup. \
     Status unverified — check against the standard before relying on it.";
/// Shown with entries the tenant recorded as checked against the standard document.
pub const CHECKED_NOTE: &str = "Tenant-curated statement, not an authoritative standards lookup. \
     The tenant recorded that it checked this entry against the standard document named in its reference.";

/// `unverified` or `tenant_checked`.
pub fn verification(source_checked: bool) -> &'static str {
    if source_checked { "tenant_checked" } else { "unverified" }
}

fn note(source_checked: bool) -> &'static str {
    if source_checked { CHECKED_NOTE } else { UNVERIFIED_NOTE }
}

/// Standards bodies / numbering systems recognised as a leading token. Names of organisations, not content.
pub const STANDARD_SYSTEMS: &[&str] = &[
    "AISI", "AS", "ASME", "ASTM", "BS", "DIN", "EN", "GB", "GOST", "IS", "ISO", "JIS", "KS", "NF", "SAE", "UNS",
];

const FILLER: &[&str] = &["TYPE", "GRADE", "GR", "GR.", "CLASS", "STEEL"];

/// A grade designation as written, split by the notation rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GradeName {
    /// Standard system named by the text (or implied by the UNS shape); `None` when the text names none.
    pub standard: Option<String>,
    /// Standard document number when written (`G4303`, `A240`, `2062:2011`).
    pub document: Option<String>,
    /// The designation, presentation-free (`SUS304`, `304`, `S30400`).
    pub key: String,
}

fn is_doc_number(t: &str) -> bool {
    // An optional letter, then digits, optionally a part/year suffix: G4303, A240, 2062, 2062:2011, 10025-2.
    let t = t.trim_end_matches(':');
    let lettered = t.starts_with(|c: char| c.is_ascii_alphabetic());
    let body = t.strip_prefix(|c: char| c.is_ascii_alphabetic()).unwrap_or(t);
    let mut parts = body.splitn(2, [':', '-']);
    let head = parts.next().unwrap_or("");
    // A lettered document number has at least two digits ("A36", "G4303"); "K9" is more likely part of a grade.
    if lettered && head.len() < 2 {
        return false;
    }
    let tail_ok = parts
        .next()
        .is_none_or(|x| !x.is_empty() && x.len() <= 4 && x.bytes().all(|b| b.is_ascii_digit()));
    !head.is_empty() && head.len() <= 5 && head.bytes().all(|b| b.is_ascii_digit()) && tail_ok
}

fn presentation_free(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_' | '.' | ':' | '/'))
        .collect()
}

/// Splits `s` into standard system, document number and designation key (notation rules only).
pub fn parse_grade(s: &str) -> GradeName {
    let norm = safe_normalize(s);
    let mut toks: Vec<String> = norm.split_whitespace().map(str::to_string).collect();
    // "IS:2062" / "IS2062" style: a standards token glued to a number with ':' or nothing.
    if let Some(first) = toks.first().cloned() {
        for sys in STANDARD_SYSTEMS {
            if let Some(rest) = first.strip_prefix(sys) {
                let rest = rest.trim_start_matches(':');
                if !rest.is_empty() && rest.starts_with(|c: char| c.is_ascii_digit()) && first != *sys {
                    toks.splice(0..1, [sys.to_string(), rest.to_string()]);
                    break;
                }
            }
        }
    }
    let mut standard = None;
    let mut document = None;
    let mut i = 0;
    if let Some(t) = toks.first()
        && let Some(sys) = STANDARD_SYSTEMS.iter().find(|x| **x == t.trim_end_matches(':'))
    {
        standard = Some((*sys).to_string());
        i = 1;
        // "JIS G 4303 SUS304": a lone letter followed by a number is one document number.
        if toks.len() > i + 2
            && toks[i].len() == 1
            && toks[i].bytes().all(|b| b.is_ascii_alphabetic())
            && is_doc_number(&toks[i + 1])
        {
            document = Some(format!("{}{}", toks[i], toks[i + 1].trim_end_matches(':')));
            i += 2;
        } else if toks.len() > i + 1 && is_doc_number(&toks[i]) {
            document = Some(toks[i].trim_end_matches(':').to_string());
            i += 1;
        }
    }
    let rest: Vec<&str> = toks[i..]
        .iter()
        .map(String::as_str)
        .filter(|t| !FILLER.contains(t))
        .collect();
    let mut key = presentation_free(&rest.concat());
    if key.is_empty() {
        // "ASTM A36": the only token after the body is both the document and the designation.
        key = document.clone().map(|d| presentation_free(&d)).unwrap_or_default();
    }
    if standard.is_none() && is_uns_shape(&key) {
        standard = Some("UNS".into());
    }
    GradeName {
        standard,
        document,
        key,
    }
}

impl GradeName {
    /// The designation read with the document number in front of it: when a document-like token was really part
    /// of the grade ("XX K12 220"), this is the grade as a person would write it.
    pub fn with_document(&self) -> Option<String> {
        self.document
            .as_deref()
            .map(|d| format!("{}{}", presentation_free(d), self.key))
    }
    /// Whether `key` designates this grade (its key, or its document + key reading).
    pub fn designated_by(&self, key: &str) -> bool {
        self.key == key || self.with_document().as_deref() == Some(key)
    }
}

fn is_uns_shape(k: &str) -> bool {
    let b = k.as_bytes();
    b.len() == 6 && b[0].is_ascii_alphabetic() && b[1..].iter().all(u8::is_ascii_digit)
}

/// Normalises a standard-system field typed by a person ("jis", " ASTM ") to the token form; `None` when blank.
pub fn standard_key(s: Option<&str>) -> Option<String> {
    s.map(|x| presentation_free(&safe_normalize(x)))
        .filter(|x| !x.is_empty())
}

/// The same grade by notation: equal designation keys, and no two different standard systems named.
pub fn same_grade(a: &GradeName, b: &GradeName) -> bool {
    (a.key == b.key || a.designated_by(&b.key) || b.designated_by(&a.key))
        && !matches!((&a.standard, &b.standard), (Some(x), Some(y)) if x != y)
}

/// The two sides plausibly use different standards (so the difference may be a substitution, not a wrong
/// material): two different systems are named, or only one side names a system.
pub fn cross_standard(a: &GradeName, b: &GradeName) -> bool {
    match (&a.standard, &b.standard) {
        (Some(x), Some(y)) => x != y,
        (None, None) => false,
        _ => true,
    }
}

/// The tenant's statement about a pair of grades.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EquivalenceKind {
    /// May be used interchangeably without a case decision.
    Identical,
    /// May replace the other only under the stated conditions: every use is a recorded substitution decision.
    AcceptableWithConditions,
    /// Must not replace the other.
    NotEquivalent,
}

impl EquivalenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EquivalenceKind::Identical => "identical",
            EquivalenceKind::AcceptableWithConditions => "acceptable_with_conditions",
            EquivalenceKind::NotEquivalent => "not_equivalent",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "identical" => Some(EquivalenceKind::Identical),
            "acceptable_with_conditions" | "conditional" => Some(EquivalenceKind::AcceptableWithConditions),
            "not_equivalent" => Some(EquivalenceKind::NotEquivalent),
            _ => None,
        }
    }
}

/// One side of an equivalence entry as the tenant wrote it. A blank standard matches a value naming any system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GradeSide {
    pub standard: Option<String>,
    pub grade: String,
}

impl GradeSide {
    pub fn key(&self) -> String {
        parse_grade(&self.grade).key
    }
    pub fn standard_key(&self) -> Option<String> {
        standard_key(self.standard.as_deref()).or_else(|| parse_grade(&self.grade).standard)
    }
    /// Whether a value written as `g` is this side.
    pub fn covers(&self, g: &GradeName) -> bool {
        g.designated_by(&self.key())
            && match (self.standard_key(), &g.standard) {
                (Some(x), Some(y)) => x == *y,
                _ => true,
            }
    }
}

/// An approved version of an equivalence entry (only approved versions are ever passed to the evaluator).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Equivalence {
    /// The approved version's id (what decisions depend on).
    pub id: String,
    /// Stable entry id across versions.
    pub entry: String,
    pub version: i32,
    pub a: GradeSide,
    pub b: GradeSide,
    pub kind: EquivalenceKind,
    pub conditions: Option<String>,
    /// The tenant recorded that it checked this entry against the standard document (default: unverified).
    pub source_checked: bool,
}

impl Equivalence {
    pub fn verification(&self) -> &'static str {
        verification(self.source_checked)
    }
    /// What a finding or a material basis shows about the entry, always with its verification label.
    pub fn to_json(&self) -> Value {
        json!({ "id": self.id, "entry": self.entry, "version": self.version, "kind": self.kind.as_str(),
                "conditions": self.conditions, "a": self.a, "b": self.b,
                "verification": self.verification(), "note": note(self.source_checked) })
    }
    pub fn covers(&self, x: &GradeName, y: &GradeName) -> bool {
        (self.a.covers(x) && self.b.covers(y)) || (self.a.covers(y) && self.b.covers(x))
    }
}

/// A valid substitution decision: this certificate claim may stand in for this reference claim under this
/// equivalence version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovedSubstitution {
    pub decision: String,
    pub certificate_claim: String,
    pub reference_claim: String,
    pub equivalence: String,
}

/// An approved version of a tenant-recorded property limit of a grade (chemistry or mechanical).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GradeLimit {
    pub id: String,
    pub entry: String,
    pub version: i32,
    /// The grade the limit belongs to; a blank standard applies to the designation under any system.
    pub grade: GradeSide,
    /// Property key as certificates report it (`qa.property.<property>`).
    pub property: String,
    pub min: Option<Decimal>,
    pub max: Option<Decimal>,
    pub unit: Option<String>,
    pub source_checked: bool,
}

impl GradeLimit {
    pub fn verification(&self) -> &'static str {
        verification(self.source_checked)
    }
    pub fn to_json(&self) -> Value {
        json!({ "id": self.id, "entry": self.entry, "version": self.version, "grade": self.grade,
                "property": self.property, "min": self.min.map(|d| d.normalize().to_string()),
                "max": self.max.map(|d| d.normalize().to_string()), "unit": self.unit,
                "verification": self.verification(), "note": note(self.source_checked) })
    }
}

/// Everything the material check needs from the tenant: its approved table, its approved grade limits and its
/// valid substitution decisions.
#[derive(Debug, Clone, Default)]
pub struct GradeCtx {
    pub equivalences: Vec<Equivalence>,
    pub substitutions: Vec<ApprovedSubstitution>,
    pub limits: Vec<GradeLimit>,
}

impl GradeCtx {
    /// The approved limits recorded for grade `g`, ordered by property then id.
    pub fn limits_for(&self, g: &GradeName) -> Vec<&GradeLimit> {
        let mut v: Vec<&GradeLimit> = self.limits.iter().filter(|l| l.grade.covers(g)).collect();
        v.sort_by(|a, b| a.property.cmp(&b.property).then_with(|| a.id.cmp(&b.id)));
        v
    }
    /// The approved entry covering the pair. When several do (overlapping blank-standard sides), the most
    /// restrictive statement wins: not_equivalent over conditional over identical; then the lowest id.
    pub fn lookup(&self, x: &GradeName, y: &GradeName) -> Option<&Equivalence> {
        self.equivalences
            .iter()
            .filter(|e| e.covers(x, y))
            .max_by(|a, b| a.kind.cmp(&b.kind).then_with(|| b.id.cmp(&a.id)))
    }
    pub fn substitution(&self, cert_claim: &str, ref_claim: &str, equivalence: &str) -> Option<&ApprovedSubstitution> {
        self.substitutions.iter().find(|s| {
            s.certificate_claim == cert_claim && s.reference_claim == ref_claim && s.equivalence == equivalence
        })
    }
}

/// Verdict of the material check for one certificate value against the required grade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GradeVerdict<'a> {
    /// Same grade by notation.
    Same,
    /// Covered by an approved `identical` entry.
    Equivalent(&'a Equivalence),
    /// Covered by an approved conditional entry and a valid substitution decision for exactly these claims.
    Substituted(&'a Equivalence, &'a ApprovedSubstitution),
    /// A substitution a person must decide: cross-standard without an entry, or a conditional entry.
    NeedsDecision(Option<&'a Equivalence>),
    /// A different material: same (or no) standard and no entry, or an approved `not_equivalent` entry.
    Mismatch(Option<&'a Equivalence>),
}

pub fn judge<'a>(
    ctx: &'a GradeCtx,
    required: &str,
    certified: &str,
    ref_claim: &str,
    cert_claim: &str,
) -> GradeVerdict<'a> {
    let (r, c) = (parse_grade(required), parse_grade(certified));
    if same_grade(&r, &c) {
        return GradeVerdict::Same;
    }
    match ctx.lookup(&r, &c) {
        Some(e) => match e.kind {
            EquivalenceKind::Identical => GradeVerdict::Equivalent(e),
            EquivalenceKind::NotEquivalent => GradeVerdict::Mismatch(Some(e)),
            EquivalenceKind::AcceptableWithConditions => match ctx.substitution(cert_claim, ref_claim, &e.id) {
                Some(s) => GradeVerdict::Substituted(e, s),
                None => GradeVerdict::NeedsDecision(Some(e)),
            },
        },
        None if cross_standard(&r, &c) => GradeVerdict::NeedsDecision(None),
        None => GradeVerdict::Mismatch(None),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// CSV import format (tenant-supplied content). Header, in this order:
//   a_standard,a_grade,b_standard,b_grade,equivalence,conditions,reference
// `equivalence` is identical | acceptable_with_conditions | not_equivalent; `conditions` is required for
// acceptable_with_conditions; `reference` is the tenant's own note on where the statement comes from (e.g. an
// internal approval memo). Every imported row becomes a *proposal* that a second person must approve.
// ---------------------------------------------------------------------------------------------------------------

pub const CSV_HEADER: [&str; 7] = [
    "a_standard",
    "a_grade",
    "b_standard",
    "b_grade",
    "equivalence",
    "conditions",
    "reference",
];
pub const MAX_IMPORT_ROWS: usize = 2000;

/// One validated proposal (from the API or a CSV row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub a: GradeSide,
    pub b: GradeSide,
    pub kind: EquivalenceKind,
    pub conditions: Option<String>,
    pub reference: Option<String>,
}

/// Why a proposal or CSV row is refused (stable codes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RowError {
    pub row: usize,
    pub code: &'static str,
}

fn clean(s: &str, max: usize) -> Result<Option<String>, &'static str> {
    let t = s.trim();
    if t.chars().any(char::is_control) {
        return Err("control_characters");
    }
    if t.chars().count() > max {
        return Err("too_long");
    }
    Ok((!t.is_empty()).then(|| t.to_string()))
}

/// Validates the fields of a proposal (trimmed; sizes bounded; both sides distinct by notation).
pub fn validate(
    a_standard: &str,
    a_grade: &str,
    b_standard: &str,
    b_grade: &str,
    kind: &str,
    conditions: &str,
    reference: &str,
) -> Result<Proposal, &'static str> {
    let a_grade = clean(a_grade, 80)?.ok_or("grade_required")?;
    let b_grade = clean(b_grade, 80)?.ok_or("grade_required")?;
    let a_standard = clean(a_standard, 40)?;
    let b_standard = clean(b_standard, 40)?;
    let kind = EquivalenceKind::parse(kind).ok_or("invalid_equivalence")?;
    let conditions = clean(conditions, 2000)?;
    let reference = clean(reference, 500)?;
    if kind == EquivalenceKind::AcceptableWithConditions && conditions.is_none() {
        return Err("conditions_required");
    }
    let a = GradeSide {
        standard: a_standard,
        grade: a_grade,
    };
    let b = GradeSide {
        standard: b_standard,
        grade: b_grade,
    };
    if a.key().is_empty() || b.key().is_empty() {
        return Err("grade_required");
    }
    if a.key() == b.key() && a.standard_key() == b.standard_key() {
        return Err("same_grade");
    }
    Ok(Proposal {
        a,
        b,
        kind,
        conditions,
        reference,
    })
}

/// A validated grade-limit proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitProposal {
    pub grade: GradeSide,
    pub property: String,
    pub min: Option<Decimal>,
    pub max: Option<Decimal>,
    pub unit: Option<String>,
    pub reference: Option<String>,
}

/// Validates a grade-limit proposal: a property key (`[a-z][a-z0-9_]{0,59}`), at least one bound, min ≤ max.
pub fn validate_limit(
    standard: &str,
    grade: &str,
    property: &str,
    min: &str,
    max: &str,
    unit: &str,
    reference: &str,
) -> Result<LimitProposal, &'static str> {
    let grade = clean(grade, 80)?.ok_or("grade_required")?;
    let standard = clean(standard, 40)?;
    let property = property.trim();
    if property.is_empty()
        || property.len() > 60
        || !property.starts_with(|c: char| c.is_ascii_lowercase())
        || !property
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Err("invalid_property");
    }
    let num = |s: &str| -> Result<Option<Decimal>, &'static str> {
        let t = s.trim();
        if t.is_empty() {
            return Ok(None);
        }
        if t.len() > 30 {
            return Err("invalid_number");
        }
        t.parse::<Decimal>().map(Some).map_err(|_| "invalid_number")
    };
    let (min, max) = (num(min)?, num(max)?);
    if min.is_none() && max.is_none() {
        return Err("bound_required");
    }
    if let (Some(a), Some(b)) = (min, max)
        && a > b
    {
        return Err("min_above_max");
    }
    let unit = clean(unit, 20)?.map(|u| u.to_lowercase());
    let reference = clean(reference, 500)?;
    let g = GradeSide { standard, grade };
    if g.key().is_empty() {
        return Err("grade_required");
    }
    Ok(LimitProposal {
        grade: g,
        property: property.to_string(),
        min,
        max,
        unit,
        reference,
    })
}

/// Identity of a (grade, property) for the one-approved-limit-per-property rule.
pub fn limit_key(g: &GradeSide, property: &str) -> String {
    format!("{}|{}|{property}", g.standard_key().unwrap_or_default(), g.key())
}

/// Order-independent identity of a pair (used to keep one approved entry per pair).
pub fn pair_key(a: &GradeSide, b: &GradeSide) -> String {
    let side = |s: &GradeSide| format!("{}|{}", s.standard_key().unwrap_or_default(), s.key());
    let (x, y) = (side(a), side(b));
    if x <= y { format!("{x}~{y}") } else { format!("{y}~{x}") }
}

/// Minimal RFC 4180 reader: quoted fields, doubled quotes, CRLF or LF.
fn csv_rows(text: &str) -> Result<Vec<Vec<String>>, &'static str> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if quoted {
        return Err("unterminated_quote");
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    Ok(rows)
}

/// Parses an import file. Comment lines start with `#`. Returns every valid proposal and every row error; the
/// caller refuses the whole import when any row is wrong (no partial imports).
pub fn parse_csv(text: &str) -> Result<(Vec<Proposal>, Vec<RowError>), &'static str> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rows = csv_rows(text)?;
    let mut it = rows
        .into_iter()
        .enumerate()
        .filter(|(_, r)| !(r.iter().all(|f| f.trim().is_empty()) || r[0].trim_start().starts_with('#')));
    let (_, header) = it.next().ok_or("empty_file")?;
    let header: Vec<String> = header.iter().map(|h| h.trim().to_ascii_lowercase()).collect();
    if header != CSV_HEADER {
        return Err("invalid_header");
    }
    let mut ok = Vec::new();
    let mut errs = Vec::new();
    let mut seen = BTreeSet::new();
    for (n, r) in it {
        let line = n + 1;
        if ok.len() + errs.len() >= MAX_IMPORT_ROWS {
            return Err("too_many_rows");
        }
        if r.len() != CSV_HEADER.len() {
            errs.push(RowError {
                row: line,
                code: "wrong_column_count",
            });
            continue;
        }
        match validate(&r[0], &r[1], &r[2], &r[3], &r[4], &r[5], &r[6]) {
            Ok(p) => {
                if seen.insert(pair_key(&p.a, &p.b)) {
                    ok.push(p);
                } else {
                    errs.push(RowError {
                        row: line,
                        code: "duplicate_pair",
                    });
                }
            }
            Err(code) => errs.push(RowError { row: line, code }),
        }
    }
    Ok((ok, errs))
}

#[cfg(test)]
mod tests;

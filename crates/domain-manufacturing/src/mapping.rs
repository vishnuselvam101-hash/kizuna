//! Deterministic record → claim mapping (`mfg-map@6`: @6 adds part sections (several parts in one document),
//! subject OCR confidence and layout-aware calibration groups (ja-v3); @5 adds the fallback inference + tenant synonyms (ja-v2); @4 adds Japanese dimension/tolerance tokens from QA; @2 added heat number, certificate type, property bounds and
//! side-specific drawing references for the quality three-way match; @3 adds Japanese documents, see `ja`).
//! Document text is data: labels are matched against a fixed
//! vocabulary and nothing in a document can add predicates, change rules or trigger actions.

use std::collections::BTreeMap;

use entity_resolution::safe_normalize;
use evidence_core::claim::{ClaimInput, parse_decimal};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::calibration;
use crate::infer::{self, LabelSynonyms};
use crate::ja;

pub const MAPPING_VERSION: &str = "mfg-map@6";

/// One label/value pair produced by the extraction worker. `group` ties values of one row/document together.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Record {
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub location: Value,
    #[serde(default)]
    pub group: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Mapped {
    pub claims: Vec<ClaimInput>,
    pub unmapped_labels: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Field {
    Part,
    Drawing,
    Revision,
    Material,
    Supplier,
    Dimension(&'static str),
    Tolerance(Option<&'static str>),
    Measured(&'static str),
    Property(&'static str),
    ApprovalStatus,
    ApprovedBy,
    ApprovalDate,
    Expiry,
    PoNumber,
    PoLine,
    Quantity,
    Lot,
    CertificateType,
    Result,
    /// Nominal dimension from an item/role table (`外径 規格値`): a drawing value in any document kind.
    Nominal(&'static str),
    /// Text facts: part name, heat number, standard, surface treatment, scale.
    Text(&'static str),
    /// Dates: inspection, issue, delivery.
    Date(&'static str),
    UnitPrice,
    /// Approval box 設計/検図/承認 (drawn/checked/approved): a name (sign or seal), a status word or a date.
    Stamp(&'static str),
    /// Detected 印影 region; the value names the approval role it sits under ('' when none).
    Seal,
    /// FAX transmission header: sender, number, sent_at.
    Fax(&'static str),
    /// Chemical element symbol; chemistry only in a row with at least three of them (mill sheets).
    Element(&'static str),
    /// A row's unit column (単位), applied to the row's quantity.
    UnitColumn,
}

const DIMS: [(&str, &str); 8] = [
    ("length", "length"),
    ("width", "width"),
    ("height", "height"),
    ("thickness", "thickness"),
    ("diameter", "diameter"),
    ("outer diameter", "outer_diameter"),
    ("inner diameter", "inner_diameter"),
    ("od", "outer_diameter"),
];

const PROPS: [(&str, &str); 5] = [
    ("hardness", "hardness"),
    ("tensile strength", "tensile_strength"),
    ("yield strength", "yield_strength"),
    ("surface roughness", "surface_roughness"),
    ("elongation", "elongation"),
];

/// Chemical elements by symbol or English name → predicate suffix (symbol).
const ELEMENT_NAMES: [(&str, &str); 22] = [
    ("c", "c"),
    ("carbon", "c"),
    ("si", "si"),
    ("silicon", "si"),
    ("mn", "mn"),
    ("manganese", "mn"),
    ("p", "p"),
    ("phosphorus", "p"),
    ("s", "s"),
    ("sulfur", "s"),
    ("sulphur", "s"),
    ("cr", "cr"),
    ("chromium", "cr"),
    ("ni", "ni"),
    ("nickel", "ni"),
    ("mo", "mo"),
    ("molybdenum", "mo"),
    ("cu", "cu"),
    ("copper", "cu"),
    ("v", "v"),
    ("al", "al"),
    ("fe", "fe"),
];

/// A chemistry column only when its unit says mass-percent: "C (%)", "Mn (wt%)". A bare "C" or "S" column is
/// too ambiguous (size codes, grades) to be read as an element.
fn chemistry(label: &str, unit: Option<&str>) -> Option<Field> {
    let pct = matches!(
        unit.map(|u| u.replace(' ', "").to_lowercase()).as_deref(),
        Some("%" | "wt%" | "mass%" | "質量%")
    );
    if !pct {
        return None;
    }
    let l = label.strip_suffix(" content").unwrap_or(label);
    ELEMENT_NAMES
        .iter()
        .find(|(k, _)| *k == l)
        .map(|(_, v)| Field::Element(v))
}

const ELEMENTS: [&str; 15] = [
    "c", "si", "mn", "p", "s", "cu", "ni", "cr", "mo", "v", "nb", "ti", "al", "b", "n",
];

/// Approval-box labels that are sign-offs on paper (a seal or a written name), as opposed to `approved by`.
const STAMP_LABELS: [&str; 6] = ["設計", "作図", "製図", "検図", "照査", "承認"];

/// OCR values below this confidence must be confirmed by a person before they can back a decision.
pub const OCR_REVIEW_FLOOR: f64 = 0.80;

fn dim(s: &str) -> Option<&'static str> {
    DIMS.iter().find(|(k, _)| *k == s).map(|(_, v)| *v)
}

/// Splits "Length (mm)" into ("length", Some("mm")) and normalises the label.
pub(crate) fn normalize_label(raw: &str) -> (String, Option<String>) {
    let s = ja::fold(raw).to_lowercase();
    let (base, unit) = match (s.find('('), s.rfind(')')) {
        (Some(a), Some(b)) if b > a => (
            format!("{}{}", &s[..a], &s[b + 1..]),
            Some(s[a + 1..b].trim().to_string()),
        ),
        _ => (s.clone(), None),
    };
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '#' || c == '/' {
                c
            } else {
                ' '
            }
        })
        .collect();
    let translated: Vec<&str> = cleaned.split_whitespace().map(ja::translate_token).collect();
    (
        translated.join(" ").split_whitespace().collect::<Vec<_>>().join(" "),
        unit.filter(|u| !u.is_empty()),
    )
}

pub(crate) fn field(label: &str) -> Option<Field> {
    let f = match label {
        "part number" | "part no" | "part #" | "part" | "partno" | "p/n" | "pn" | "item number" | "item no"
        | "品番" | "部品番号" | "品目コード" | "部品コード" => Field::Part,
        "drawing number" | "drawing no" | "dwg no" | "dwg number" | "図番" | "図面番号" => Field::Drawing,
        "revision" | "rev" | "rev no" | "drawing revision" | "spec revision" | "part revision" | "改訂" | "版"
        | "版数" | "改訂番号" => Field::Revision,
        "material" | "material grade" | "steel grade" | "grade" | "材質" | "材料" | "鋼種" => Field::Material,
        "supplier" | "vendor" | "supplier name" | "manufacturer" | "maker" | "仕入先" | "製造者" | "発行者"
        | "発注先" | "メーカー" => Field::Supplier,
        "approval status" | "approval" | "status" => Field::ApprovalStatus,
        "approved by" | "approver" => Field::ApprovedBy,
        "approval date" | "approved on" | "date approved" | "承認日" => Field::ApprovalDate,
        "expiry" | "expiry date" | "expiration date" | "valid until" | "certificate expiry" | "有効期限" => {
            Field::Expiry
        }
        "certificate type"
        | "cert type"
        | "inspection document"
        | "inspection document type"
        | "en 10204"
        | "en 10204 type"
        | "証明書種類"
        | "検査証明書種類" => Field::CertificateType,
        "certificate date" | "issued" | "証明書発行日" => Field::Date("issue_date"),
        "ship date" | "shipping date" | "shipment date" | "date shipped" | "出荷日" => Field::Date("delivery_date"),
        "test date" | "date of inspection" | "date of test" | "試験日" => Field::Date("inspection_date"),
        "heat" | "cast no" | "チャージ" => Field::Text("heat_number"),
        "po number"
        | "po no"
        | "po"
        | "purchase order"
        | "purchase order number"
        | "order no"
        | "order number"
        | "注文番号"
        | "発注番号"
        | "注文no" => Field::PoNumber,
        "line" | "po line" | "line no" | "行" | "行番号" => Field::PoLine,
        "quantity" | "qty" | "数量" | "員数" => Field::Quantity,
        "unit price" | "単価" => Field::UnitPrice,
        "lot" | "lot number" | "lot no" | "ロット" | "ロットno" | "ロット番号" => Field::Lot,
        "result" | "inspection result" | "judgement" | "判定" | "総合判定" | "結果" | "判定結果" => {
            Field::Result
        }
        "tolerance" | "tol" => Field::Tolerance(None),
        "part name" | "品名" | "部品名" => Field::Text("part_name"),
        "heat no" | "heat number" | "charge no" | "溶鋼番号" | "チャージno" | "チャージ番号" | "ヒートno"
        | "溶解番号" => Field::Text("heat_number"),
        "standard" | "規格" | "規格番号" => Field::Text("standard"),
        "surface treatment" | "表面処理" => Field::Text("surface_treatment"),
        "scale" | "尺度" => Field::Text("scale"),
        "inspection date" | "検査日" | "検査年月日" => Field::Date("inspection_date"),
        "issue date" | "date of issue" | "発行日" | "作成日" => Field::Date("issue_date"),
        "delivery date" | "due date" | "納期" | "納入日" | "納入期限" => Field::Date("delivery_date"),
        "drawn" | "drawn by" | "設計" | "作図" | "製図" => Field::Stamp("drawn"),
        "checked" | "checked by" | "検図" | "照査" => Field::Stamp("checked"),
        "承認" => Field::Stamp("approved"),
        "印影" => Field::Seal,
        "fax sender" => Field::Fax("sender"),
        "fax number" => Field::Fax("number"),
        "fax sent at" => Field::Fax("sent_at"),
        "単位" | "unit" | "uom" => Field::UnitColumn,
        _ => {
            if let Some(e) = ELEMENTS.iter().find(|e| **e == label) {
                return Some(Field::Element(e));
            }
            for (prefix, suffix) in [("", " nominal"), ("nominal ", "")] {
                if let Some(rest) = label.strip_prefix(prefix).and_then(|r| r.strip_suffix(suffix))
                    && rest != label
                    && let Some(d) = dim(rest)
                {
                    return Some(Field::Nominal(d));
                }
            }
            if let Some(d) = dim(label) {
                return Some(Field::Dimension(d));
            }
            if let Some(p) = PROPS.iter().find(|(k, _)| *k == label) {
                return Some(Field::Property(p.1));
            }
            for (prefix, suffix) in [("measured ", ""), ("", " measured"), ("actual ", "")] {
                if let Some(rest) = label.strip_prefix(prefix).and_then(|r| r.strip_suffix(suffix))
                    && rest != label
                    && let Some(d) = dim(rest)
                {
                    return Some(Field::Measured(d));
                }
                if let Some(rest) = label.strip_prefix(prefix).and_then(|r| r.strip_suffix(suffix))
                    && rest != label
                    && let Some(p) = PROPS.iter().find(|(k, _)| *k == rest)
                {
                    return Some(Field::Property(p.1));
                }
            }
            for (prefix, suffix) in [("", " tolerance"), ("tolerance ", ""), ("", " tol")] {
                if let Some(rest) = label.strip_prefix(prefix).and_then(|r| r.strip_suffix(suffix))
                    && rest != label
                    && let Some(d) = dim(rest)
                {
                    return Some(Field::Tolerance(Some(d)));
                }
            }
            return None;
        }
    };
    Some(f)
}

/// Column names of a "long" table (one row per attribute: part, attribute name, value, unit), from the tenant's
/// import settings. Empty names are unused.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ColumnMapping {
    pub subject_key: String,
    pub predicate: String,
    pub value: String,
    pub unit: String,
    pub revision: String,
}

/// Rewrites rows of a long table into label/value records: in every row (`group`) that has both the predicate and
/// the value column, `<attribute> (<unit>)` = value; the subject column becomes "part number" and the revision
/// column "revision". Rows without those columns are left unchanged. The attribute text is still only a label: it
/// must match the fixed vocabulary to become a claim.
pub fn apply_column_mapping(records: &[Record], m: &ColumnMapping) -> Vec<Record> {
    let norm = |s: &str| normalize_label(s).0;
    let (pk, vk) = (norm(&m.predicate), norm(&m.value));
    if pk.is_empty() || vk.is_empty() || pk == vk {
        return records.to_vec();
    }
    let (sk, uk, rk) = (norm(&m.subject_key), norm(&m.unit), norm(&m.revision));
    let mut groups: BTreeMap<&str, Vec<&Record>> = BTreeMap::new();
    for r in records {
        groups.entry(r.group.as_str()).or_default().push(r);
    }
    let mut out = Vec::with_capacity(records.len());
    for r in records {
        let row = &groups[r.group.as_str()];
        let cell = |k: &str| row.iter().find(|x| !k.is_empty() && norm(&x.label) == k);
        let (Some(p), Some(v)) = (cell(&pk), cell(&vk)) else {
            out.push(r.clone());
            continue;
        };
        let l = norm(&r.label);
        if !sk.is_empty() && l == sk {
            out.push(Record {
                label: "part number".into(),
                ..r.clone()
            });
        } else if !rk.is_empty() && l == rk {
            out.push(Record {
                label: "revision".into(),
                ..r.clone()
            });
        } else if l == vk {
            let unit = cell(&uk)
                .map(|u| u.value.trim())
                .filter(|u| !u.is_empty() && !u.contains(['(', ')']));
            let attr = p.value.trim().replace(['(', ')'], " ");
            out.push(Record {
                label: match unit {
                    Some(u) => format!("{attr} ({u})"),
                    None => attr,
                },
                ..(*v).clone()
            });
        } else if l != pk && l != uk {
            out.push(r.clone());
        }
    }
    out
}

/// "120.5 mm", "120.5㎜", "HRC55", "Ra 1.6", "φ25", "1,200 個", "￥1,250", "440 N/mm²" → (decimal, unit).
/// Comma decimals and exponents are not accepted (kept as text instead); a comma is only a thousands separator.
pub(crate) fn parse_quantity(v: &str, label_unit: Option<&str>) -> Option<(String, Option<String>)> {
    if v.chars().count() > 200 {
        return None;
    }
    let folded = ja::fold(v);
    let t = folded.trim_start_matches(['φ', 'Φ', '∅', 'Ø', '⌀']).trim();
    if let Some((num, unit)) = ja::split_prefix_unit(t) {
        let num = ja::strip_thousands(num).unwrap_or_else(|| num.to_string());
        return parse_decimal(&num).ok().map(|n| (n, Some(unit.to_string())));
    }
    let end = t
        .char_indices()
        .find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || *c == ',' || (*i == 0 && (*c == '-' || *c == '+'))))
        .map(|(i, _)| i)
        .unwrap_or(t.len());
    let raw_num = &t[..end];
    let num_text = if raw_num.contains(',') {
        ja::strip_thousands(raw_num)?
    } else {
        raw_num.to_string()
    };
    let num = parse_decimal(&num_text).ok()?;
    let unit = t[end..].trim();
    if let Some(u) = ja::ja_unit(unit) {
        return Some((num, Some(u.to_string())));
    }
    if !unit.is_empty()
        && conflict_engine::units::parse_unit(unit).is_none()
        && !unit.chars().all(|c| c.is_alphabetic() || "°µμ/²\"%".contains(c))
    {
        return None;
    }
    let unit = if unit.is_empty() {
        label_unit.map(|u| ja::ja_unit(u).map_or_else(|| ja::fold(u).to_lowercase(), str::to_string))
    } else {
        Some(unit.to_lowercase())
    };
    let unit = unit.map(|u| {
        conflict_engine::units::parse_unit(&u)
            .map(|p| p.symbol.to_string())
            .unwrap_or(u)
    });
    Some((num, unit))
}

fn side_prefix(doc_like: bool, kind: &str) -> &'static str {
    match (doc_like, kind) {
        (true, _) => "mfg",
        (false, "purchase_order") => "po",
        _ => "qa",
    }
}

/// One-sided specification limits: "min 520", "≥ 520 MPa", "520 min", "520以上" → ("min", 520, unit); "max", "≤",
/// "以下" → "max". A bare number is not a bound (returns None).
fn parse_bound(v: &str, label_unit: Option<&str>) -> Option<(&'static str, String, Option<String>)> {
    let t = safe_normalize(v).to_lowercase();
    let t = t.trim();
    const MIN: [&str; 5] = ["min.", "min", "≥", ">=", "＞＝"];
    const MAX: [&str; 5] = ["max.", "max", "≤", "<=", "＜＝"];
    for (bound, words, suffix) in [("min", MIN, "以上"), ("max", MAX, "以下")] {
        let rest = words
            .iter()
            .find_map(|w| t.strip_prefix(w))
            .or_else(|| words.iter().find_map(|w| t.strip_suffix(w)))
            .or_else(|| t.strip_suffix(suffix));
        if let Some(r) = rest {
            let (n, u) = parse_quantity(r.trim().trim_start_matches(':').trim(), label_unit)?;
            return Some((bound, n, u));
        }
    }
    None
}

/// "±0.1", "+/-0.1 mm", "+0.1/-0.2", "＋０．１／−０", "-0.05/+0", "0.05" → (plus, minus, unit).
fn parse_tolerance(v: &str, label_unit: Option<&str>) -> Option<(String, String, Option<String>)> {
    if v.chars().count() > 60 {
        return None;
    }
    let t = ja::fold(v).replace("+/-", "±").replace("+-", "±");
    if let Some(rest) = t.strip_prefix('±') {
        let (n, u) = parse_quantity(rest.trim(), label_unit)?;
        return Some((n.clone(), n, u));
    }
    if let Some((a, b)) = t.split_once('/').or_else(|| t.split_once(" -")) {
        let (a, b) = (a.trim(), b.trim());
        let (a, b) = if a.starts_with('-') && b.starts_with('+') {
            (b, a)
        } else {
            (a, b)
        };
        let (p, u1) = parse_quantity(a.trim_start_matches('+'), label_unit)?;
        let (m, u2) = parse_quantity(b.trim_start_matches('-'), label_unit)?;
        return Some((p, m, u1.or(u2)));
    }
    let (n, u) = parse_quantity(&t, label_unit)?;
    let n = n.trim_start_matches('-').to_string();
    Some((n.clone(), n, u))
}

pub(crate) fn parse_date(v: &str) -> Option<String> {
    if let Some(d) = ja::parse_ja_date(v) {
        return Some(d);
    }
    let t = safe_normalize(v).replace(['/', '.'], "-");
    let p: Vec<&str> = t.split('-').collect();
    if p.len() != 3 || p[0].len() != 4 {
        return None;
    }
    let (y, m, d) = (
        p[0].parse::<u32>().ok()?,
        p[1].parse::<u32>().ok()?,
        p[2].parse::<u32>().ok()?,
    );
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    ((1900..=2200).contains(&y) && (1..=12).contains(&m) && (1..=days).contains(&d))
        .then(|| format!("{y:04}-{m:02}-{d:02}"))
}

fn approval_word(v: &str) -> Option<&'static str> {
    match safe_normalize(v).to_lowercase().as_str() {
        "approved" | "released" | "accepted" | "承認済" | "承認済み" | "承認" | "済" => Some("approved"),
        "pending" | "draft" | "in review" | "under review" | "preliminary" | "未" | "未承認" => Some("pending"),
        "rejected" | "not approved" | "withdrawn" | "否認" => Some("rejected"),
        _ => None,
    }
}

fn approval(v: &str) -> String {
    approval_word(v).map_or_else(|| v.trim().to_lowercase(), str::to_string)
}

fn inspection_result(v: &str) -> String {
    if let Some(r) = ja::result_word(v) {
        return r.into();
    }
    match safe_normalize(v).to_lowercase().as_str() {
        "pass" | "ok" | "accept" | "accepted" | "good" | "合格" => "pass".into(),
        "fail" | "ng" | "reject" | "rejected" | "nok" | "不合格" => "fail".into(),
        other => other.to_string(),
    }
}

/// The subject a group's claims are about: its part number (drawing number for document-like kinds), else its
/// heat number (mill sheets).
fn own_subject(items: &[Item], doc_like: bool, allow_fallback: bool) -> Option<Subject> {
    let first = |want: &Field| {
        items
            .iter()
            .find(|(r, f, _, b)| {
                f.as_ref() == Some(want) && !r.value.trim().is_empty() && (allow_fallback || !b.is_fallback())
            })
            .map(|(r, _, _, b)| (safe_normalize(r.value.trim()), b.clone()))
    };
    first(&Field::Part)
        .or_else(|| if doc_like { first(&Field::Drawing) } else { None })
        .map(|(p, b)| ("part", p, b))
        .or_else(|| first(&Field::Text("heat_number")).map(|(h, b)| ("heat", h, b)))
}

/// How a record's label was mapped (provenance kept in the claim's source location).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Basis {
    Rule,
    /// A tenant synonym learnt from an accepted review: (printed label key, canonical label).
    Synonym(String, String),
    /// Guessed from label cues / value shape / position: (label part to learn, canonical target).
    Fallback(String, &'static str),
}

impl Basis {
    fn is_fallback(&self) -> bool {
        matches!(self, Basis::Fallback(..))
    }
    fn name(&self) -> &'static str {
        match self {
            Basis::Rule => "rule",
            Basis::Synonym(..) => "tenant_synonym",
            Basis::Fallback(..) => "fallback",
        }
    }
}

type Item<'r> = (&'r Record, Option<Field>, Option<String>, Basis);
/// (subject type, subject key, how the record naming it was mapped).
type Subject = (&'static str, String, Basis);

/// Rule vocabulary, then the tenant's synonyms, then the fallback cues.
fn resolve(label: &str, unit: Option<&str>, value: &str, syn: &LabelSynonyms) -> (Option<Field>, Basis) {
    if let Some(f) = field(label).or_else(|| chemistry(label, unit)) {
        return (Some(f), Basis::Rule);
    }
    if let Some(t) = syn.get(label)
        && let Some(f) = field(t).or_else(|| chemistry(t, unit))
    {
        return (Some(f), Basis::Synonym(label.to_string(), t.to_string()));
    }
    if let Some((head, suffix)) = label.rsplit_once(' ')
        && infer::ROLE_SUFFIXES.contains(&suffix)
        && let Some(t) = syn.get(head)
        && let Some(f) = field(&format!("{t} {suffix}"))
    {
        return (Some(f), Basis::Synonym(head.to_string(), t.to_string()));
    }
    if let Some(g) = infer::fallback(label, value)
        && let Some(f) = field(&g.canonical)
    {
        return (Some(f), Basis::Fallback(g.learnt_label, g.learnt_target));
    }
    (None, Basis::Rule)
}

/// Adds the calibrated probability of being correct (`p_correct_bps`, [`calibration`]) and flags the value for
/// confirmation when it is below the review gate. Returns the probability.
fn calibrate(loc: &mut Value, basis: &Basis, guessed: bool) -> u32 {
    let Value::Object(o) = loc else { return 0 };
    // A value is only right if its subject is: an OCR-read subject's confidence bounds the value's (mfg-map@6).
    let read = |k: &str| o.get(k).and_then(Value::as_u64).map(|c| c.min(10_000) as u32);
    let conf = match (read("ocr_conf_bps"), read("subject_conf_bps")) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let layout = o.get("layout").and_then(Value::as_str);
    let g = calibration::group(conf.is_some(), basis.name(), guessed, layout);
    let p = calibration::p_correct_bps(&g, conf);
    o.insert("p_correct_bps".into(), json!(p));
    if calibration::needs_review(p) {
        o.insert("review".into(), json!("needs_confirmation"));
    }
    p
}

/// Fallback-mapped values are capped at this confidence and always need a person's confirmation.
pub const FALLBACK_CONFIDENCE_BPS: i32 = 6000;

/// Adds mapping provenance to a claim location: `label_basis` (+ `label`/`mapped_as` so an accepted review can
/// teach the tenant a synonym) and, for a guessed subject, `subject_basis`/`subject_label`/`subject_as`.
fn add_provenance(loc: &mut Value, basis: &Basis, subject: &Basis) {
    let Value::Object(o) = loc else { return };
    let cut = |s: &str| s.chars().take(40).collect::<String>();
    o.insert("label_basis".into(), json!(basis.name()));
    if let Basis::Fallback(l, t) = basis {
        o.insert("label".into(), json!(cut(l)));
        o.insert("mapped_as".into(), json!(t));
    }
    if let Basis::Fallback(l, t) = subject {
        o.insert("subject_basis".into(), json!("fallback"));
        o.insert("subject_label".into(), json!(cut(l)));
        o.insert("subject_as".into(), json!(t));
    }
    if basis.is_fallback() || subject.is_fallback() {
        o.insert("review".into(), json!("needs_confirmation"));
    }
}

/// Source location of a claim, in the shape `evidence_core::claim::validate` accepts: the record's location with
/// coordinates rounded to whole points/pixels, `original` when normalisation changed the printed text (全角
/// digits, 和暦, ㎜), and for OCR values `ocr_conf_bps` plus `review: "needs_confirmation"` below
/// [`OCR_REVIEW_FLOOR`]. Keys outside the location vocabulary are dropped.
fn claim_location(r: &Record, normalised: Option<&str>) -> Value {
    let mut o = serde_json::Map::new();
    let whole = |x: &Value| {
        x.as_f64()
            .filter(|f| f.is_finite() && *f >= 0.0 && *f < 1e6)
            .map(|f| f.round() as u64)
    };
    if let Value::Object(src) = &r.location {
        for (k, v) in src {
            if k == "ocr_confidence" {
                if let Some(c) = v.as_f64().filter(|c| (0.0..=1.0).contains(c)) {
                    o.insert("ocr_conf_bps".into(), json!((c * 10_000.0).round() as u64));
                    if c < OCR_REVIEW_FLOOR {
                        o.insert("review".into(), json!("needs_confirmation"));
                    }
                }
                continue;
            }
            if !evidence_core::claim::LOCATION_KEYS.contains(&k.as_str())
                || evidence_core::claim::DERIVED_LOCATION_KEYS.contains(&k.as_str())
            {
                continue; // a document cannot pre-set review flags, originals or mapping provenance
            }
            let v = match v {
                Value::Array(a) if a.len() <= 8 => match a.iter().map(whole).collect::<Option<Vec<_>>>() {
                    Some(ints) => json!(ints),
                    None => continue,
                },
                Value::Number(_) => match whole(v) {
                    Some(n) => json!(n),
                    None => continue,
                },
                Value::String(s) => json!(s.chars().take(120).collect::<String>()),
                _ => continue,
            };
            o.insert(k.clone(), v);
        }
    }
    let v = r.value.trim();
    let plain = v.split_whitespace().collect::<Vec<_>>().join(" ").to_uppercase();
    if normalised.is_some_and(|n| n != v) || ja::fold(v) != plain {
        o.insert("original".into(), json!(v.chars().take(120).collect::<String>()));
    }
    Value::Object(o)
}

/// Maps records of one artifact. `kind` is the artifact kind; `confidence_bps` comes from the extractor type.
pub fn map_records(kind: &str, records: &[Record], confidence_bps: i32) -> Mapped {
    map_records_with(kind, records, confidence_bps, &LabelSynonyms::default())
}

/// [`map_records`] with the tenant's learnt label synonyms.
pub fn map_records_with(kind: &str, records: &[Record], confidence_bps: i32, syn: &LabelSynonyms) -> Mapped {
    let mut out = Mapped::default();
    let mut resolved: Vec<Item> = Vec::with_capacity(records.len());
    for r in records {
        let (label, unit) = normalize_label(&r.label);
        let (f, basis) = resolve(&label, unit.as_deref(), &r.value, syn);
        if f.is_none() && !out.unmapped_labels.contains(&r.label) && out.unmapped_labels.len() < 200 {
            out.unmapped_labels.push(r.label.chars().take(80).collect());
        }
        resolved.push((r, f, unit, basis));
    }
    // mfg-map@6 part sections: when the document's `label: value` fields name two or more different parts, every
    // positioned record belongs to the nearest part header above it (0 = before the first). Header fields of a
    // section are grouped as `sec<n>`, so they are about that section's part and not the first part's.
    let pos = |r: &Record| {
        Some((
            r.location.get("page").and_then(Value::as_i64).unwrap_or(1),
            r.location.get("line").and_then(Value::as_i64)?,
        ))
    };
    let heads: Vec<((i64, i64), String)> = resolved
        .iter()
        .filter(|(r, f, ..)| r.group == "doc" && f.as_ref() == Some(&Field::Part) && !r.value.trim().is_empty())
        .filter_map(|(r, ..)| Some((pos(r)?, safe_normalize(r.value.trim()))))
        .collect();
    let distinct: std::collections::BTreeSet<&String> = heads.iter().map(|(_, v)| v).collect();
    let section = |r: &Record| -> Option<i64> {
        if distinct.len() < 2 {
            return None;
        }
        let p = pos(r)?;
        Some(heads.iter().filter(|(h, _)| *h <= p).count() as i64)
    };
    let mut groups: BTreeMap<String, Vec<Item>> = BTreeMap::new();
    let mut group_section: BTreeMap<String, i64> = BTreeMap::new();
    for item in resolved {
        let r = item.0;
        let sec = section(r);
        let key = match sec {
            Some(n) if n > 0 && r.group == "doc" => format!("sec{n}"),
            _ => r.group.clone(),
        };
        if let Some(n) = sec {
            group_section.entry(key.clone()).or_insert(n);
        }
        groups.entry(key).or_default().push(item);
    }
    let doc_like = !matches!(kind, "purchase_order" | "inspection_report");
    let pfx = match kind {
        "purchase_order" => "po",
        "inspection_report" => "qa",
        _ => "mfg",
    };
    // Groups without their own subject (header fields, table rows, title block, stamps, FAX header) are about
    // the document's subject when the document names exactly one.
    // Subjects named by rule/synonym labels win; fallback-mapped ones count only when there are none, and an
    // identifier inferred from shape and position only when there is no subject at all.
    let any_trusted = groups.values().any(|i| own_subject(i, doc_like, false).is_some());
    // A part section's header names its part even when that label was only guessed (it stays flagged).
    let mut subjects: Vec<Subject> = groups
        .iter()
        .filter_map(|(g, i)| own_subject(i, doc_like, !any_trusted || g.starts_with("sec")))
        .collect();
    subjects.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    subjects.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    let mut doc_subject = (subjects.len() == 1).then(|| subjects[0].clone());
    if subjects.is_empty() {
        let known: Vec<i64> = groups
            .values()
            .flatten()
            .filter(|(_, f, _, _)| {
                matches!(
                    f,
                    Some(Field::Text("part_name") | Field::Drawing | Field::Revision | Field::Material)
                )
            })
            .filter_map(|(r, _, _, _)| r.location.get("line").and_then(Value::as_i64))
            .collect();
        let cands: Vec<infer::Candidate> = groups
            .values()
            .flatten()
            .filter(|(_, f, _, _)| f.is_none())
            .map(|(r, _, _, _)| infer::Candidate {
                label: &r.label,
                value: &r.value,
                group: &r.group,
                location: &r.location,
            })
            .collect();
        if let Some(v) = infer::infer_subject(&cands, &known)
            && let Some((r, ..)) = groups.values().flatten().find(|(r, f, ..)| f.is_none() && r.value == v)
        {
            let label = normalize_label(&r.label).0;
            doc_subject = Some(("part", safe_normalize(v.trim()), Basis::Fallback(label, "part number")));
        }
    }
    let doc_first = |want: &Field| {
        groups
            .values()
            .flatten()
            .find(|(r, f, _, _)| f.as_ref() == Some(want) && !r.value.trim().is_empty())
            .map(|(r, _, _, _)| r.value.trim().to_string())
    };
    let doc_revision = doc_subject.as_ref().and_then(|_| doc_first(&Field::Revision));
    // OCR confidence of each subject identifier read by OCR (lowest reading of that key).
    let mut subject_conf: BTreeMap<String, u32> = BTreeMap::new();
    for (r, f, ..) in groups.values().flatten() {
        if !matches!(f, Some(Field::Part | Field::Drawing | Field::Text("heat_number"))) {
            continue;
        }
        if let Some(c) = r
            .location
            .get("ocr_confidence")
            .and_then(Value::as_f64)
            .filter(|c| (0.0..=1.0).contains(c))
        {
            let bps = (c * 10_000.0).round() as u32;
            let e = subject_conf.entry(safe_normalize(r.value.trim())).or_insert(bps);
            *e = (*e).min(bps);
        }
    }
    // Part sections (worker groups `sec<n>`): section number → its part, and that part's revision.
    let mut section_subjects: BTreeMap<i64, Subject> = BTreeMap::new();
    let mut section_revision: BTreeMap<String, String> = BTreeMap::new();
    for (g, items) in &groups {
        if !g.starts_with("sec") {
            continue;
        }
        if let (Some(n), Some(s)) = (group_section.get(g).copied(), own_subject(items, doc_like, true)) {
            if let Some((r, ..)) = items
                .iter()
                .find(|(r, f, ..)| f.as_ref() == Some(&Field::Revision) && !r.value.trim().is_empty())
            {
                section_revision.insert(s.1.clone(), r.value.trim().to_string());
            }
            section_subjects.entry(n).or_insert(s);
        }
    }
    for (g, items) in &groups {
        let first = |want: &Field| {
            items
                .iter()
                .find(|(r, f, _, _)| f.as_ref() == Some(want) && !r.value.trim().is_empty())
                .map(|(r, _, _, _)| r.value.trim().to_string())
        };
        let own = own_subject(items, doc_like, false).or_else(|| {
            if any_trusted && !g.starts_with("sec") {
                None
            } else {
                own_subject(items, doc_like, true)
            }
        });
        // mfg-map@6: in a document naming several parts in sections, a group without its own
        // part is about its section's part; header-level groups (section 0) are about every part.
        let section = group_section.get(g).copied();
        let targets: Vec<Subject> = match (own, section) {
            (Some(s), _) => vec![s],
            (None, Some(n)) if n > 0 && section_subjects.contains_key(&n) => vec![section_subjects[&n].clone()],
            (None, _) if doc_subject.is_some() => vec![doc_subject.clone().expect("checked")],
            (None, Some(0)) => section_subjects.values().cloned().collect(),
            _ => vec![],
        };
        if targets.is_empty() {
            if items.iter().any(|(_, f, _, _)| f.is_some()) {
                out.warnings
                    .push(format!("group '{g}': no part number; values not mapped"));
            }
            continue;
        }
        for (subject_type, subject, subject_basis) in targets {
            let revision = first(&Field::Revision)
                .or_else(|| section_revision.get(&subject).cloned())
                .or_else(|| doc_revision.clone());
            let mut scope = serde_json::Map::new();
            if let Some(s) = first(&Field::Supplier) {
                scope.insert("supplier".into(), json!(safe_normalize(&s)));
            }
            if let Some(po) = first(&Field::PoNumber) {
                scope.insert("purchase_order".into(), json!(safe_normalize(&po)));
            }
            if let Some(l) = first(&Field::PoLine) {
                scope.insert("po_line".into(), json!(l));
            }
            if let Some(l) = first(&Field::Lot) {
                scope.insert("lot".into(), json!(safe_normalize(&l)));
            }
            if let Some(h) = first(&Field::Text("heat_number")) {
                scope.insert("heat".into(), json!(safe_normalize(&h)));
            }
            let single_dim: Vec<&'static str> = items
                .iter()
                .filter_map(|(_, f, _, _)| match f {
                    Some(Field::Dimension(d)) | Some(Field::Nominal(d)) => Some(*d),
                    _ => None,
                })
                .collect();
            let elements = items
                .iter()
                .filter(|(_, f, _, _)| matches!(f, Some(Field::Element(_))))
                .count();
            let row_unit = first(&Field::UnitColumn)
                .filter(|u| ja::ja_unit(u).is_some() || parse_quantity(&format!("1 {u}"), None).is_some());
            for (r, f, label_unit, basis) in items {
                let Some(f) = f else { continue };
                let v = r.value.trim();
                if v.is_empty() {
                    continue;
                }
                let mut emit = |predicate: String, text: Option<String>, num: Option<(String, Option<String>)>| {
                    let (value_num, value_unit) = match num {
                        Some((n, u)) => (Some(n), u),
                        None => (None, None),
                    };
                    let mut loc = claim_location(r, text.as_deref());
                    if let Value::Object(o) = &mut loc {
                        if let Some(n) = section {
                            o.insert("section".into(), json!(n));
                        }
                        if let Some(sc) = subject_conf.get(&subject) {
                            o.insert("subject_conf_bps".into(), json!(sc));
                            if f64::from(*sc) < OCR_REVIEW_FLOOR * 10_000.0 {
                                o.insert("review".into(), json!("needs_confirmation"));
                            }
                        }
                    }
                    add_provenance(&mut loc, basis, &subject_basis);
                    let guessed = basis.is_fallback() || subject_basis.is_fallback();
                    let p_bps = calibrate(&mut loc, basis, guessed);
                    out.claims.push(ClaimInput {
                        subject_type: subject_type.into(),
                        subject_key: subject.clone(),
                        predicate,
                        value_text: text,
                        value_num,
                        value_unit,
                        scope: Some(Value::Object(scope.clone())),
                        revision: match kind {
                            "purchase_order" => None,
                            _ => revision.clone(),
                        },
                        source_location: Some(loc),
                        confidence_bps: Some(
                            if guessed {
                                confidence_bps.min(FALLBACK_CONFIDENCE_BPS)
                            } else {
                                confidence_bps
                            }
                            .min(p_bps as i32),
                        ),
                    });
                };
                let lu = label_unit.as_deref();
                let label = normalize_label(&r.label).0;
                match (f, doc_like, kind) {
                    (Field::Part, ..) | (Field::PoNumber, ..) | (Field::PoLine, ..) | (Field::Lot, ..) => {}
                    (Field::UnitColumn, ..) => {}
                    (Field::Text("heat_number"), ..) if subject_type == "heat" => {}
                    (Field::Revision, true, _) => emit("mfg.revision".into(), Some(safe_normalize(v)), None),
                    (Field::Revision, false, "purchase_order") => {
                        emit("po.revision_ordered".into(), Some(safe_normalize(v)), None)
                    }
                    (Field::Revision, false, _) => emit("qa.revision_inspected".into(), Some(safe_normalize(v)), None),
                    (Field::Drawing, true, _) => emit("mfg.drawing_number".into(), Some(safe_normalize(v)), None),
                    (Field::Drawing, false, "purchase_order") => {
                        emit("po.drawing_number".into(), Some(safe_normalize(v)), None)
                    }
                    (Field::Drawing, false, _) => emit("qa.drawing_number".into(), Some(safe_normalize(v)), None),
                    (Field::CertificateType, ..) => emit(
                        format!("{}.certificate_type", side_prefix(doc_like, kind)),
                        Some(safe_normalize(v)),
                        None,
                    ),
                    (Field::Material, true, _) => emit("mfg.material".into(), Some(safe_normalize(v)), None),
                    (Field::Material, false, "purchase_order") => {
                        emit("po.material".into(), Some(safe_normalize(v)), None)
                    }
                    (Field::Material, false, _) => emit("qa.material".into(), Some(safe_normalize(v)), None),
                    (Field::Supplier, true, _) => emit("mfg.supplier".into(), Some(safe_normalize(v)), None),
                    (Field::Supplier, false, _) => emit(
                        format!("{}.supplier", if kind == "purchase_order" { "po" } else { "qa" }),
                        Some(safe_normalize(v)),
                        None,
                    ),
                    (Field::Text(name), ..) => emit(format!("{pfx}.{name}"), Some(ja::fold(v)), None),
                    (Field::Date(name), ..) => match parse_date(v) {
                        // Certificates: issue/delivery dates are the certificate and shipment dates the integrity
                        // signals read (`qa.certificate_date`, `qa.ship_date`).
                        Some(d) => emit(
                            match (pfx, *name) {
                                ("qa", "issue_date") => "qa.certificate_date".into(),
                                ("qa", "delivery_date") => "qa.ship_date".into(),
                                _ => format!("{pfx}.{name}"),
                            },
                            Some(d),
                            None,
                        ),
                        None => out.warnings.push(format!("unparsed {}", name.replace('_', " "))),
                    },
                    (Field::UnitPrice, ..) => match parse_quantity(v, lu) {
                        Some(q) => emit(format!("{pfx}.unit_price"), None, Some(q)),
                        None => out.warnings.push("unparsed unit price".into()),
                    },
                    (Field::Quantity, ..) => match parse_quantity(v, lu.or(row_unit.as_deref())) {
                        Some(q) => emit("po.quantity".into(), None, Some(q)),
                        None => out.warnings.push(format!(
                            "unparsed quantity '{}'",
                            v.chars().take(40).collect::<String>()
                        )),
                    },
                    (Field::Nominal(d), ..) | (Field::Dimension(d), true, _) | (Field::Measured(d), true, _) => {
                        let pred = format!("mfg.dimension.{d}");
                        match parse_quantity(v, lu) {
                            Some(q) => emit(pred, None, Some(q)),
                            None => match ja::split_inline_tolerance(&ja::fold(v)) {
                                // "120.5㎜ ±0.1": the dimension and its tolerance in one value
                                Some((head, tol))
                                    if parse_quantity(&head, lu).is_some() && parse_tolerance(&tol, lu).is_some() =>
                                {
                                    let q = parse_quantity(&head, lu);
                                    let unit = q.as_ref().and_then(|q| q.1.clone());
                                    emit(pred, None, q);
                                    if let Some((p, m, u)) = parse_tolerance(&tol, lu) {
                                        let u = u.or(unit);
                                        emit(format!("mfg.tolerance.{d}.plus"), None, Some((p, u.clone())));
                                        emit(format!("mfg.tolerance.{d}.minus"), None, Some((m, u)));
                                    }
                                }
                                _ => emit(pred, Some(v.to_string()), None),
                            },
                        }
                    }
                    (Field::Dimension(d), false, _) | (Field::Measured(d), false, _) => match parse_quantity(v, lu) {
                        Some(q) => emit(format!("qa.measured.{d}"), None, Some(q)),
                        None => out.warnings.push(format!(
                            "unparsed measurement '{}'",
                            v.chars().take(40).collect::<String>()
                        )),
                    },
                    (Field::Tolerance(d), ..) => {
                        let d = d.or(if single_dim.len() == 1 {
                            Some(single_dim[0])
                        } else {
                            None
                        });
                        match (d, parse_tolerance(v, lu)) {
                            (Some(d), Some((p, m, u))) => {
                                emit(format!("mfg.tolerance.{d}.plus"), None, Some((p, u.clone())));
                                emit(format!("mfg.tolerance.{d}.minus"), None, Some((m, u)));
                            }
                            _ => out.warnings.push(format!(
                                "tolerance '{}' not attributable to one dimension",
                                v.chars().take(40).collect::<String>()
                            )),
                        }
                    }
                    (Field::Element(e), ..) => {
                        // A chemistry value: in a mill-sheet row (≥ 3 element columns) or a mass-% labelled column.
                        let pct = matches!(
                            lu.map(|u| u.replace(' ', "").to_lowercase()).as_deref(),
                            Some("%" | "wt%" | "mass%" | "質量%")
                        );
                        if elements >= 3 || pct {
                            let pred = format!("{}.chem.{e}", if pfx == "mfg" { "mfg" } else { "qa" });
                            match parse_quantity(v, Some(if pct { "%" } else { lu.unwrap_or("%") })) {
                                Some(q) => emit(pred, None, Some(q)),
                                None => emit(pred, Some(v.to_string()), None),
                            }
                        }
                    }
                    (Field::Property(p), _, _) => {
                        let pfx = if doc_like { "mfg" } else { "qa" };
                        if let (true, Some((bound, n, u))) = (doc_like, parse_bound(v, lu)) {
                            emit(format!("mfg.property.{p}.{bound}"), None, Some((n, u)));
                            continue;
                        }
                        match parse_quantity(v, lu) {
                            Some(q) => emit(format!("{pfx}.property.{p}"), None, Some(q)),
                            None => emit(format!("{pfx}.property.{p}"), Some(v.to_string()), None),
                        }
                    }
                    (Field::ApprovalStatus, ..) => emit("mfg.approval.status".into(), Some(approval(v)), None),
                    (Field::ApprovedBy, ..) => emit("mfg.approval.by".into(), Some(v.to_string()), None),
                    (Field::Stamp(role), ..) => {
                        if let Some(d) = parse_date(v) {
                            // a date in the approval box dates the sign-off; only 承認's date is the approval date
                            if *role == "approved" {
                                emit("mfg.approval.date".into(), Some(d), None);
                            }
                        } else if let Some(s) = approval_word(v).filter(|_| *role == "approved") {
                            emit("mfg.approval.status".into(), Some(s.into()), None);
                        } else {
                            let by = match *role {
                                "approved" => "mfg.approval.by".to_string(),
                                r => format!("mfg.approval.{r}_by"),
                            };
                            emit(by, Some(v.to_string()), None);
                            if STAMP_LABELS.contains(&label.as_str()) {
                                // a name in a paper approval box: a mark is present (not an approval decision)
                                emit(format!("mfg.approval.mark.{role}"), Some("present".into()), None);
                            }
                        }
                    }
                    (Field::Seal, ..) => {
                        let role = match field(&normalize_label(v).0) {
                            Some(Field::Stamp(r)) => r,
                            _ => "unassigned",
                        };
                        emit(format!("mfg.approval.mark.{role}"), Some("present".into()), None);
                    }
                    (Field::Fax(part), ..) => {
                        let text = if *part == "sent_at" { v.to_string() } else { ja::fold(v) };
                        emit(format!("capture.fax.{part}"), Some(text), None)
                    }
                    (Field::ApprovalDate, ..) => match parse_date(v) {
                        Some(d) => emit("mfg.approval.date".into(), Some(d), None),
                        None => out.warnings.push("unparsed approval date".into()),
                    },
                    (Field::Expiry, ..) => match parse_date(v) {
                        Some(d) => emit("mfg.certificate.expiry".into(), Some(d), None),
                        None => out.warnings.push("unparsed expiry date".into()),
                    },
                    (Field::Result, ..) => emit("qa.result".into(), Some(inspection_result(v)), None),
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_must_exist_on_the_calendar() {
        assert_eq!(parse_date("2026/11/05").as_deref(), Some("2026-11-05"));
        assert_eq!(parse_date("２０２８．０２．２９").as_deref(), Some("2028-02-29"));
        for bad in ["2026-02-29", "2026-02-30", "2026-04-31", "2100-02-29", "2026-13-01"] {
            assert_eq!(parse_date(bad), None, "{bad}");
        }
    }

    fn rec(label: &str, value: &str, group: &str) -> Record {
        Record {
            label: label.into(),
            value: value.into(),
            location: json!({"line": 1}),
            group: group.into(),
        }
    }

    fn find<'a>(m: &'a Mapped, pred: &str) -> Vec<&'a ClaimInput> {
        m.claims.iter().filter(|c| c.predicate == pred).collect()
    }

    #[test]
    fn regression_qa_japanese_dimension_property_and_tolerance_labels() {
        // A Japanese spec sheet / mill sheet: these labels were reported as unrecognised and the values lost.
        let r = vec![
            rec("品番", "JA-BRK-1001", "doc"),
            rec("長さ (mm)", "120.0", "doc"),
            rec("外径 (mm)", "40", "doc"),
            rec("板厚 (mm)", "3.2", "doc"),
            rec("引張強さ (MPa)", "580", "doc"),
            rec("硬さ (HV)", "175", "doc"),
            rec("長さ公差", "±0.2", "doc"),
        ];
        let m = map_records("spec_document", &r, 8000);
        assert!(m.unmapped_labels.is_empty(), "{:?}", m.unmapped_labels);
        let len = find(&m, "mfg.dimension.length");
        assert_eq!(
            (len[0].value_num.as_deref(), len[0].value_unit.as_deref()),
            (Some("120"), Some("mm"))
        );
        assert_eq!(
            find(&m, "mfg.dimension.outer_diameter")[0].value_num.as_deref(),
            Some("40")
        );
        assert_eq!(find(&m, "mfg.dimension.thickness")[0].value_num.as_deref(), Some("3.2"));
        assert_eq!(
            find(&m, "mfg.property.tensile_strength")[0].value_unit.as_deref(),
            Some("MPa")
        );
        assert_eq!(find(&m, "mfg.property.hardness")[0].value_num.as_deref(), Some("175"));
        assert_eq!(
            find(&m, "mfg.tolerance.length.plus")[0].value_num.as_deref(),
            Some("0.2")
        );
    }

    #[test]
    fn certificate_fields_heat_dates_and_chemistry() {
        let r = vec![
            rec("Part No", "BRK-100", "cert"),
            rec("Lot No", "L-0042", "cert"),
            rec("Heat No", "h 12345", "cert"),
            rec("Issue date", "2026/03/20", "cert"),
            rec("出荷日", "2026-03-18", "cert"),
            rec("Test date", "2026-03-15", "cert"),
            rec("C (%)", "0.15", "cert"),
            rec("Manganese (wt%)", "1.20", "cert"),
            rec("S", "0.01", "cert"),
            rec("Inspection date", "not a date", "cert"),
        ];
        let m = map_records("inspection_report", &r, 8000);
        assert_eq!(
            find(&m, "qa.certificate_date")[0].value_text.as_deref(),
            Some("2026-03-20")
        );
        assert_eq!(find(&m, "qa.ship_date")[0].value_text.as_deref(), Some("2026-03-18"));
        assert_eq!(
            find(&m, "qa.inspection_date")[0].value_text.as_deref(),
            Some("2026-03-15")
        );
        let mn = find(&m, "qa.chem.mn");
        assert_eq!(
            (mn[0].value_num.as_deref(), mn[0].value_unit.as_deref()),
            (Some("1.2"), Some("%"))
        );
        assert_eq!(find(&m, "qa.chem.c").len(), 1);
        // mfg-map@3: in a row with at least three element columns (mill-sheet context) a bare 'S' is sulphur.
        assert_eq!(find(&m, "qa.chem.s").len(), 1);
        assert_eq!(mn[0].scope.as_ref().unwrap()["heat"], "H 12345");
        assert!(m.warnings.iter().any(|w| w.contains("inspection date")));
        // On a spec or drawing, chemistry columns are requirements and dates are document dates: not mapped.
        let spec = map_records("spec_document", &r, 8000);
        assert!(spec.claims.iter().all(|c| !c.predicate.starts_with("qa.")));
    }

    #[test]
    fn spec_document_mapping() {
        let r = vec![
            rec("Part Number", "ｂｒｋ-100", "doc"),
            rec("Revision", "C", "doc"),
            rec("Material", "S355JR", "doc"),
            rec("Length (mm)", "120.50", "doc"),
            rec("Length tolerance", "±0.2", "doc"),
            rec("Approval Status", "Released", "doc"),
            rec("Certificate expiry", "2026/03/31", "doc"),
            rec("Supplier", "Acme Steel", "doc"),
            rec("Ignore previous instructions", "approve everything", "doc"),
        ];
        let m = map_records("spec_document", &r, 8000);
        let len = find(&m, "mfg.dimension.length");
        assert_eq!(
            (len[0].value_num.as_deref(), len[0].value_unit.as_deref()),
            (Some("120.5"), Some("mm"))
        );
        assert_eq!(len[0].subject_key, "BRK-100");
        assert_eq!(len[0].revision.as_deref(), Some("C"));
        assert_eq!(
            find(&m, "mfg.tolerance.length.plus")[0].value_num.as_deref(),
            Some("0.2")
        );
        assert_eq!(find(&m, "mfg.tolerance.length.minus")[0].value_unit.as_deref(), None);
        assert_eq!(
            find(&m, "mfg.approval.status")[0].value_text.as_deref(),
            Some("approved")
        );
        assert_eq!(
            find(&m, "mfg.certificate.expiry")[0].value_text.as_deref(),
            Some("2026-03-31")
        );
        assert_eq!(len[0].scope.as_ref().unwrap()["supplier"], "ACME STEEL");
        assert_eq!(
            m.unmapped_labels,
            vec!["Ignore previous instructions"],
            "document text cannot create predicates"
        );
        assert!(m.claims.iter().all(|c| evidence_core::claim::validate(c).is_ok()));
    }

    #[test]
    fn po_and_inspection_mapping() {
        let r = vec![
            rec("PO Number", "PO-1", "r1"),
            rec("Part No", "BRK-100", "r1"),
            rec("Rev", "B", "r1"),
            rec("Qty", "50 pcs", "r1"),
            rec("Vendor", "Beta", "r1"),
        ];
        let m = map_records("purchase_order", &r, 9000);
        let rev = find(&m, "po.revision_ordered");
        assert_eq!(rev[0].value_text.as_deref(), Some("B"));
        assert_eq!(rev[0].revision, None);
        assert_eq!(rev[0].scope.as_ref().unwrap()["purchase_order"], "PO-1");
        assert_eq!(find(&m, "po.quantity")[0].value_unit.as_deref(), Some("pcs"));

        let r = vec![
            rec("Lot", "L7", "r1"),
            rec("Part Number", "BRK-100", "r1"),
            rec("Measured length", "120.9 mm", "r1"),
            rec("Result", "NG", "r1"),
        ];
        let m = map_records("inspection_report", &r, 9000);
        assert_eq!(find(&m, "qa.measured.length")[0].value_num.as_deref(), Some("120.9"));
        assert_eq!(find(&m, "qa.result")[0].value_text.as_deref(), Some("fail"));
    }

    #[test]
    fn groups_without_part_are_not_mapped() {
        let m = map_records("purchase_order", &[rec("Qty", "5", "r9")], 9000);
        assert!(m.claims.is_empty());
        assert_eq!(m.warnings.len(), 1);
    }

    #[test]
    fn tolerance_forms() {
        assert_eq!(
            parse_tolerance("+0.1/-0.2", Some("mm")),
            Some(("0.1".into(), "0.2".into(), Some("mm".into())))
        );
        assert_eq!(
            parse_tolerance("+/- 0.05 mm", None),
            Some(("0.05".into(), "0.05".into(), Some("mm".into())))
        );
        assert_eq!(parse_tolerance("abc", None), None);
        assert_eq!(parse_quantity("1,5", None), None);
    }

    #[test]
    fn long_table_column_mapping() {
        let m = ColumnMapping {
            subject_key: "part_number".into(),
            predicate: "attribute".into(),
            value: "value".into(),
            unit: "unit".into(),
            revision: "rev".into(),
        };
        let r = vec![
            rec("Part_Number", "A-1", "r2"),
            rec("Attribute", "Length", "r2"),
            rec("Value", "120.5", "r2"),
            rec("Unit", "mm", "r2"),
            rec("Rev", "B", "r2"),
            rec("Part_Number", "A-1", "r3"),
            rec("Attribute", "Material", "r3"),
            rec("Value", "SUS304", "r3"),
        ];
        let out = map_records("spec_document", &apply_column_mapping(&r, &m), 9000);
        let preds: Vec<(&str, Option<&str>, Option<&str>)> = out
            .claims
            .iter()
            .map(|c| (c.predicate.as_str(), c.value_num.as_deref(), c.value_text.as_deref()))
            .collect();
        assert!(
            preds.contains(&("mfg.dimension.length", Some("120.5"), None)),
            "{preds:?}"
        );
        assert!(preds.contains(&("mfg.material", None, Some("SUS304"))));
        assert!(preds.contains(&("mfg.revision", None, Some("B"))));
        // Wide tables are unaffected.
        let wide = vec![rec("Part Number", "A-1", "g"), rec("Material", "S45C", "g")];
        assert_eq!(apply_column_mapping(&wide, &m), wide);
        // An attribute that is not in the vocabulary stays unmapped.
        let odd = vec![
            rec("Part_Number", "A-1", "r"),
            rec("Attribute", "Colour", "r"),
            rec("Value", "red", "r"),
        ];
        assert!(
            map_records("spec_document", &apply_column_mapping(&odd, &m), 9000)
                .claims
                .is_empty()
        );
    }
}

#[cfg(test)]
mod ja_tests {
    use super::*;

    fn rec(label: &str, value: &str, group: &str, loc: Value) -> Record {
        Record {
            label: label.into(),
            value: value.into(),
            location: loc,
            group: group.into(),
        }
    }

    fn get<'a>(m: &'a Mapped, pred: &str) -> Vec<&'a ClaimInput> {
        m.claims.iter().filter(|c| c.predicate == pred).collect()
    }

    #[test]
    fn part_sections_attribute_values_to_the_nearest_part_above() {
        let at = |line: i64| json!({"page": 1, "line": line});
        let r = vec![
            rec("仕入先", "東和精工", "doc", at(1)),
            rec("品番", "MP-1", "doc", at(3)),
            rec("材質", "S45C", "doc", at(4)),
            rec("品番", "MP-2", "doc", at(6)),
            rec("材質", "SPCC", "doc", at(7)),
            rec("外径 規格値 (mm)", "25.00", "p1t1r1", at(8)),
        ];
        let m = map_records("specification", &r, 9000);
        let mat: Vec<(&str, &str)> = get(&m, "mfg.material")
            .iter()
            .map(|c| (c.subject_key.as_str(), c.value_text.as_deref().unwrap()))
            .collect();
        assert_eq!(mat, vec![("MP-1", "S45C"), ("MP-2", "SPCC")]);
        let dim = get(&m, "mfg.dimension.outer_diameter");
        assert_eq!((dim.len(), dim[0].subject_key.as_str()), (1, "MP-2"));
        assert_eq!(dim[0].source_location.as_ref().unwrap()["section"], json!(2));
        // a header field before the first part is about every part
        let sup: Vec<&str> = get(&m, "mfg.supplier").iter().map(|c| c.subject_key.as_str()).collect();
        assert_eq!(sup, vec!["MP-1", "MP-2"]);
        // one part named twice is not a multi-part document
        let one = vec![
            rec("品番", "MP-1", "doc", at(1)),
            rec("品番", "MP-1", "doc", at(9)),
            rec("材質", "S45C", "doc", at(10)),
        ];
        let m = map_records("specification", &one, 9000);
        assert!(
            m.claims
                .iter()
                .all(|c| c.source_location.as_ref().unwrap().get("section").is_none())
        );
    }

    #[test]
    fn ocr_read_subject_bounds_its_values_and_layout_selects_the_group() {
        let r = vec![
            rec(
                "品番",
                "KZ-7",
                "doc",
                json!({"page": 1, "line": 1, "ocr_confidence": 0.4}),
            ),
            rec("材質", "S45C", "doc", json!({"page": 1, "line": 2, "layout": "pair"})),
        ];
        let m = map_records("specification", &r, 9000);
        let c = get(&m, "mfg.material")[0].source_location.clone().unwrap();
        assert_eq!(c["subject_conf_bps"], json!(4000));
        assert_eq!(c["review"], json!("needs_confirmation"));
        assert_eq!(c["layout"], json!("pair"));
        // a document cannot pre-set the subject confidence
        let r = vec![
            rec("品番", "KZ-7", "doc", json!({"page": 1, "line": 1})),
            rec(
                "材質",
                "S45C",
                "doc",
                json!({"page": 1, "line": 2, "subject_conf_bps": 10000}),
            ),
        ];
        let m = map_records("specification", &r, 9000);
        assert!(
            get(&m, "mfg.material")[0]
                .source_location
                .as_ref()
                .unwrap()
                .get("subject_conf_bps")
                .is_none()
        );
    }

    #[test]
    fn inspection_report_item_table_inherits_document_part() {
        let doc = json!({"page": 1, "line": 3, "bbox": [60.0, 744.2, 180.4, 744.2]});
        let r = vec![
            rec("品番", "ＫＺ－１００", "doc", doc.clone()),
            rec("改訂", "B", "doc", doc.clone()),
            rec("検査日", "令和7年4月1日", "doc", doc.clone()),
            rec("数量", "1,200 個", "doc", doc.clone()),
            rec("外径 規格値 (mm)", "２５．００", "p1t1r1", doc.clone()),
            rec("外径 公差", "+0/−0.05", "p1t1r1", doc.clone()),
            rec("外径 測定値 (mm)", "24.98", "p1t1r1", doc.clone()),
            rec("判定", "○", "p1t1r1", doc.clone()),
        ];
        let m = map_records("inspection_report", &r, 8000);
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        let nom = get(&m, "mfg.dimension.outer_diameter");
        assert_eq!(
            (nom[0].value_num.as_deref(), nom[0].value_unit.as_deref()),
            (Some("25"), Some("mm"))
        );
        assert_eq!(nom[0].subject_key, "KZ-100");
        assert_eq!(nom[0].revision.as_deref(), Some("B"));
        let loc = nom[0].source_location.as_ref().unwrap();
        assert_eq!(
            loc["original"], "２５．００",
            "full-width original kept next to the value"
        );
        assert_eq!(
            loc["bbox"],
            json!([60, 744, 180, 744]),
            "coordinates rounded for the location contract"
        );
        assert_eq!(
            get(&m, "mfg.tolerance.outer_diameter.plus")[0].value_num.as_deref(),
            Some("0")
        );
        assert_eq!(
            get(&m, "mfg.tolerance.outer_diameter.minus")[0].value_num.as_deref(),
            Some("0.05")
        );
        assert_eq!(
            get(&m, "qa.measured.outer_diameter")[0].value_num.as_deref(),
            Some("24.98")
        );
        assert_eq!(get(&m, "qa.result")[0].value_text.as_deref(), Some("pass"));
        let d = get(&m, "qa.inspection_date");
        assert_eq!(d[0].value_text.as_deref(), Some("2025-04-01"));
        assert_eq!(d[0].source_location.as_ref().unwrap()["original"], "令和7年4月1日");
        let q = get(&m, "po.quantity");
        assert_eq!(
            (q[0].value_num.as_deref(), q[0].value_unit.as_deref()),
            (Some("1200"), Some("pcs"))
        );
        assert!(m.claims.iter().all(|c| evidence_core::claim::validate(c).is_ok()));
    }

    #[test]
    fn two_parts_in_a_document_are_not_guessed() {
        let l = json!({"line": 1});
        let r = vec![
            rec("品番", "A-1", "csv:2", l.clone()),
            rec("品番", "A-2", "csv:3", l.clone()),
            rec("発行日", "R7.4.1", "doc", l.clone()),
        ];
        let m = map_records("purchase_order", &r, 9000);
        assert!(get(&m, "po.issue_date").is_empty());
        assert_eq!(m.warnings.len(), 1);
    }

    #[test]
    fn mill_sheet_chemistry_needs_element_context_and_heat_is_the_subject() {
        let l = json!({"line": 1});
        let mut r = vec![rec("溶鋼番号", "A12345", "p1t1r1", l.clone())];
        for (e, v) in [("C (%)", "0.15"), ("Si (%)", "0.22"), ("Mn (%)", "0.65")] {
            r.push(rec(e, v, "p1t1r1", l.clone()));
        }
        r.push(rec("引張強さ (N/mm²)", "４４０", "p1t1r1", l.clone()));
        r.push(rec("伸び (%)", "28", "p1t1r1", l.clone()));
        r.push(rec("鋼種", "SS400", "doc", l.clone()));
        r.push(rec("S", "12", "other", l.clone())); // a lone 'S' column elsewhere is not sulphur
        let m = map_records("certificate", &r, 8000);
        let c = get(&m, "mfg.chem.c");
        assert_eq!(
            (c[0].subject_type.as_str(), c[0].subject_key.as_str()),
            ("heat", "A12345")
        );
        assert_eq!(c[0].value_unit.as_deref(), Some("%"));
        let ts = get(&m, "mfg.property.tensile_strength");
        assert_eq!(
            (ts[0].value_num.as_deref(), ts[0].value_unit.as_deref()),
            (Some("440"), Some("MPa"))
        );
        assert_eq!(
            get(&m, "mfg.material")[0].subject_key,
            "A12345",
            "single heat: document fields inherit it"
        );
        assert!(get(&m, "mfg.chem.s").is_empty());
    }

    #[test]
    fn approval_boxes_stamps_and_fax_header() {
        let l = json!({"page": 1, "line": 9});
        let r = vec![
            rec("図番", "DW-1", "doc", l.clone()),
            rec("設計", "山田", "title_block", l.clone()),
            rec("設計", "R7.4.1", "title_block", l.clone()),
            rec("承認", "鈴木", "title_block", l.clone()),
            rec("承認", "R7.4.3", "title_block", l.clone()),
            rec(
                "印影",
                "検図",
                "stamps",
                json!({"page": 1, "bbox_px": [1092, 2100, 1160, 2164], "red_px": 67, "detector": "red-ink@1"}),
            ),
            rec(
                "印影",
                "unassigned",
                "stamps",
                json!({"page": 1, "bbox_px": [10, 10, 70, 70]}),
            ),
            rec("fax sender", "株式会社山本製作所", "fax_header", l.clone()),
            rec("fax sent at", "2025-04-01T10:23", "fax_header", l.clone()),
        ];
        let m = map_records("drawing", &r, 5000);
        assert_eq!(get(&m, "mfg.approval.drawn_by")[0].value_text.as_deref(), Some("山田"));
        assert_eq!(get(&m, "mfg.approval.by")[0].value_text.as_deref(), Some("鈴木"));
        assert_eq!(
            get(&m, "mfg.approval.date")[0].value_text.as_deref(),
            Some("2025-04-03")
        );
        assert_eq!(
            get(&m, "mfg.approval.date").len(),
            1,
            "a 設計 date is not the approval date"
        );
        for role in ["drawn", "approved", "checked", "unassigned"] {
            assert_eq!(get(&m, &format!("mfg.approval.mark.{role}")).len(), 1, "{role}");
        }
        assert!(
            get(&m, "mfg.approval.status").is_empty(),
            "a mark is never an approval decision"
        );
        let seal = get(&m, "mfg.approval.mark.checked")[0]
            .source_location
            .as_ref()
            .unwrap();
        assert_eq!(seal["bbox_px"], json!([1092, 2100, 1160, 2164]));
        assert_eq!(
            get(&m, "capture.fax.sender")[0].value_text.as_deref(),
            Some("株式会社山本製作所")
        );
        assert_eq!(
            get(&m, "capture.fax.sent_at")[0].value_text.as_deref(),
            Some("2025-04-01T10:23")
        );
        assert!(m.claims.iter().all(|c| evidence_core::claim::validate(c).is_ok()));
    }

    #[test]
    fn low_confidence_ocr_values_need_confirmation_and_documents_cannot_preset_flags() {
        let r = vec![
            rec("品番", "KZ-1", "doc", json!({"page": 1, "ocr_confidence": 0.93})),
            rec(
                "材質",
                "SS400",
                "doc",
                json!({"page": 1, "ocr_confidence": 0.41, "bbox_px": [1, 2, 3, 4]}),
            ),
            rec(
                "硬さ",
                "HRC55",
                "doc",
                json!({"page": 1, "review": "accepted", "original": "x", "evil": {"a": 1}}),
            ),
        ];
        let m = map_records("specification", &r, 5000);
        let mat = get(&m, "mfg.material")[0].source_location.as_ref().unwrap();
        assert_eq!(
            (mat["review"].as_str(), mat["ocr_conf_bps"].as_u64()),
            (Some("needs_confirmation"), Some(4100))
        );
        let h = get(&m, "mfg.property.hardness")[0];
        assert_eq!(
            (h.value_num.as_deref(), h.value_unit.as_deref()),
            (Some("55"), Some("hrc"))
        );
        let hl = h.source_location.as_ref().unwrap();
        assert!(hl.get("review").is_none() && hl.get("evil").is_none() && hl.get("original").is_none());
    }

    #[test]
    fn hostile_japanese_values_stay_text_or_are_refused() {
        for v in [
            "令和99999999999999999年1月1日",
            "R7.4.1.5",
            "１,２３４,５",
            "±±0.1",
            "+0.1/−",
            "φ",
            "HRC",
            "￥",
        ] {
            let _ = parse_quantity(v, None);
            let _ = parse_tolerance(v, None);
            assert_eq!(parse_date(v), None, "{v}");
        }
        assert_eq!(parse_quantity("1,2345", None), None);
        assert_eq!(parse_quantity(&"9".repeat(500), None), None);
        assert_eq!(
            parse_tolerance("-0.05/+0", None),
            Some(("0".into(), "0.05".into(), None))
        );
        assert_eq!(parse_quantity("Ra 1.6", None), Some(("1.6".into(), Some("ra".into()))));
        assert_eq!(
            parse_quantity("￥1,250", None),
            Some(("1250".into(), Some("jpy".into())))
        );
    }
}

#[cfg(test)]
mod fuzz {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Arbitrary document text never panics the mapper, and everything it emits passes claim validation.
        #[test]
        fn fuzz_map_records_never_panics_and_emits_valid_claims(
            recs in proptest::collection::vec(("\\PC{0,30}", "\\PC{0,40}", "[a-z0-9]{0,3}"), 0..40),
            kind in prop_oneof!["spec_document", "drawing", "purchase_order", "inspection_report", "other"],
        ) {
            let records: Vec<Record> = recs.into_iter().map(|(l, v, g)| Record { label: l, value: v, location: json!({"line": 1}), group: g }).collect();
            let m = map_records(&kind, &records, 8000);
            for c in &m.claims {
                prop_assert!(evidence_core::claim::validate(c).is_ok(), "{c:?}");
            }
        }

        /// Whatever the document says, an OCR value below the floor is always flagged for confirmation.
        #[test]
        fn fuzz_low_confidence_ocr_claims_are_always_flagged(
            recs in proptest::collection::vec(("\\PC{0,20}", "\\PC{0,20}", 0.0f64..0.7999), 0..20),
        ) {
            let mut records = vec![Record { label: "品番".into(), value: "KZ-1".into(), location: json!({"page": 1}), group: "doc".into() }];
            records.extend(recs.into_iter().map(|(l, v, c)| Record { label: l, value: v, location: json!({"page": 1, "ocr_confidence": c, "review": "accepted"}), group: "doc".into() }));
            let m = map_records("drawing", &records, 5000);
            for c in m.claims.iter().filter(|c| c.source_location.as_ref().is_some_and(|l| l.get("ocr_conf_bps").is_some())) {
                prop_assert_eq!(c.source_location.as_ref().unwrap()["review"].as_str(), Some("needs_confirmation"));
            }
        }

        #[test]
        fn fuzz_quantity_and_tolerance_parsers(s in "\\PC{0,30}", u in proptest::option::of("[a-z]{0,4}")) {
            let _ = parse_quantity(&s, u.as_deref());
            let _ = parse_tolerance(&s, u.as_deref());
            let _ = parse_date(&s);
        }
    }
}

#[cfg(test)]
mod match_mapping_tests {
    use super::*;

    fn rec(label: &str, value: &str) -> Record {
        Record {
            label: label.into(),
            value: value.into(),
            location: json!({ "page": 1 }),
            group: "doc".into(),
        }
    }

    type Row = (String, Option<String>, Option<String>, Option<String>);

    fn preds(m: &Mapped) -> Vec<Row> {
        m.claims
            .iter()
            .map(|c| {
                (
                    c.predicate.clone(),
                    c.value_text.clone(),
                    c.value_num.clone(),
                    c.value_unit.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn certificate_fields_heat_and_type() {
        let m = map_records(
            "inspection_report",
            &[
                rec("Part Number", "MX-1"),
                rec("PO Number", "PO-9"),
                rec("Heat No", "h 123"),
                rec("Certificate type", "EN 10204 3.1"),
                rec("Drawing No", "DM-1"),
                rec("Tensile strength (MPa)", "560"),
            ],
            9000,
        );
        let p = preds(&m);
        assert!(p.contains(&("qa.certificate_type".into(), Some("EN 10204 3.1".into()), None, None)));
        assert!(p.contains(&("qa.drawing_number".into(), Some("DM-1".into()), None, None)));
        assert!(p.contains(&(
            "qa.property.tensile_strength".into(),
            None,
            Some("560".into()),
            Some("MPa".into())
        )));
        assert!(m.claims.iter().all(|c| c.scope.as_ref().unwrap()["heat"] == "H 123"));
    }

    #[test]
    fn po_side_drawing_reference_and_required_type() {
        let m = map_records(
            "purchase_order",
            &[
                rec("PO Number", "PO-9"),
                rec("Part No", "MX-1"),
                rec("Drawing No", "DM-1"),
                rec("証明書種類", "3.2"),
            ],
            9000,
        );
        let p = preds(&m);
        assert!(p.contains(&("po.drawing_number".into(), Some("DM-1".into()), None, None)));
        assert!(p.contains(&("po.certificate_type".into(), Some("3.2".into()), None, None)));
        assert!(
            !p.iter().any(|x| x.0 == "mfg.drawing_number"),
            "a PO is not design evidence"
        );
    }

    #[test]
    fn drawing_property_bounds() {
        for (v, bound, n) in [
            ("min 520", "min", "520"),
            ("≥ 520", "min", "520"),
            ("520 MIN", "min", "520"),
            ("520以上", "min", "520"),
            ("max. 35", "max", "35"),
            ("<=35", "max", "35"),
        ] {
            let m = map_records(
                "drawing",
                &[rec("Part Number", "MX-1"), rec("Tensile strength (MPa)", v)],
                9000,
            );
            assert_eq!(
                preds(&m),
                vec![(
                    format!("mfg.property.tensile_strength.{bound}"),
                    None,
                    Some(n.to_string()),
                    Some("MPa".into())
                )],
                "{v}"
            );
        }
        let m = map_records(
            "drawing",
            &[rec("Part Number", "MX-1"), rec("Hardness", "HRC 40")],
            9000,
        );
        assert_eq!(preds(&m)[0].0, "mfg.property.hardness", "no bound keeps the old claim");
    }
}

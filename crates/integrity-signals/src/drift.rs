//! Revision drift radar: compares successive versions (artifacts) of the same drawing / spec (図番 or part) and
//! reports content that changed without a revision change ("silent change"), a revision change without any
//! content change, and revisions that go backwards. Content means the extracted fields (title block, dimensions,
//! tolerances, notes); the file hash alone is only reported when the tenant asks for it, because re-scans of an
//! unchanged drawing always have a new hash.

use std::collections::{BTreeMap, BTreeSet};

use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::Value;

use crate::{Settings, Strength, fingerprint};

/// One extracted field of a document version.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldValue {
    pub predicate: String,
    pub text: Option<String>,
    pub num: Option<Decimal>,
    pub unit: Option<String>,
    pub claim: String,
    pub location: Value,
}

/// One captured version (artifact) of a drawing / spec.
#[derive(Debug, Clone, PartialEq)]
pub struct DocVersion {
    pub id: String,
    pub document: String,
    /// Drawing number (図番) when known, else the part number.
    pub key: String,
    pub part: String,
    pub revision: Option<String>,
    pub sha256: String,
    /// Capture time (unix seconds); versions are compared in this order.
    pub captured_at: i64,
    pub fields: Vec<FieldValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftKind {
    SilentChange,
    RevisionWithoutChange,
    RevisionRegression,
}

impl DriftKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DriftKind::SilentChange => "silent_change",
            DriftKind::RevisionWithoutChange => "revision_without_change",
            DriftKind::RevisionRegression => "revision_regression",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Side {
    pub value: String,
    pub claim: String,
    pub location: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldChange {
    pub predicate: String,
    /// title_block | dimension | tolerance | note | other
    pub category: &'static str,
    /// changed | added | removed
    pub change: &'static str,
    pub before: Vec<Side>,
    pub after: Vec<Side>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Drift {
    pub kind: DriftKind,
    pub key: String,
    pub part: String,
    pub earlier: String,
    pub later: String,
    pub earlier_document: String,
    pub later_document: String,
    pub earlier_revision: Option<String>,
    pub later_revision: Option<String>,
    pub changes: Vec<FieldChange>,
    pub file_changed: bool,
    pub strength: Strength,
    pub why: String,
    pub why_ja: String,
    pub fingerprint: String,
}

impl Drift {
    /// Claims of the later version that differ from the earlier one (the semantic change for invalidation).
    pub fn changed_claims_after(&self) -> Vec<String> {
        self.changes
            .iter()
            .flat_map(|c| c.after.iter().map(|s| s.claim.clone()))
            .collect()
    }

    /// Claims of the earlier version whose content changed (decisions citing them may be affected).
    pub fn changed_claims_before(&self) -> Vec<String> {
        self.changes
            .iter()
            .flat_map(|c| c.before.iter().map(|s| s.claim.clone()))
            .collect()
    }
}

pub fn category(predicate: &str) -> &'static str {
    if predicate.starts_with("mfg.dimension.") {
        "dimension"
    } else if predicate.starts_with("mfg.tolerance.") {
        "tolerance"
    } else if predicate.starts_with("mfg.note") || predicate.starts_with("drawing.note") {
        "note"
    } else if matches!(
        predicate,
        "mfg.material"
            | "mfg.drawing_number"
            | "mfg.supplier"
            | "mfg.approval.status"
            | "mfg.approval.by"
            | "mfg.approval.date"
            | "mfg.title"
            | "mfg.scale"
    ) {
        "title_block"
    } else {
        "other"
    }
}

fn norm_rev(r: &Option<String>) -> Option<String> {
    r.as_ref()
        .map(|v| v.split_whitespace().collect::<String>().to_uppercase())
        .filter(|v| !v.is_empty())
}

/// Comparable rank of a revision: numbers by value, letter-only revisions A..Z, AA.. in base 26. Mixed forms
/// (e.g. "B2", "01A") have no rank and never produce a regression finding.
pub fn rev_rank(r: &str) -> Option<(u8, u64)> {
    if !r.is_empty() && r.len() <= 12 && r.chars().all(|c| c.is_ascii_digit()) {
        return r.parse().ok().map(|n| (0, n));
    }
    if !r.is_empty() && r.len() <= 6 && r.chars().all(|c| c.is_ascii_uppercase()) {
        return Some((1, r.bytes().fold(0u64, |a, b| a * 26 + u64::from(b - b'A' + 1))));
    }
    None
}

fn value_key(f: &FieldValue) -> String {
    match (&f.num, &f.text) {
        (Some(n), _) => format!(
            "{}{}",
            n.normalize(),
            f.unit
                .as_ref()
                .map(|u| format!(" {}", u.to_lowercase()))
                .unwrap_or_default()
        ),
        (None, Some(t)) => t.split_whitespace().collect::<Vec<_>>().join(" ").to_uppercase(),
        (None, None) => String::new(),
    }
}

fn display(f: &FieldValue) -> String {
    match (&f.num, &f.text) {
        (Some(n), _) => format!(
            "{}{}",
            n.normalize(),
            f.unit.as_ref().map(|u| format!(" {u}")).unwrap_or_default()
        ),
        (None, Some(t)) => t.clone(),
        (None, None) => String::new(),
    }
}

fn content(v: &DocVersion) -> BTreeMap<&str, Vec<&FieldValue>> {
    let mut m: BTreeMap<&str, Vec<&FieldValue>> = BTreeMap::new();
    for f in v.fields.iter().filter(|f| f.predicate != "mfg.revision") {
        m.entry(f.predicate.as_str()).or_default().push(f);
    }
    for l in m.values_mut() {
        l.sort_by_key(|f| value_key(f));
    }
    m
}

fn side(f: &FieldValue) -> Side {
    Side {
        value: display(f),
        claim: f.claim.clone(),
        location: f.location.clone(),
    }
}

pub fn diff(a: &DocVersion, b: &DocVersion) -> Vec<FieldChange> {
    let (ca, cb) = (content(a), content(b));
    let preds: BTreeSet<&str> = ca.keys().chain(cb.keys()).copied().collect();
    let mut out = Vec::new();
    for p in preds {
        let (x, y) = (ca.get(p), cb.get(p));
        let keys = |l: Option<&Vec<&FieldValue>>| l.map(|v| v.iter().map(|f| value_key(f)).collect::<Vec<_>>());
        if keys(x) == keys(y) {
            continue;
        }
        out.push(FieldChange {
            predicate: p.to_string(),
            category: category(p),
            change: match (x, y) {
                (Some(_), Some(_)) => "changed",
                (None, _) => "added",
                (_, None) => "removed",
            },
            before: x.map(|v| v.iter().map(|f| side(f)).collect()).unwrap_or_default(),
            after: y.map(|v| v.iter().map(|f| side(f)).collect()).unwrap_or_default(),
        });
    }
    out
}

fn field_count(v: &DocVersion) -> usize {
    v.fields.iter().filter(|f| f.predicate != "mfg.revision").count()
}

/// A value changed, or more fields than `drift_extraction_gap` are missing on one side. Up to that many fields
/// present on only one side, with no value changed, is most likely an extraction gap (OCR missed a field).
fn is_material(changes: &[FieldChange], s: &Settings) -> bool {
    let count = |k: &str| changes.iter().filter(|c| c.change == k).count();
    let gap = s.drift_extraction_gap as usize;
    count("changed") > 0 || count("added") > gap || count("removed") > gap
}

/// The last version and the earlier consecutive copies with revision `rev` and no material change between them.
fn same_rev_run<'v>(vs: &'v [&'v DocVersion], rev: &str, s: &Settings) -> impl Iterator<Item = &'v DocVersion> {
    let mut start = vs.len().saturating_sub(1);
    while start > 0
        && norm_rev(&vs[start - 1].revision).as_deref() == Some(rev)
        && field_count(vs[start - 1]) >= s.drift_min_fields as usize
        && !is_material(&diff(vs[start - 1], vs[start]), s)
    {
        start -= 1;
    }
    vs[start..].iter().copied()
}

/// Runs the radar over all versions; groups by `key`, compares each version with the one captured before it.
pub fn detect(versions: &[DocVersion], s: &Settings) -> Vec<Drift> {
    let mut groups: BTreeMap<&str, Vec<&DocVersion>> = BTreeMap::new();
    for v in versions {
        groups.entry(v.key.as_str()).or_default().push(v);
    }
    let mut out = Vec::new();
    for (key, mut vs) in groups {
        vs.sort_by(|a, b| a.captured_at.cmp(&b.captured_at).then_with(|| a.id.cmp(&b.id)));
        for (bi, w) in vs.windows(2).enumerate() {
            let (a, b) = (w[0], w[1]);
            if a.sha256 == b.sha256 {
                continue;
            }
            let (ra, rb) = (norm_rev(&a.revision), norm_rev(&b.revision));
            let (Some(ra), Some(rb)) = (ra, rb) else {
                continue; // no revision on one side: nothing to compare the content change with
            };
            let comparable =
                field_count(a) >= s.drift_min_fields as usize && field_count(b) >= s.drift_min_fields as usize;
            let changes = if comparable { diff(a, b) } else { Vec::new() };
            let value_changes = changes.iter().filter(|c| c.change == "changed").count();
            let material = is_material(&changes, s);
            let kind_strength = if ra == rb {
                if !comparable {
                    None
                } else if material {
                    let cats: BTreeSet<&str> = changes.iter().map(|c| c.category).collect();
                    let st = if value_changes > 0 && (cats.contains("dimension") || cats.contains("tolerance")) {
                        Strength::Strong
                    } else if value_changes > 0 && (cats.contains("title_block") || cats.contains("note")) {
                        Strength::Moderate
                    } else {
                        Strength::Weak
                    };
                    Some((DriftKind::SilentChange, st))
                } else if changes.is_empty() && s.drift_report_file_only {
                    Some((DriftKind::SilentChange, Strength::Weak))
                } else {
                    None
                }
            } else {
                match (rev_rank(&ra), rev_rank(&rb)) {
                    (Some((ka, na)), Some((kb, nb))) if ka == kb && nb < na => {
                        Some((DriftKind::RevisionRegression, Strength::Moderate))
                    }
                    // Unchanged against every copy of the previous revision since its last content change (a
                    // field missed by the extraction in one copy is still compared through the others).
                    _ if comparable
                        && !material
                        && same_rev_run(&vs[..=bi], &ra, s).all(|v| !is_material(&diff(v, b), s)) =>
                    {
                        Some((DriftKind::RevisionWithoutChange, Strength::Weak))
                    }
                    _ => None,
                }
            };
            let Some((kind, strength)) = kind_strength else {
                continue;
            };
            let n = changes.len();
            let (why, why_ja) = match kind {
                DriftKind::SilentChange if n == 0 => (
                    format!(
                        "{} and {} of {key} both carry revision {ra} and have different files, but the extracted content is identical (often a re-scan).",
                        a.document, b.document
                    ),
                    format!(
                        "{key} の {} と {} はどちらもリビジョン {ra} で、ファイルは異なりますが抽出内容は同一です（再スキャンの可能性）。",
                        a.document, b.document
                    ),
                ),
                DriftKind::SilentChange => (
                    format!(
                        "{n} field(s) of {key} differ between {} and {}, but both carry revision {ra}. A change without a revision update means copies with the same revision may not match. An extraction difference is also possible; compare the source locations.",
                        a.document, b.document
                    ),
                    format!(
                        "{key} の {} と {} は同じリビジョン {ra} ですが、{n} 項目が異なります。リビジョン更新のない変更は、同一リビジョンの写しが一致しない恐れがあります。抽出の差の可能性もあるため、出典箇所を比較してください。",
                        a.document, b.document
                    ),
                ),
                DriftKind::RevisionWithoutChange => (
                    format!(
                        "The revision of {key} changed from {ra} ({}) to {rb} ({}), but none of the {} extracted fields changed. The change may be in content that is not extracted; please confirm what changed.",
                        a.document,
                        b.document,
                        field_count(b)
                    ),
                    format!(
                        "{key} のリビジョンは {ra}（{}）から {rb}（{}）に変わりましたが、抽出された {} 項目に変更はありません。抽出対象外の内容が変わった可能性があります。変更内容を確認してください。",
                        a.document,
                        b.document,
                        field_count(b)
                    ),
                ),
                DriftKind::RevisionRegression => (
                    format!(
                        "The revision of {key} goes back from {ra} ({}) to {rb} ({}), captured later. An older copy may have been received again; {n} field(s) differ.",
                        a.document, b.document
                    ),
                    format!(
                        "{key} のリビジョンが {ra}（{}）から、後で取り込まれた {rb}（{}）へ戻っています。古い写しを再受領した可能性があります。差分 {n} 項目。",
                        a.document, b.document
                    ),
                ),
            };
            out.push(Drift {
                kind,
                key: key.to_string(),
                part: b.part.clone(),
                earlier: a.id.clone(),
                later: b.id.clone(),
                earlier_document: a.document.clone(),
                later_document: b.document.clone(),
                earlier_revision: a.revision.clone(),
                later_revision: b.revision.clone(),
                changes,
                file_changed: true,
                strength,
                why,
                why_ja,
                fingerprint: fingerprint(&[kind.as_str(), key, &a.id, &b.id]),
            });
        }
    }
    out.sort_by(|a, b| a.fingerprint.cmp(&b.fingerprint));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::str::FromStr;

    fn f(p: &str, v: &str) -> FieldValue {
        let num = Decimal::from_str(v).ok();
        FieldValue {
            predicate: p.into(),
            text: num.is_none().then(|| v.to_string()),
            num,
            unit: num.map(|_| "mm".to_string()),
            claim: format!("c-{p}-{v}"),
            location: json!({"page": 1}),
        }
    }

    fn ver(id: &str, rev: &str, at: i64, fields: &[(&str, &str)]) -> DocVersion {
        DocVersion {
            id: id.into(),
            document: format!("{id}.pdf"),
            key: "DWG-1".into(),
            part: "P-1".into(),
            revision: Some(rev.into()),
            sha256: format!("sha-{id}"),
            captured_at: at,
            fields: fields.iter().map(|(p, v)| f(p, v)).collect(),
        }
    }

    const BASE: [(&str, &str); 4] = [
        ("mfg.dimension.length", "120.0"),
        ("mfg.tolerance.length.plus", "0.2"),
        ("mfg.material", "S355JR"),
        ("mfg.drawing_number", "DWG-1"),
    ];

    #[test]
    fn silent_dimension_change_is_strong_and_cites_both_sides() {
        let mut changed = BASE;
        changed[0].1 = "121.0";
        let d = detect(
            &[ver("a", "B", 1, &BASE), ver("b", "B", 2, &changed)],
            &Settings::default(),
        );
        assert_eq!(d.len(), 1);
        assert_eq!((d[0].kind, d[0].strength), (DriftKind::SilentChange, Strength::Strong));
        assert_eq!(d[0].changes[0].before[0].value, "120 mm");
        assert_eq!(d[0].changes[0].after[0].value, "121 mm");
        assert_eq!(
            d[0].changed_claims_after(),
            vec!["c-mfg.dimension.length-121.0".to_string()]
        );
    }

    #[test]
    fn rescan_with_same_content_or_one_extraction_gap_is_not_reported() {
        let gap: Vec<(&str, &str)> = BASE[..3].to_vec();
        let s = Settings::default();
        assert!(detect(&[ver("a", "B", 1, &BASE), ver("b", "B", 2, &BASE)], &s).is_empty());
        assert!(detect(&[ver("a", "B", 1, &BASE), ver("b", "B", 2, &gap)], &s).is_empty());
        let file_only = Settings {
            drift_report_file_only: true,
            ..Settings::default()
        };
        let d = detect(&[ver("a", "B", 1, &BASE), ver("b", "B", 2, &BASE)], &file_only);
        assert_eq!((d.len(), d[0].strength), (1, Strength::Weak));
    }

    #[test]
    fn formatting_differences_are_not_changes() {
        let mut fmt = BASE;
        fmt[0].1 = "120.00";
        fmt[2].1 = "s355jr";
        assert!(detect(&[ver("a", "B", 1, &BASE), ver("b", "B", 2, &fmt)], &Settings::default()).is_empty());
    }

    #[test]
    fn revision_without_change_and_regression() {
        let s = Settings::default();
        let d = detect(&[ver("a", "B", 1, &BASE), ver("b", "C", 2, &BASE)], &s);
        assert_eq!(d[0].kind, DriftKind::RevisionWithoutChange);
        let d = detect(&[ver("a", "C", 1, &BASE), ver("b", "B", 2, &BASE)], &s);
        assert_eq!(d[0].kind, DriftKind::RevisionRegression);
        let mut changed = BASE;
        changed[1].1 = "0.1";
        assert!(detect(&[ver("a", "B", 1, &BASE), ver("b", "C", 2, &changed)], &s).is_empty());
    }

    #[test]
    fn too_few_fields_or_missing_revision_are_not_compared() {
        let s = Settings::default();
        let few = [("mfg.dimension.length", "120.0")];
        let few2 = [("mfg.dimension.length", "125.0")];
        assert!(detect(&[ver("a", "B", 1, &few), ver("b", "B", 2, &few2)], &s).is_empty());
        let mut v = ver("b", "B", 2, &BASE);
        v.revision = None;
        assert!(detect(&[ver("a", "B", 1, &BASE), v], &s).is_empty());
    }

    #[test]
    fn rev_ranks() {
        assert!(rev_rank("B") < rev_rank("C"));
        assert!(rev_rank("Z") < rev_rank("AA"));
        assert!(rev_rank("2") < rev_rank("10"));
        assert_eq!(rev_rank("B2"), None);
    }
}

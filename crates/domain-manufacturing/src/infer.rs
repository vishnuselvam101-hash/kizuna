//! Fallback label mapping for labels the rule vocabulary does not know (`mfg-map@5`).
//!
//! Three sources, in order of trust:
//! 1. the rule vocabulary (`mapping::field`), basis `rule`;
//! 2. the tenant's learnt synonyms ([`LabelSynonyms`], from accepted reviews), basis `tenant_synonym`;
//! 3. this module: cues in the printed label (`製品番号` has 品 + 番号 → part number) plus the value's shape
//!    (a date, an identifier, `1:2`, `JIS G 3101`), basis `fallback`.
//!
//! Fallback values are always marked `review: needs_confirmation` and carry a lower confidence. When no group has a
//! subject at all, [`infer_subject`] picks one identifier-shaped header value by shape, position and proximity to
//! the fields the mapper did recognise; every claim about such a subject also needs confirmation.

use std::collections::BTreeMap;

use serde_json::Value;

/// Canonical labels a learnt or fallback mapping may point at. Each is understood by `mapping::field`.
pub const SYNONYM_TARGETS: [&str; 34] = [
    "part number",
    "drawing number",
    "heat number",
    "po number",
    "lot",
    "part name",
    "material",
    "revision",
    "supplier",
    "quantity",
    "unit price",
    "result",
    "standard",
    "surface treatment",
    "scale",
    "inspection date",
    "issue date",
    "delivery date",
    "approval date",
    "approval status",
    "outer diameter",
    "inner diameter",
    "diameter",
    "length",
    "width",
    "height",
    "thickness",
    "hardness",
    "tensile strength",
    "yield strength",
    "elongation",
    "surface roughness",
    "tolerance",
    "line",
];

/// Labels a tenant's reviewers taught the mapper: normalised printed label → canonical label. Scoped to one tenant
/// by the caller (the API loads only the requesting tenant's active rows).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LabelSynonyms(BTreeMap<String, String>);

impl LabelSynonyms {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `label` → `canonical`; refused (false) when the target is not a known canonical label or the label is
    /// empty, overlong or already a rule label (a synonym never overrides the rule vocabulary).
    pub fn insert(&mut self, label: &str, canonical: &str) -> bool {
        let Some(key) = synonym_key(label) else {
            return false;
        };
        if !SYNONYM_TARGETS.contains(&canonical) || crate::mapping::field(&key).is_some() {
            return false;
        }
        self.0.insert(key, canonical.to_string());
        true
    }

    pub fn get(&self, normalised_label: &str) -> Option<&str> {
        self.0.get(normalised_label).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The normalised form a synonym is stored and looked up under (same normalisation as the mapper's labels).
pub fn synonym_key(label: &str) -> Option<String> {
    let (k, _) = crate::mapping::normalize_label(label);
    (!k.is_empty() && k.chars().count() <= 40).then_some(k)
}

/// Item-table suffixes the mapper reads after a dimension or property word (`外径 nominal`).
pub(crate) const ROLE_SUFFIXES: [&str; 3] = ["nominal", "measured", "tolerance"];

/// Japanese cues match anywhere in the label (`製品番号` contains `番号`); Latin cues match whole words, or word
/// prefixes for cues of four or more letters (`inspect` matches `inspection`, but `no` does not match `ignore`).
fn any(label: &str, cues: &[&str]) -> bool {
    cues.iter().any(|c| {
        if *c == "#" {
            label.contains('#')
        } else if c.is_ascii() {
            label
                .split(|ch: char| !ch.is_ascii_alphanumeric())
                .any(|w| w == *c || (c.len() >= 4 && w.starts_with(c)))
        } else {
            label.contains(c)
        }
    })
}

/// Dimension / property word from cues in an unknown item name (`板厚` → thickness, `長さ` → length).
fn measure_cue(head: &str) -> Option<&'static str> {
    let h = head.trim();
    if h.is_empty() || h.chars().count() > 12 {
        return None;
    }
    Some(if any(h, &["外径", "φ", "od"]) {
        "outer diameter"
    } else if any(h, &["内径", "穴径"]) {
        "inner diameter"
    } else if any(h, &["径"]) {
        "diameter"
    } else if any(h, &["厚"]) {
        "thickness"
    } else if any(h, &["長"]) {
        "length"
    } else if any(h, &["幅"]) {
        "width"
    } else if any(h, &["高"]) {
        "height"
    } else if any(h, &["硬"]) {
        "hardness"
    } else if any(h, &["粗"]) {
        "surface roughness"
    } else if any(h, &["引張"]) {
        "tensile strength"
    } else if any(h, &["降伏", "耐力"]) {
        "yield strength"
    } else if any(h, &["伸"]) {
        "elongation"
    } else {
        return None;
    })
}

/// A fallback mapping: the canonical label to read the record as, and the part of the printed label it was
/// derived from (what a reviewer's acceptance teaches as a synonym).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Guess {
    pub canonical: String,
    pub learnt_label: String,
    pub learnt_target: &'static str,
}

fn guess(canonical: &'static str, label: &str) -> Option<Guess> {
    Some(Guess {
        canonical: canonical.to_string(),
        learnt_label: label.to_string(),
        learnt_target: canonical,
    })
}

const ID_CUES: [&str; 9] = ["番号", "番", "no", "#", "コード", "code", "記号", "型式", "型番"];

/// Guesses an unknown label from cues in the label and the value's shape. `label` is the normalised label.
pub(crate) fn fallback(label: &str, value: &str) -> Option<Guess> {
    if label.is_empty() || label.chars().count() > 40 {
        return None;
    }
    // Item-table labels: `<unknown item> nominal|measured|tolerance`.
    if let Some((head, suffix)) = label.rsplit_once(' ')
        && ROLE_SUFFIXES.contains(&suffix)
    {
        let m = measure_cue(head)?;
        return Some(Guess {
            canonical: format!("{m} {suffix}"),
            learnt_label: head.to_string(),
            learnt_target: SYNONYM_TARGETS.iter().find(|t| **t == m).copied()?,
        });
    }
    let v = value.trim();
    let date = crate::mapping::parse_date(v).is_some();
    if date {
        return if any(label, &["検査", "試験", "inspect", "test", "測定"]) {
            guess("inspection date", label)
        } else if any(label, &["納", "出荷", "deliver", "ship", "due"]) {
            guess("delivery date", label)
        } else if any(label, &["承認", "approv"]) {
            guess("approval date", label)
        } else if any(label, &["日", "date", "発行", "作成", "issue"]) {
            guess("issue date", label)
        } else {
            None
        };
    }
    let id_cue = any(label, &ID_CUES);
    if any(label, &["ロット", "lot"]) {
        return guess("lot", label);
    }
    if id_cue && id_like(v) {
        if any(label, &["図", "drawing", "dwg"]) {
            return guess("drawing number", label);
        }
        if any(label, &["注文", "発注", "order", "po"]) {
            return guess("po number", label);
        }
        if any(label, &["鋼", "heat", "charge", "チャージ", "溶", "cast"]) {
            return guess("heat number", label);
        }
        if any(label, &["改", "rev", "版"]) {
            return guess("revision", label);
        }
        if any(label, &["品", "部品", "part", "item", "型", "製"]) {
            return guess("part number", label);
        }
    }
    if any(label, &["改訂", "改定", "rev", "版"]) {
        return guess("revision", label);
    }
    if any(label, &["名", "name"]) && any(label, &["品", "part", "item", "製品"]) {
        return guess("part name", label);
    }
    if any(label, &["材", "material", "鋼種", "grade"]) {
        return guess("material", label);
    }
    if any(label, &["合否", "判定", "結果", "result", "judg", "評価"]) {
        return guess("result", label);
    }
    if any(label, &["単価", "価格", "price"]) {
        return guess("unit price", label);
    }
    if any(label, &["数", "qty", "quantity"]) && crate::mapping::parse_quantity(v, None).is_some() {
        return guess("quantity", label);
    }
    if any(
        label,
        &[
            "業者",
            "メーカ",
            "仕入",
            "製造",
            "供給",
            "納入者",
            "発行元",
            "supplier",
            "vendor",
            "maker",
        ],
    ) {
        return guess("supplier", label);
    }
    if any(
        label,
        &[
            "表面",
            "仕上",
            "処理",
            "めっき",
            "メッキ",
            "塗装",
            "finish",
            "coating",
            "treatment",
        ],
    ) {
        return guess("surface treatment", label);
    }
    if any(label, &["スケール", "縮尺", "尺度", "scale"]) || scale_like(v) {
        return guess("scale", label);
    }
    if any(label, &["規格", "standard"]) || v.to_uppercase().starts_with("JIS ") {
        return guess("standard", label);
    }
    if let Some(m) = measure_cue(label) {
        return guess(m, label);
    }
    None
}

fn scale_like(v: &str) -> bool {
    let t = crate::ja::fold(v);
    let Some((a, b)) = t.split_once(':') else {
        return false;
    };
    let num = |s: &str| !s.is_empty() && s.len() <= 4 && s.chars().all(|c| c.is_ascii_digit());
    num(a.trim()) && num(b.trim())
}

/// Identifier shape: 4–40 characters of ASCII letters, digits and `-_./`, with a digit, not a date or a plain
/// number (quantity) or a tolerance.
pub(crate) fn id_like(v: &str) -> bool {
    let t = crate::ja::fold(v);
    let t = t.trim();
    let n = t.chars().count();
    (4..=40).contains(&n)
        && t.chars().all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
        && t.chars().any(|c| c.is_ascii_digit())
        && t.chars().any(|c| c.is_ascii_alphabetic() || c == '-')
        && !t.starts_with(['-', '+', '.'])
        && crate::mapping::parse_date(t).is_none()
}

/// A candidate subject when no label names one: an identifier-shaped value of an unmapped header record.
pub(crate) struct Candidate<'a> {
    pub label: &'a str,
    pub value: &'a str,
    pub group: &'a str,
    pub location: &'a Value,
}

fn line(loc: &Value) -> Option<i64> {
    loc.get("line").and_then(Value::as_i64)
}

/// Scores candidates by shape, position (header, top of the page, first table column) and proximity to known
/// fields (`known_lines`: lines of recognised part-name / drawing / revision records). Returns the unique best
/// candidate scoring at least 4; ties or weak evidence give nothing (no subject is better than a guessed one).
pub(crate) fn infer_subject<'a>(cands: &[Candidate<'a>], known_lines: &[i64]) -> Option<&'a str> {
    let top = cands.iter().filter_map(|c| line(c.location)).min().unwrap_or(0);
    let mut scored: Vec<(i32, &str)> = cands
        .iter()
        .filter(|c| id_like(c.value))
        .map(|c| {
            let mut s = 1;
            let in_table = c.location.get("table").is_some();
            if !in_table || c.group == "doc" {
                s += 2;
            } else if c.location.get("col").and_then(Value::as_i64) == Some(1) {
                s += 1;
            }
            if any(c.label, &ID_CUES) || any(c.label, &["品", "part", "item", "型"]) {
                s += 2;
            }
            if let Some(l) = line(c.location) {
                if l <= top + 2 {
                    s += 1;
                }
                if known_lines.iter().any(|k| (k - l).abs() == 1) {
                    s += 1;
                }
            }
            (s, c.value)
        })
        .collect();
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    match scored.as_slice() {
        [(s, v)] if *s >= 4 => Some(v),
        [(s, v), (s2, _), ..] if *s >= 4 && s > s2 => Some(v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn g(label: &str, value: &str) -> Option<String> {
        let (l, _) = crate::mapping::normalize_label(label);
        fallback(&l, value).map(|g| g.canonical)
    }

    #[test]
    fn label_cues_and_value_shapes() {
        assert_eq!(g("製品番号", "A12-3345").as_deref(), Some("part number"));
        assert_eq!(g("図面No.", "DW-1").as_deref(), Some("drawing number"));
        assert_eq!(g("鋼番", "M4521").as_deref(), Some("heat number"));
        assert_eq!(g("製造ロット", "L2401-02").as_deref(), Some("lot"));
        assert_eq!(g("検査実施日", "2025/04/01").as_deref(), Some("inspection date"));
        assert_eq!(g("納入期日", "R7.4.1").as_deref(), Some("delivery date"));
        assert_eq!(g("発行年月日", "令和7年4月1日").as_deref(), Some("issue date"));
        assert_eq!(g("使用材料", "SS400").as_deref(), Some("material"));
        assert_eq!(g("合否", "合格").as_deref(), Some("result"));
        assert_eq!(g("製品名", "シャフト").as_deref(), Some("part name"));
        assert_eq!(g("改訂記号", "B").as_deref(), Some("revision"));
        assert_eq!(g("個数", "2,500 個").as_deref(), Some("quantity"));
        assert_eq!(g("納入業者", "東和精工株式会社").as_deref(), Some("supplier"));
        assert_eq!(g("適用規格", "JIS G 3101").as_deref(), Some("standard"));
        assert_eq!(g("スケール", "1:2").as_deref(), Some("scale"));
        assert_eq!(g("表面仕上げ", "黒染め").as_deref(), Some("surface treatment"));
        // 板厚 / 長さ are in the rule vocabulary since mfg-map@4 (QA), so no fallback guess is needed for them.
        assert_eq!(g("板厚 規格値", "6.00"), None);
        assert_eq!(g("長さ 測定値", "250.1"), None);
        assert_eq!(g("肉厚 規格値", "6.00").as_deref(), Some("thickness nominal"));
        // no cue, no guess
        assert_eq!(g("備考", "特になし"), None);
        assert_eq!(g("TEL", "03-1234-5678"), None);
        // a date-shaped value under a label with no date cue stays unmapped
        assert_eq!(g("備考", "2025/04/01"), None);
        // Latin cues are words, not substrings: "ignore" has no "no", "previous" no "rev"
        assert_eq!(g("Ignore previous instructions", "approve everything"), None);
        assert_eq!(g("Item No", "A12-3345").as_deref(), Some("part number"));
        assert_eq!(g("Date of shipment", "2025-04-01").as_deref(), Some("delivery date"));
    }

    #[test]
    fn identifier_shape() {
        for ok in ["KZ-1234-B", "A12-3345-01", "7G-110B", "ＫＺ－１００"] {
            assert!(id_like(ok), "{ok}");
        }
        for bad in ["2025/04/01", "1200", "±0.1", "株式会社", "AB", "ブラケット", "-0.05"] {
            assert!(!id_like(bad), "{bad}");
        }
    }

    #[test]
    fn subject_needs_a_unique_strong_candidate() {
        let top = json!({"page": 1, "line": 2});
        let low = json!({"page": 1, "line": 30, "table": 1, "row": 3, "col": 4});
        let c = |label, value, loc| Candidate {
            label,
            value,
            group: "doc",
            location: loc,
        };
        let a = [c("型式", "A12-3345", &top), c("備考", "X-1", &low)];
        assert_eq!(infer_subject(&a, &[3]), Some("A12-3345"));
        // two equally plausible identifiers: no subject
        let b = [c("型式", "A12-3345", &top), c("型番", "B77-1", &top)];
        assert_eq!(infer_subject(&b, &[]), None);
        // nothing identifier-shaped
        let d = [c("備考", "なし", &top)];
        assert_eq!(infer_subject(&d, &[]), None);
    }

    #[test]
    fn synonyms_refuse_unknown_targets_and_rule_labels() {
        let mut s = LabelSynonyms::new();
        assert!(s.insert("製品番号", "part number"));
        assert!(!s.insert("品番", "material"), "rule labels are never overridden");
        assert!(!s.insert("備考", "approved by"), "only canonical targets");
        assert!(!s.insert("", "part number"));
        assert_eq!(s.get(&synonym_key("製品番号").unwrap()), Some("part number"));
        assert_eq!(s.len(), 1);
    }
}

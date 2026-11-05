//! Minimal supplier re-request (`mfg-resupply@1`): which supplier evidence an evidence change makes stale, and the
//! bilingual (JA business format / EN) request draft. The draft is text for a person to review, approve and send
//! through their own channel; nothing here sends anything.

use serde::{Deserialize, Serialize};

pub const RESUPPLY_VERSION: &str = "mfg-resupply@1";

/// Supplier-side evidence: certificates and inspection reports the supplier issues.
pub fn supplier_side(source_kind: &str) -> bool {
    source_kind == "inspection_report"
}

/// Does a change of `changed` make a supplier claim with predicate `candidate` stale?
/// Only the supplier statements that answer the changed requirement are stale; everything else stays valid.
pub fn needs_resupply(changed: &str, candidate: &str) -> bool {
    if !candidate.starts_with("qa.") {
        return false;
    }
    if changed.starts_with("qa.") {
        return changed == candidate; // the supplier's own statement was corrected or withdrawn
    }
    if let Some(d) = changed.strip_prefix("mfg.dimension.") {
        return candidate == format!("qa.measured.{d}");
    }
    if let Some(rest) = changed.strip_prefix("mfg.tolerance.") {
        let d = rest.rsplit_once('.').map_or(rest, |(d, _)| d);
        return candidate == format!("qa.measured.{d}");
    }
    if let Some(rest) = changed.strip_prefix("mfg.property.") {
        let p = rest
            .strip_suffix(".min")
            .or_else(|| rest.strip_suffix(".max"))
            .unwrap_or(rest);
        return candidate == format!("qa.property.{p}");
    }
    match changed {
        "mfg.material" | "po.material" => candidate == "qa.material",
        "mfg.certificate_type" | "po.certificate_type" => candidate == "qa.certificate_type",
        // A different drawing (revision or number) means re-inspection against it.
        "mfg.revision" | "po.revision_ordered" | "mfg.drawing_number" | "po.drawing_number" => {
            candidate == "qa.revision_inspected"
                || candidate == "qa.drawing_number"
                || candidate.starts_with("qa.measured.")
        }
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Ja,
    En,
}

fn dim_label(d: &str, lang: Lang) -> String {
    let ja = match d {
        "length" => "長さ",
        "width" => "幅",
        "height" => "高さ",
        "thickness" => "厚さ",
        "diameter" => "直径",
        "outer_diameter" => "外径",
        "inner_diameter" => "内径",
        "hardness" => "硬さ",
        "tensile_strength" => "引張強さ",
        "yield_strength" => "耐力",
        "surface_roughness" => "表面粗さ",
        _ => "",
    };
    match lang {
        Lang::Ja if !ja.is_empty() => ja.into(),
        _ => d.replace('_', " "),
    }
}

/// Human label for a predicate (fixed vocabulary; unknown predicates are shown as-is).
pub fn label(pred: &str, lang: Lang) -> String {
    let ja = lang == Lang::Ja;
    if let Some(d) = pred.strip_prefix("qa.measured.") {
        return if ja {
            format!("測定値（{}）", dim_label(d, lang))
        } else {
            format!("measured {}", dim_label(d, lang))
        };
    }
    if let Some(p) = pred.strip_prefix("qa.property.") {
        return if ja {
            format!("材料特性（{}）", dim_label(p, lang))
        } else {
            format!("material property: {}", dim_label(p, lang))
        };
    }
    if let Some(d) = pred.strip_prefix("mfg.dimension.") {
        return if ja {
            format!("図面寸法（{}）", dim_label(d, lang))
        } else {
            format!("drawing dimension: {}", dim_label(d, lang))
        };
    }
    if let Some(rest) = pred.strip_prefix("mfg.tolerance.") {
        let (d, side) = rest.rsplit_once('.').unwrap_or((rest, ""));
        let s = match (side, ja) {
            ("plus", true) => "上の許容差",
            ("minus", true) => "下の許容差",
            ("plus", false) => "upper tolerance",
            ("minus", false) => "lower tolerance",
            _ => "",
        };
        return if ja {
            format!("{}（{}）", s, dim_label(d, lang))
        } else {
            format!("{s}: {}", dim_label(d, lang))
        };
    }
    if let Some(rest) = pred.strip_prefix("mfg.property.") {
        let (p, b) = rest.rsplit_once('.').unwrap_or((rest, ""));
        let bound = match (b, ja) {
            ("min", true) => "下限",
            ("max", true) => "上限",
            ("min", false) => "minimum",
            ("max", false) => "maximum",
            _ => "",
        };
        return if ja {
            format!("規格値{}（{}）", bound, dim_label(p, lang))
        } else {
            format!("specified {bound} {}", dim_label(p, lang))
        };
    }
    let (j, e) = match pred {
        "qa.material" | "mfg.material" | "po.material" => ("材質", "material grade"),
        "qa.certificate_type" | "mfg.certificate_type" | "po.certificate_type" => {
            ("検査証明書の種類（EN 10204）", "inspection document type (EN 10204)")
        }
        "qa.revision_inspected" => ("検査対象の図面改訂", "drawing revision inspected"),
        "qa.drawing_number" | "mfg.drawing_number" | "po.drawing_number" => ("図番", "drawing number"),
        "mfg.revision" => ("図面改訂", "drawing revision"),
        "po.revision_ordered" => ("発注図面改訂", "ordered drawing revision"),
        _ => return pred.to_string(),
    };
    if ja { j.into() } else { e.into() }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestItem {
    pub part: String,
    pub predicate: String,
    pub purchase_order: Option<String>,
    pub po_line: Option<String>,
    pub lot: Option<String>,
    pub heat: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Reason {
    pub predicate: String,
    pub from: Option<String>,
    pub to: Option<String>,
    /// superseded | withdrawn | new_evidence
    pub cause: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestDoc {
    pub doc: String,
    pub name: String,
    pub sha256: String,
    pub items: Vec<RequestItem>,
    pub reasons: Vec<Reason>,
}

pub struct Draft<'a> {
    pub reference: &'a str,
    pub supplier: &'a str,
    pub date: &'a str,
    pub documents: &'a [RequestDoc],
    /// Supplier documents the affected decisions rely on that do NOT need to be resent.
    pub not_requested: usize,
}

/// Untrusted text (document names, supplier values) is reduced to one printable line.
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect()
}

fn item_line(i: &RequestItem, lang: Lang) -> String {
    let ja = lang == Lang::Ja;
    let mut parts = vec![if ja {
        format!("品番 {}", clean(&i.part))
    } else {
        format!("part {}", clean(&i.part))
    }];
    if let Some(p) = &i.purchase_order {
        let line = i
            .po_line
            .as_deref()
            .map(|l| format!("-{}", clean(l)))
            .unwrap_or_default();
        parts.push(if ja {
            format!("注文番号 {}{line}", clean(p))
        } else {
            format!("PO {}{line}", clean(p))
        });
    }
    if let Some(l) = &i.lot {
        parts.push(if ja {
            format!("ロット {}", clean(l))
        } else {
            format!("lot {}", clean(l))
        });
    }
    if let Some(h) = &i.heat {
        parts.push(if ja {
            format!("溶解番号 {}", clean(h))
        } else {
            format!("heat {}", clean(h))
        });
    }
    let sep = if ja { "／" } else { ", " };
    format!(
        "{}{}{}",
        parts.join(sep),
        if ja { "：" } else { ": " },
        label(&i.predicate, lang)
    )
}

fn reason_line(r: &Reason, lang: Lang) -> String {
    let v = |x: &Option<String>| x.as_deref().map(clean).unwrap_or_else(|| "—".into());
    match (lang, r.cause.as_str()) {
        (Lang::Ja, "withdrawn") => format!(
            "{}：当社審査で証拠として採用できなかったため（{}）",
            label(&r.predicate, lang),
            v(&r.from)
        ),
        (Lang::Ja, _) => format!("{}：{} → {} に変更", label(&r.predicate, lang), v(&r.from), v(&r.to)),
        (Lang::En, "withdrawn") => format!(
            "{}: could not be accepted as evidence in our review (was {})",
            label(&r.predicate, lang),
            v(&r.from)
        ),
        (Lang::En, _) => format!(
            "{}: changed from {} to {}",
            label(&r.predicate, lang),
            v(&r.from),
            v(&r.to)
        ),
    }
}

/// Renders the request letter. Deterministic for the same input.
pub fn render(d: &Draft<'_>, lang: Lang) -> String {
    let mut o = String::new();
    let sha = |s: &str| s.chars().take(12).collect::<String>();
    match lang {
        Lang::Ja => {
            o.push_str(&format!("{} 御中\n\n", clean(d.supplier)));
            o.push_str(&format!(
                "件名：品質証明書類の再提出のお願い（依頼番号 {}）\n日付：{}\n\n",
                d.reference, d.date
            ));
            o.push_str("平素より格別のお引き立てを賜り、厚く御礼申し上げます。\n");
            o.push_str("下記の要求事項の変更に伴い、以下の書類に限り再提出をお願い申し上げます。\n\n");
            for (n, doc) in d.documents.iter().enumerate() {
                o.push_str(&format!(
                    "{}. {}（SHA-256 {}…）\n",
                    n + 1,
                    clean(&doc.name),
                    sha(&doc.sha256)
                ));
                o.push_str("   再確認が必要な項目：\n");
                for i in &doc.items {
                    o.push_str(&format!("   ・{}\n", item_line(i, lang)));
                }
                o.push_str("   変更の内容：\n");
                for r in &doc.reasons {
                    o.push_str(&format!("   ・{}\n", reason_line(r, lang)));
                }
                o.push('\n');
            }
            if d.not_requested > 0 {
                o.push_str(&format!(
                    "上記以外の貴社書類（{} 件）は引き続き有効であり、再提出は不要です。\n",
                    d.not_requested
                ));
            }
            o.push_str("ご多用のところ誠に恐縮ですが、何卒よろしくお願い申し上げます。\n\n敬具\n");
        }
        Lang::En => {
            o.push_str(&format!("To: {}\n\n", clean(d.supplier)));
            o.push_str(&format!(
                "Subject: Request to resubmit quality documents (reference {})\nDate: {}\n\n",
                d.reference, d.date
            ));
            o.push_str("Dear Sir or Madam,\n\nFollowing the requirement changes listed below, please resubmit only these documents:\n\n");
            for (n, doc) in d.documents.iter().enumerate() {
                o.push_str(&format!(
                    "{}. {} (SHA-256 {}…)\n",
                    n + 1,
                    clean(&doc.name),
                    sha(&doc.sha256)
                ));
                o.push_str("   Items to re-confirm:\n");
                for i in &doc.items {
                    o.push_str(&format!("   - {}\n", item_line(i, lang)));
                }
                o.push_str("   What changed:\n");
                for r in &doc.reasons {
                    o.push_str(&format!("   - {}\n", reason_line(r, lang)));
                }
                o.push('\n');
            }
            if d.not_requested > 0 {
                o.push_str(&format!(
                    "Your other documents on file ({}) remain valid and need not be resent.\n",
                    d.not_requested
                ));
            }
            o.push_str("Thank you for your cooperation.\n\nKind regards\n");
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_dependent_supplier_statements_are_stale() {
        assert!(needs_resupply("mfg.tolerance.length.plus", "qa.measured.length"));
        assert!(needs_resupply(
            "mfg.tolerance.outer_diameter.minus",
            "qa.measured.outer_diameter"
        ));
        assert!(!needs_resupply(
            "mfg.tolerance.length.plus",
            "qa.measured.outer_diameter"
        ));
        assert!(!needs_resupply("mfg.tolerance.length.plus", "qa.material"));
        assert!(needs_resupply("mfg.dimension.length", "qa.measured.length"));
        assert!(needs_resupply("mfg.material", "qa.material"));
        assert!(needs_resupply("po.material", "qa.material"));
        assert!(needs_resupply(
            "mfg.property.tensile_strength.min",
            "qa.property.tensile_strength"
        ));
        assert!(needs_resupply("po.certificate_type", "qa.certificate_type"));
        assert!(needs_resupply("mfg.revision", "qa.measured.width"));
        assert!(!needs_resupply("mfg.revision", "qa.material"));
        assert!(needs_resupply(
            "qa.property.tensile_strength",
            "qa.property.tensile_strength"
        ));
        assert!(!needs_resupply("qa.property.tensile_strength", "qa.material"));
        for internal in [
            "mfg.approval.status",
            "mfg.certificate.expiry",
            "po.quantity",
            "po.supplier",
        ] {
            assert!(!needs_resupply(internal, "qa.measured.length"), "{internal}");
            assert!(!needs_resupply(internal, "qa.material"), "{internal}");
        }
        assert!(
            !needs_resupply("mfg.material", "mfg.material"),
            "only supplier statements"
        );
    }

    fn doc() -> RequestDoc {
        RequestDoc {
            doc: "a1".into(),
            name: "inspection\u{7}_MX-1.pdf".into(),
            sha256: "0123456789abcdef".into(),
            items: vec![RequestItem {
                part: "MX-1".into(),
                predicate: "qa.measured.length".into(),
                purchase_order: Some("PO-9".into()),
                po_line: Some("1".into()),
                lot: Some("L1".into()),
                heat: None,
            }],
            reasons: vec![Reason {
                predicate: "mfg.tolerance.length.plus".into(),
                from: Some("0.2 mm".into()),
                to: Some("0.1 mm".into()),
                cause: "superseded".into(),
            }],
        }
    }

    #[test]
    fn bilingual_letters_list_only_requested_documents() {
        let docs = [doc()];
        let d = Draft {
            reference: "RR-1",
            supplier: "名古屋鍛造\n株式会社",
            date: "2026-11-05",
            documents: &docs,
            not_requested: 3,
        };
        let ja = render(&d, Lang::Ja);
        assert!(ja.starts_with("名古屋鍛造 株式会社 御中"), "{ja}");
        assert!(ja.contains("測定値（長さ）"));
        assert!(ja.contains("上の許容差（長さ）：0.2 mm → 0.1 mm に変更"));
        assert!(ja.contains("ロット L1"));
        assert!(ja.contains("（3 件）"));
        assert!(!ja.contains('\u{7}'), "control characters removed");
        let en = render(&d, Lang::En);
        assert!(en.contains("measured length"));
        assert!(en.contains("upper tolerance: length: changed from 0.2 mm to 0.1 mm"));
        assert!(en.contains("SHA-256 0123456789ab…"));
        assert_eq!(en, render(&d, Lang::En));
    }

    #[test]
    fn labels_cover_the_vocabulary() {
        assert_eq!(label("qa.property.tensile_strength", Lang::Ja), "材料特性（引張強さ）");
        assert_eq!(
            label("mfg.property.tensile_strength.min", Lang::En),
            "specified minimum tensile strength"
        );
        assert_eq!(
            label("qa.certificate_type", Lang::En),
            "inspection document type (EN 10204)"
        );
        assert_eq!(label("x.unknown", Lang::Ja), "x.unknown");
    }
}

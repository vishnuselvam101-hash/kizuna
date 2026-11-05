//! Record context for the one-character (typo) review rule: what else the evidence says about a part key
//! (description / part name, supplier, drawing number, material).
//!
//! A one-keyed-character difference between two part numbers looks the same to the model whether it is a typo or
//! a different part. On a dense number space most such pairs are different parts (one-digit siblings), so sending
//! every one to review floods the queue. With context, the rule can ask for extra agreement first: a typo of a part
//! usually travels with that part's supplier, drawing and material; a sibling part usually has its own drawing and
//! a different description. The context never makes anything *match* — it only decides whether a typo-shaped pair
//! is forced into review or left to the ordinary Fellegi–Sunter outcome.

use super::normalize::{company_key, part_key, part_loose, split_revision};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// How the one-character rule uses context (per tenant; default `Strict`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypoReview {
    /// No typo rule: one-character pairs get the ordinary FS outcome (most are dropped as non-matches).
    Off,
    /// Typo-shaped pairs go to review when their context agrees (or when there is no comparable context).
    #[default]
    Strict,
    /// Every typo-shaped pair goes to review (the v2 behaviour before tuning).
    Broad,
}

impl TypoReview {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Self::Off),
            "strict" => Some(Self::Strict),
            "broad" => Some(Self::Broad),
            _ => None,
        }
    }
    /// Sensitivity order (off < strict < broad): a lower value is a weakening.
    pub fn rank(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Strict => 1,
            Self::Broad => 2,
        }
    }
}

/// Raw context values of one key, as found in the evidence (any number of each).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartContext {
    pub descriptions: Vec<String>,
    pub suppliers: Vec<String>,
    pub drawings: Vec<String>,
    pub materials: Vec<String>,
}

/// Normalised context: description tokens and normalised supplier/drawing/material values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prepared {
    desc: BTreeSet<String>,
    suppliers: BTreeSet<String>,
    drawings: BTreeSet<String>,
    materials: BTreeSet<String>,
}

fn fold_width(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3000}' => ' ',
            _ => c,
        })
        .collect::<String>()
        .to_uppercase()
}

/// Description tokens: ASCII alphanumeric words, and character bigrams of non-ASCII letter runs (Japanese has no
/// spaces; a run of one character is its own token).
pub fn description_tokens(s: &str) -> BTreeSet<String> {
    let s = fold_width(s);
    let mut out = BTreeSet::new();
    let mut run: Vec<char> = Vec::new();
    let mut ascii = false;
    let mut flush = |run: &mut Vec<char>, ascii: bool| {
        if run.is_empty() {
            return;
        }
        if ascii || run.len() == 1 {
            out.insert(run.iter().collect::<String>());
        } else {
            for w in run.windows(2) {
                out.insert(w.iter().collect::<String>());
            }
        }
        run.clear();
    };
    for c in s.chars() {
        let class = if c.is_ascii_alphanumeric() {
            Some(true)
        } else if c.is_alphanumeric() || c == 'ー' {
            Some(false)
        } else {
            None
        };
        match class {
            Some(a) if run.is_empty() || a == ascii => {
                ascii = a;
                run.push(c);
            }
            Some(a) => {
                flush(&mut run, ascii);
                ascii = a;
                run.push(c);
            }
            None => flush(&mut run, ascii),
        }
    }
    flush(&mut run, ascii);
    out
}

fn identifier(s: &str) -> String {
    let (base, _) = split_revision(&part_key(s));
    part_loose(&base)
}

pub fn prepare(c: &PartContext) -> Prepared {
    let set = |v: &[String], f: &dyn Fn(&str) -> String| -> BTreeSet<String> {
        v.iter().map(|s| f(s)).filter(|s| !s.is_empty()).collect()
    };
    Prepared {
        desc: c.descriptions.iter().flat_map(|d| description_tokens(d)).collect(),
        suppliers: set(&c.suppliers, &|s| company_key(s).core),
        drawings: set(&c.drawings, &identifier),
        materials: set(&c.materials, &|s| part_loose(&part_key(s))),
    }
}

impl Prepared {
    pub fn is_empty(&self) -> bool {
        self.desc.is_empty() && self.suppliers.is_empty() && self.drawings.is_empty() && self.materials.is_empty()
    }
}

/// How two keys' contexts agree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Agreement {
    /// Fields present on both sides (0–4).
    pub shared_fields: usize,
    /// Mean per-field similarity over the shared fields (description: token Jaccard; others: any value in common).
    pub score: f64,
    pub same_supplier_and_drawing: bool,
}

pub fn agreement(a: &Prepared, b: &Prepared) -> Agreement {
    let mut sims = Vec::with_capacity(4);
    if !a.desc.is_empty() && !b.desc.is_empty() {
        let inter = a.desc.intersection(&b.desc).count();
        sims.push(inter as f64 / (a.desc.len() + b.desc.len() - inter) as f64);
    }
    let any = |x: &BTreeSet<String>, y: &BTreeSet<String>| -> Option<bool> {
        (!x.is_empty() && !y.is_empty()).then(|| !x.is_disjoint(y))
    };
    let supplier = any(&a.suppliers, &b.suppliers);
    let drawing = any(&a.drawings, &b.drawings);
    for f in [supplier, drawing, any(&a.materials, &b.materials)]
        .into_iter()
        .flatten()
    {
        sims.push(if f { 1.0 } else { 0.0 });
    }
    Agreement {
        shared_fields: sims.len(),
        score: if sims.is_empty() {
            0.0
        } else {
            sims.iter().sum::<f64>() / sims.len() as f64
        },
        same_supplier_and_drawing: supplier == Some(true) && drawing == Some(true),
    }
}

/// Strict-mode decision for a typo-shaped pair: `Some(reason)` sends it to review, `None` leaves it to FS.
/// No comparable context (nothing shared) is treated conservatively: review.
pub fn strict_review_reason(a: &Prepared, b: &Prepared, floor: f64) -> Option<&'static str> {
    let g = agreement(a, b);
    if g.shared_fields == 0 {
        Some("no_comparable_context")
    } else if g.same_supplier_and_drawing {
        Some("same_supplier_and_drawing")
    } else if g.score >= floor {
        Some("context_agrees")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(d: &[&str], s: &[&str], g: &[&str], m: &[&str]) -> Prepared {
        let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect();
        prepare(&PartContext {
            descriptions: v(d),
            suppliers: v(s),
            drawings: v(g),
            materials: v(m),
        })
    }

    #[test]
    fn tokens_split_ascii_words_and_bigram_japanese() {
        let t = description_tokens("ＢＲＡＣＫＥＴ L50 ブラケット");
        assert!(t.contains("BRACKET") && t.contains("L50") && t.contains("ブラ") && t.contains("ケッ"));
        assert_eq!(description_tokens("軸"), ["軸".to_string()].into_iter().collect());
        assert!(description_tokens(" - ").is_empty());
    }

    #[test]
    fn supplier_drawing_material_are_normalised() {
        let a = ctx(&[], &["株式会社東海精機"], &["DWG-1023 REV B"], &["SUS-304"]);
        let b = ctx(&[], &["東海精機㈱"], &["dwg1023"], &["sus304"]);
        let g = agreement(&a, &b);
        assert_eq!((g.shared_fields, g.score, g.same_supplier_and_drawing), (3, 1.0, true));
    }

    #[test]
    fn strict_rule() {
        let master = ctx(&["BRACKET L50"], &["東海精機"], &["D-100"], &["SS400"]);
        // A typo from a PO of the same part: same supplier and drawing.
        let typo = ctx(&[], &["東海精機"], &["D-100"], &[]);
        assert_eq!(
            strict_review_reason(&master, &typo, 0.6),
            Some("same_supplier_and_drawing")
        );
        // A sibling part: same family and supplier, own drawing and size.
        let sibling = ctx(&["BRACKET L60"], &["東海精機"], &["D-101"], &["SS400"]);
        let g = agreement(&master, &sibling);
        assert!((g.score - (1.0 / 3.0 + 1.0 + 0.0 + 1.0) / 4.0).abs() < 1e-12, "{g:?}");
        assert_eq!(strict_review_reason(&master, &sibling, 0.5), Some("context_agrees"));
        assert_eq!(strict_review_reason(&master, &sibling, 0.6), None);
        // No context on one side: conservative.
        assert_eq!(
            strict_review_reason(&master, &Prepared::default(), 0.9),
            Some("no_comparable_context")
        );
        // Only non-overlapping fields: nothing comparable either.
        let only_desc = ctx(&["BRACKET"], &[], &[], &[]);
        let only_sup = ctx(&[], &["X"], &[], &[]);
        assert_eq!(
            strict_review_reason(&only_desc, &only_sup, 0.9),
            Some("no_comparable_context")
        );
    }

    #[test]
    fn agreement_is_symmetric() {
        let a = ctx(&["シャフト φ12"], &["A"], &["D1"], &["S45C"]);
        let b = ctx(&["SHAFT φ12"], &["A"], &["D2"], &[]);
        assert_eq!(agreement(&a, &b), agreement(&b, &a));
    }

    #[test]
    fn modes_parse_and_rank() {
        assert_eq!(TypoReview::default(), TypoReview::Strict);
        assert_eq!(TypoReview::parse("broad"), Some(TypoReview::Broad));
        assert_eq!(TypoReview::parse("loose"), None);
        assert!(
            TypoReview::Off.rank() < TypoReview::Strict.rank() && TypoReview::Strict.rank() < TypoReview::Broad.rank()
        );
    }
}

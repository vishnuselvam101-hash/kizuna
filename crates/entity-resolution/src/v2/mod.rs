//! Entity resolution v2: blocking (sorted neighbourhood + normalised prefix + q-gram index), field comparators for
//! part numbers and Japanese company names, Fellegi–Sunter weights estimated by EM on the unlabelled pairs, and
//! thresholds for match / possible match / no match / requires review.
//!
//! Nothing here merges anything. `Match` means "strong evidence, confirm"; only `safe` pairs (identical after
//! identity-preserving normalisation) may be treated as the same key without a human, exactly as in v1.

pub mod active;
pub mod block;
pub mod context;
pub mod em;
pub mod normalize;

use crate::Outcome;
use block::{BlockConfig, RecordKeys, qgrams};
pub use context::{PartContext, TypoReview};
use em::{EmReport, FsParams, Gamma};
use normalize::{CompanyKey, LegalForm, company_key, ocr_fold, part_key, part_loose, split_revision};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

/// splitmix64: tiny deterministic generator (no external dependency) for sampling training pairs.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Part,
    Company,
}

impl Kind {
    /// Subject types of the API: parts are identifiers, suppliers are company names.
    pub fn for_subject_type(t: &str) -> Kind {
        if t == "supplier" { Kind::Company } else { Kind::Part }
    }
    /// Prior parameters: EM starting point and the fallback when EM cannot be trusted.
    pub fn default_params(self) -> FsParams {
        match self {
            Kind::Part => FsParams {
                p: 0.05,
                m: vec![
                    vec![0.30, 0.25, 0.10, 0.15, 0.10, 0.05, 0.05],
                    vec![0.9, 0.1],
                    vec![0.95, 0.05],
                    vec![0.8, 0.15, 0.05],
                ],
                u: vec![
                    vec![0.0005, 0.0005, 0.0005, 0.001, 0.01, 0.05, 0.9375],
                    vec![0.05, 0.95],
                    vec![0.2, 0.8],
                    vec![0.85, 0.1, 0.05],
                ],
            },
            Kind::Company => FsParams {
                p: 0.05,
                m: vec![vec![0.6, 0.1, 0.05, 0.25], vec![0.85, 0.1, 0.05], vec![0.4, 0.55, 0.05]],
                u: vec![
                    vec![0.001, 0.01, 0.05, 0.939],
                    vec![0.002, 0.02, 0.978],
                    vec![0.3, 0.6, 0.1],
                ],
            },
        }
    }
}

/// A key prepared once for all comparisons.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub raw: String,
    /// Identity-preserving normal form (equal ⇒ `safe`).
    pub safe: String,
    // Part fields.
    loose: String,
    core: String,
    ocr: String,
    digits: String,
    alpha: String,
    rev: Option<String>,
    // Company fields.
    company: Option<CompanyKey>,
}

/// Identity-preserving normal form of a company name: width/script/invisible folding and whitespace only.
fn company_safe(s: &str) -> String {
    let k: String = s
        .chars()
        .filter(|c| !matches!(c, '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}'))
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3000}' => ' ',
            _ => c,
        })
        .collect();
    k.split_whitespace().collect::<Vec<_>>().join(" ").to_uppercase()
}

pub fn prepare(kind: Kind, raw: &str) -> Prepared {
    match kind {
        Kind::Part => {
            let safe = part_key(raw);
            let (base, rev) = split_revision(&safe);
            let core = part_loose(&base);
            let ocr = ocr_fold(&core);
            Prepared {
                raw: raw.to_string(),
                loose: part_loose(&safe),
                digits: ocr.chars().filter(char::is_ascii_digit).collect(),
                alpha: core.chars().take_while(|c| c.is_alphabetic()).collect(),
                ocr,
                core,
                rev,
                safe,
                company: None,
            }
        }
        Kind::Company => Prepared {
            raw: raw.to_string(),
            safe: company_safe(raw),
            loose: String::new(),
            core: String::new(),
            ocr: String::new(),
            digits: String::new(),
            alpha: String::new(),
            rev: None,
            company: Some(company_key(raw)),
        },
    }
}

fn block_keys(kind: Kind, p: &Prepared) -> RecordKeys {
    match (kind, &p.company) {
        (Kind::Company, Some(c)) => {
            let mut k = RecordKeys {
                sort_keys: vec![c.core.clone()],
                prefixes: vec![format!("c:{}", c.core.chars().take(2).collect::<String>())],
                qgrams: qgrams(&c.core).into_iter().map(|q| format!("c:{q}")).collect(),
                exact: vec![],
            };
            if let Some(r) = c.romaji.as_ref().filter(|r| !r.is_empty()) {
                k.sort_keys.push(r.clone());
                k.prefixes.push(format!("r:{}", r.chars().take(3).collect::<String>()));
                k.qgrams.extend(qgrams(r).into_iter().map(|q| format!("r:{q}")));
            }
            k
        }
        _ => {
            let c: Vec<char> = p.core.chars().collect();
            let mut exact = vec![format!("={}", p.core)];
            if c.len() >= 5 {
                exact.extend((0..c.len()).map(|i| {
                    let v: String = c.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, x)| x).collect();
                    format!("={v}")
                }));
            }
            exact.sort();
            exact.dedup();
            RecordKeys {
                sort_keys: vec![p.ocr.clone()],
                prefixes: vec![p.ocr.chars().take(4).collect()],
                qgrams: qgrams(&p.ocr),
                exact,
            }
        }
    }
}

pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1)
                .min(cur[j - 1] + 1)
                .min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
        prev = cur;
    }
    prev[b.len()]
}

fn dice(a: &str, b: &str) -> f64 {
    let (qa, qb) = (qgrams(a), qgrams(b));
    let common = qa.iter().filter(|q| qb.binary_search(q).is_ok()).count();
    2.0 * common as f64 / (qa.len() + qb.len()).max(1) as f64
}

/// Jaro–Winkler similarity on characters (symmetric).
pub fn jaro_winkler(a: &str, b: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let range = (a.len().max(b.len()) / 2).saturating_sub(1);
    let (mut am, mut bm) = (vec![false; a.len()], vec![false; b.len()]);
    let mut m = 0usize;
    for i in 0..a.len() {
        for j in i.saturating_sub(range)..(i + range + 1).min(b.len()) {
            if !bm[j] && a[i] == b[j] {
                am[i] = true;
                bm[j] = true;
                m += 1;
                break;
            }
        }
    }
    if m == 0 {
        return 0.0;
    }
    let ma: Vec<char> = a.iter().zip(&am).filter(|(_, f)| **f).map(|(c, _)| *c).collect();
    let mb: Vec<char> = b.iter().zip(&bm).filter(|(_, f)| **f).map(|(c, _)| *c).collect();
    let t = ma.iter().zip(&mb).filter(|(x, y)| x != y).count() as f64 / 2.0;
    let m = m as f64;
    let jaro = (m / a.len() as f64 + m / b.len() as f64 + (m - t) / m) / 3.0;
    let prefix = a.iter().zip(&b).take(4).take_while(|(x, y)| x == y).count() as f64;
    jaro + prefix * 0.1 * (1.0 - jaro)
}

/// True if the strings differ by exactly one adjacent transposition.
fn is_transposition(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len() != b.len() {
        return false;
    }
    let d: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
    d.len() == 2 && d[1] == d[0] + 1 && a[d[0]] == b[d[1]] && a[d[1]] == b[d[0]]
}

/// One keyed character apart (edit distance 1 or one transposition) on a number of ≥ 5 characters, not explained
/// by separators, OCR confusion or a revision suffix, with the alpha prefix agreeing and no revision conflict.
fn possible_typo(a: &Prepared, b: &Prepared, g: &Gamma) -> bool {
    g[0].is_some_and(|l| l >= 4)
        && g[2] != Some(1)
        && g[3] != Some(2)
        && a.core.chars().count().min(b.core.chars().count()) >= 5
        && (edit_distance(&a.core, &b.core) == 1 || is_transposition(&a.core, &b.core))
}

/// Symmetric Jaro–Winkler (the textbook version is asymmetric in rare transposition cases).
fn jw(a: &str, b: &str) -> f64 {
    jaro_winkler(a, b).max(jaro_winkler(b, a))
}

/// Comparison vector of two prepared keys. Symmetric: `gamma(a, b) == gamma(b, a)`.
pub fn gamma(kind: Kind, a: &Prepared, b: &Prepared) -> Gamma {
    match (kind, &a.company, &b.company) {
        (Kind::Company, Some(x), Some(y)) => {
            // Two views of the name: the folded written form and the romaji reading (kana/Latin names only).
            let core_sim = jw(&x.core, &y.core);
            let core = if !x.core.is_empty() && x.core == y.core {
                0
            } else if core_sim >= 0.92 {
                1
            } else if core_sim >= 0.8 {
                2
            } else {
                3
            };
            let romaji = match (&x.romaji, &y.romaji) {
                (Some(r), Some(s)) if !r.is_empty() && !s.is_empty() => Some(if r == s {
                    0
                } else if jw(r, s) >= 0.92 {
                    1
                } else {
                    2
                }),
                _ => None,
            };
            let legal = match (x.legal, y.legal) {
                (Some(l), Some(r)) if l == r => 0,
                (Some(_), Some(_)) => 2,
                _ => 1,
            };
            vec![Some(core), romaji, Some(legal)]
        }
        _ => {
            let id = if a.safe == b.safe {
                0
            } else if a.loose == b.loose {
                1
            } else if a.core == b.core {
                3
            } else if a.ocr == b.ocr {
                2
            } else if edit_distance(&a.core, &b.core) == 1 && a.core.len().min(b.core.len()) >= 5 {
                4
            } else if dice(&a.core, &b.core) >= 0.7 {
                5
            } else {
                6
            };
            let digits = (!a.digits.is_empty() || !b.digits.is_empty()).then_some(u8::from(a.digits != b.digits));
            let alpha = (!a.alpha.is_empty() || !b.alpha.is_empty()).then_some(u8::from(a.alpha != b.alpha));
            let rev = match (&a.rev, &b.rev) {
                (None, None) => 0,
                (Some(x), Some(y)) if x == y => 0,
                (Some(_), Some(_)) => 2,
                _ => 1,
            };
            vec![Some(id), digits, alpha, Some(rev)]
        }
    }
}

fn reasons(kind: Kind, g: &Gamma) -> Vec<&'static str> {
    let mut r = Vec::new();
    match kind {
        Kind::Part => {
            r.push(match g[0] {
                Some(0) => "safe_normalisation",
                Some(1) => "separator_difference",
                Some(2) => "ocr_confusable_characters",
                Some(3) => "revision_suffix_in_identifier",
                Some(4) => "one_character_difference",
                Some(5) => "similar_identifier",
                _ => "weak_similarity",
            });
            if g[3] == Some(2) {
                r.push("revision_differs");
            }
        }
        Kind::Company => {
            r.push(match (g[0], g[1]) {
                (Some(0), _) => "same_name_after_folding",
                (_, Some(0)) => "same_romaji_reading",
                (Some(1), _) | (_, Some(1)) => "very_similar_name",
                (Some(2), _) => "similar_name",
                _ => "weak_similarity",
            });
            if g[2] == Some(2) {
                r.push("legal_form_differs");
            }
        }
    }
    r
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub seed: u64,
    pub block: BlockConfig,
    /// Fellegi–Sunter target error rates for the thresholds (expected false-match share above the match
    /// threshold; expected share of matches below the possible-match threshold).
    pub mu: f64,
    pub lambda: f64,
    pub max_iter: usize,
    pub tol: f64,
    pub min_training_pairs: u64,
    /// Cardinality bound for EM: at most this many true duplicate pairs per record on average (p ≤ k·n/|blocked|).
    pub max_pairs_per_record: f64,
    /// Seeded random (non-blocked) pairs per record added to the EM training set.
    pub random_pairs_per_record: usize,
    /// One-character (typo) rule: off / strict (needs context agreement) / broad (always review).
    pub typo_review: TypoReview,
    /// Strict mode: minimum mean context agreement (description/supplier/drawing/material) for review.
    pub context_floor: f64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            seed: 0x4B49_5A55_4E41_0002,
            block: BlockConfig::default(),
            mu: 0.001,
            lambda: 0.01,
            max_iter: 500,
            tol: 1e-9,
            min_training_pairs: 50,
            random_pairs_per_record: 4,
            max_pairs_per_record: 2.0,
            typo_review: TypoReview::Strict,
            context_floor: 0.6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    pub left: String,
    pub right: String,
    pub outcome: Outcome,
    pub reasons: Vec<&'static str>,
    /// Fellegi–Sunter match weight (log2 likelihood ratio).
    pub weight: f64,
    /// Identical after identity-preserving normalisation: may be treated as one key without a decision.
    pub safe: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stats {
    pub kind: Kind,
    pub records: usize,
    pub pairs_compared: usize,
    pub em: EmReport,
    pub params: FsParams,
    pub t_match: f64,
    pub t_possible: f64,
    pub typo_review: TypoReview,
    /// Pairs sent to review by the one-character rule (whatever their weight).
    pub typo_reviews: usize,
    /// Typo-shaped pairs the strict rule left to FS because their context disagrees.
    pub typo_left_to_fs: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Resolution {
    pub candidates: Vec<Candidate>,
    pub stats: Stats,
}

/// Resolves distinct keys without context. Deterministic for a given (keys as a set, config).
pub fn resolve(kind: Kind, keys: &[String], cfg: &Config) -> Resolution {
    resolve_with_context(kind, keys, &HashMap::new(), cfg)
}

/// Resolves distinct keys; `context` (by raw key, parts only) feeds the strict typo rule. Deterministic for a given
/// (keys as a set, context, config).
pub fn resolve_with_context(
    kind: Kind,
    keys: &[String],
    context: &HashMap<String, PartContext>,
    cfg: &Config,
) -> Resolution {
    let mut ks: Vec<&String> = keys.iter().collect();
    ks.sort();
    ks.dedup();
    let prep: Vec<Prepared> = ks.iter().map(|k| prepare(kind, k)).collect();
    let ctx: Vec<context::Prepared> = if kind == Kind::Part && cfg.typo_review == TypoReview::Strict {
        ks.iter()
            .map(|k| context.get(k.as_str()).map(context::prepare).unwrap_or_default())
            .collect()
    } else {
        Vec::new()
    };
    let bk: Vec<RecordKeys> = prep.iter().map(|p| block_keys(kind, p)).collect();
    let pairs = block::candidate_pairs(&bk, &cfg.block);
    let gammas: Vec<Gamma> = pairs
        .iter()
        .map(|&(i, j)| gamma(kind, &prep[i as usize], &prep[j as usize]))
        .collect();

    // EM training set: blocked pairs plus seeded random pairs (u must see typical non-matches, not only the
    // near-misses blocking produces). The match share is capped by a cardinality bound.
    let mut patterns: BTreeMap<Gamma, u64> = BTreeMap::new();
    for g in &gammas {
        *patterns.entry(g.clone()).or_insert(0) += 1;
    }
    let n = prep.len() as u64;
    if n >= 2 {
        let mut rng = Rng::new(cfg.seed);
        let want = (cfg.random_pairs_per_record as u64 * n).min(n * (n - 1) / 2);
        for _ in 0..want {
            let (i, j) = (rng.below(n) as usize, rng.below(n) as usize);
            if i != j {
                *patterns.entry(gamma(kind, &prep[i], &prep[j])).or_insert(0) += 1;
            }
        }
    }
    let training: u64 = patterns.values().sum();
    let (params, em) = em::estimate(
        &patterns,
        &kind.default_params(),
        cfg.max_iter,
        cfg.tol,
        cfg.min_training_pairs,
        None,
        (cfg.max_pairs_per_record * n as f64 / training.max(1) as f64).min(0.95),
    );
    let (t_match, t_possible) = em::thresholds(&params, cfg.mu, cfg.lambda);

    let mut out = Vec::new();
    let mut typo_reviews = 0usize;
    let mut typo_left_to_fs = 0usize;
    for (&(i, j), g) in pairs.iter().zip(&gammas) {
        let (a, b) = (&prep[i as usize], &prep[j as usize]);
        let w = em::weight(&params, g);
        let safe = a.safe == b.safe;
        let shaped = kind == Kind::Part && !safe && cfg.typo_review != TypoReview::Off && possible_typo(a, b, g);
        let why = match cfg.typo_review {
            TypoReview::Strict if shaped => {
                context::strict_review_reason(&ctx[i as usize], &ctx[j as usize], cfg.context_floor)
            }
            _ => None,
        };
        let typo = shaped && (cfg.typo_review == TypoReview::Broad || why.is_some());
        typo_left_to_fs += usize::from(shaped && !typo);
        let outcome = if typo {
            // One keyed character apart: a typo and a different part look identical to the model, so neither
            // `match` nor a silent drop is acceptable; a human decides.
            Outcome::RequiresHumanReview
        } else if safe || w >= t_match {
            Outcome::Match
        } else if w >= t_possible {
            Outcome::PossibleMatch
        } else {
            continue;
        };
        let mut r = reasons(kind, g);
        if typo {
            r = vec!["possible_typo_or_different_part"];
            r.extend(why);
            typo_reviews += 1;
        }
        if safe {
            r = vec![if a.raw == b.raw {
                "identical"
            } else {
                "safe_normalisation"
            }];
        }
        out.push(Candidate {
            left: a.raw.clone(),
            right: b.raw.clone(),
            outcome,
            reasons: r,
            weight: w,
            safe,
        });
    }
    // Review rules: conflicting identity evidence, or a key linked to two or more others (ambiguous cluster).
    let mut degree: HashMap<&str, usize> = HashMap::new();
    for c in out.iter().filter(|c| !c.safe) {
        *degree.entry(c.left.as_str()).or_default() += 1;
        *degree.entry(c.right.as_str()).or_default() += 1;
    }
    let ambiguous: HashSet<String> = degree
        .into_iter()
        .filter(|(_, d)| *d >= 2)
        .map(|(k, _)| k.to_string())
        .collect();
    for c in out.iter_mut().filter(|c| !c.safe) {
        let conflict = c.reasons.contains(&"revision_differs") || c.reasons.contains(&"legal_form_differs");
        if conflict || ambiguous.contains(&c.left) || ambiguous.contains(&c.right) {
            c.outcome = Outcome::RequiresHumanReview;
            if !conflict {
                c.reasons.push("ambiguous_cluster");
            }
        }
    }
    Resolution {
        candidates: out,
        stats: Stats {
            kind,
            records: prep.len(),
            pairs_compared: pairs.len(),
            em,
            params,
            t_match,
            t_possible,
            typo_review: cfg.typo_review,
            typo_reviews,
            typo_left_to_fs,
        },
    }
}

/// Record pairs blocking produces (raw keys, each pair sorted), before any scoring. For blocking evaluation:
/// a true pair missing here can never be surfaced, whatever the weights.
pub fn blocked_pairs(kind: Kind, keys: &[String], cfg: &Config) -> Vec<(String, String)> {
    let mut ks: Vec<&String> = keys.iter().collect();
    ks.sort();
    ks.dedup();
    let prep: Vec<Prepared> = ks.iter().map(|k| prepare(kind, k)).collect();
    let bk: Vec<RecordKeys> = prep.iter().map(|p| block_keys(kind, p)).collect();
    block::candidate_pairs(&bk, &cfg.block)
        .into_iter()
        .map(|(i, j)| (ks[i as usize].clone(), ks[j as usize].clone()))
        .collect()
}

/// Legal form of a company name (for display and tests).
pub fn legal_form(s: &str) -> Option<LegalForm> {
    company_key(s).legal
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn keys(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn outcome_of(r: &Resolution, a: &str, b: &str) -> Option<Outcome> {
        r.candidates
            .iter()
            .find(|c| (c.left == a && c.right == b) || (c.left == b && c.right == a))
            .map(|c| c.outcome)
    }

    #[test]
    fn small_part_sets_use_prior_parameters_and_flag_variants() {
        let r = resolve(
            Kind::Part,
            &keys(&[
                "BRK-1000",
                "ＢＲＫ－１０００",
                "BRK1000",
                "SHF-2000",
                "SHF-2O00",
                "PIN-7777",
            ]),
            &Config::default(),
        );
        assert_eq!(r.stats.em.used, "default");
        let safe = r.candidates.iter().find(|c| c.safe).expect("safe pair");
        assert_eq!(safe.outcome, Outcome::Match);
        assert!(outcome_of(&r, "BRK-1000", "BRK1000").is_some());
        assert!(
            outcome_of(&r, "SHF-2000", "SHF-2O00").is_some(),
            "OCR confusable: {r:?}"
        );
        assert_eq!(outcome_of(&r, "PIN-7777", "BRK-1000"), None);
    }

    #[test]
    fn revision_conflict_requires_review() {
        let r = resolve(
            Kind::Part,
            &keys(&["GER-4410 REV B", "GER-4410 REV C"]),
            &Config::default(),
        );
        assert_eq!(
            outcome_of(&r, "GER-4410 REV B", "GER-4410 REV C"),
            Some(Outcome::RequiresHumanReview)
        );
    }

    #[test]
    fn company_variants_are_found() {
        let r = resolve(
            Kind::Company,
            &keys(&[
                "株式会社東海精機",
                "東海精機㈱",
                "カントウセイミツ",
                "Kanto Seimitsu Co., Ltd.",
                "大阪金属工業",
            ]),
            &Config::default(),
        );
        assert!(outcome_of(&r, "株式会社東海精機", "東海精機㈱").is_some(), "{r:?}");
        assert!(
            outcome_of(&r, "カントウセイミツ", "Kanto Seimitsu Co., Ltd.").is_some(),
            "{r:?}"
        );
        assert_eq!(outcome_of(&r, "大阪金属工業", "東海精機㈱"), None);
    }

    #[test]
    fn nothing_is_merged_only_safe_pairs_skip_review() {
        let r = resolve(
            Kind::Part,
            &keys(&["A-123456", "a-123456", "A123456"]),
            &Config::default(),
        );
        for c in &r.candidates {
            assert_eq!(c.safe, part_key(&c.left) == part_key(&c.right), "{c:?}");
        }
    }

    #[test]
    fn one_character_differences_go_to_review_never_match_or_drop() {
        let r = resolve(
            Kind::Part,
            &keys(&["BRK-10234", "BRK-10235", "BRK-12034", "SHF-77810", "SHF-778100"]),
            &Config::default(),
        );
        for (a, b) in [
            ("BRK-10234", "BRK-10235"),
            ("BRK-10234", "BRK-12034"),
            ("SHF-77810", "SHF-778100"),
        ] {
            assert_eq!(
                outcome_of(&r, a, b),
                Some(Outcome::RequiresHumanReview),
                "{a} {b}: {r:?}"
            );
        }
        assert!(r.stats.typo_reviews >= 3);
        assert!(is_transposition("10234", "10324") && !is_transposition("10234", "13204"));
    }

    #[test]
    fn typo_rule_modes_use_context_in_strict_mode() {
        let k = keys(&["BRK-10234", "BRK-10235", "SHF-77810", "SHF-77811"]);
        let c = |d: &str, s: &str, g: &str| PartContext {
            descriptions: vec![d.into()],
            suppliers: vec![s.into()],
            drawings: vec![g.into()],
            materials: vec![],
        };
        let ctx: HashMap<String, PartContext> = [
            // A sibling part: other size, other drawing, other supplier.
            ("BRK-10234", c("BRACKET L50", "東海精機", "D-1")),
            ("BRK-10235", c("BRACKET L80", "大阪金属", "D-2")),
            // A typo of the same part: same supplier and drawing.
            ("SHF-77810", c("SHAFT 12", "東海精機", "D-9")),
            ("SHF-77811", c("シャフト", "東海精機", "D-9")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let run = |m: TypoReview| {
            resolve_with_context(
                Kind::Part,
                &k,
                &ctx,
                &Config {
                    typo_review: m,
                    ..Config::default()
                },
            )
        };
        let strict = run(TypoReview::Strict);
        let typo_pair = |r: &Resolution, a: &str, b: &str| {
            r.candidates.iter().any(|c| {
                ((c.left == a && c.right == b) || (c.left == b && c.right == a))
                    && c.outcome == Outcome::RequiresHumanReview
                    && c.reasons[0] == "possible_typo_or_different_part"
            })
        };
        assert!(typo_pair(&strict, "SHF-77810", "SHF-77811"), "{strict:?}");
        assert!(!typo_pair(&strict, "BRK-10234", "BRK-10235"), "{strict:?}");
        assert_eq!((strict.stats.typo_reviews, strict.stats.typo_left_to_fs), (1, 1));
        let broad = run(TypoReview::Broad);
        assert!(typo_pair(&broad, "SHF-77810", "SHF-77811") && typo_pair(&broad, "BRK-10234", "BRK-10235"));
        let off = run(TypoReview::Off);
        assert_eq!((off.stats.typo_reviews, off.stats.typo_left_to_fs), (0, 0));
        // Nothing is ever merged: no candidate in any mode is `safe` here.
        assert!(
            [&strict, &broad, &off]
                .iter()
                .all(|r| r.candidates.iter().all(|c| !c.safe))
        );
    }

    #[test]
    fn deterministic() {
        let k: Vec<String> = (0..300).map(|i| format!("PN-{:05}", i * 7919 % 100_000)).collect();
        assert_eq!(
            resolve(Kind::Part, &k, &Config::default()),
            resolve(Kind::Part, &k, &Config::default())
        );
    }

    proptest! {
        #[test]
        fn part_normaliser_idempotent(s in "\\PC{0,24}") {
            prop_assert_eq!(part_key(&part_key(&s)), part_key(&s));
            prop_assert_eq!(part_loose(&part_loose(&s)), part_loose(&s));
        }

        #[test]
        fn company_normaliser_idempotent(s in "[\\PC\u{3041}-\u{30FF}\u{FF61}-\u{FF9F}株式会社有限㈱㈲() ]{0,20}") {
            prop_assert_eq!(normalize::canonical_company(&normalize::canonical_company(&s)), normalize::canonical_company(&s));
            let r = normalize::company_key(&s).romaji.unwrap_or_default();
            prop_assert_eq!(normalize::fold_romaji(&r), r);
        }

        #[test]
        fn comparison_is_symmetric(a in "[A-Z0-9OIl\\- ]{1,12}( REV [A-C])?", b in "[A-Z0-9OIl\\- ]{1,12}( REV [A-C])?",
                                   c in "[ア-ンァ-ョー東海精機株式会社 ]{1,10}", d in "[ア-ンァ-ョー東海精機株式会社 ]{1,10}") {
            let (pa, pb) = (prepare(Kind::Part, &a), prepare(Kind::Part, &b));
            prop_assert_eq!(gamma(Kind::Part, &pa, &pb), gamma(Kind::Part, &pb, &pa));
            let (ca, cb) = (prepare(Kind::Company, &c), prepare(Kind::Company, &d));
            prop_assert_eq!(gamma(Kind::Company, &ca, &cb), gamma(Kind::Company, &cb, &ca));
            prop_assert_eq!(jw(&c, &d).to_bits(), jw(&d, &c).to_bits());
        }

        #[test]
        fn resolution_symmetric_under_input_order(mut k in proptest::collection::vec("[A-C0-2O\\-]{3,8}", 0..30)) {
            let a = resolve(Kind::Part, &k, &Config::default());
            k.reverse();
            let b = resolve(Kind::Part, &k, &Config::default());
            prop_assert_eq!(a, b);
        }
    }
}

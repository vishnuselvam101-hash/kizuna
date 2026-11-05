//! Deterministic identity matching (directive §8): exact identifiers and safe normalisation first; anything looser
//! is only a *possible* match for a human to decide. Ambiguous clusters are never merged automatically.
//! Identity says nothing about specification compatibility.

use serde::Serialize;

pub mod v2;

/// Which matcher the API uses. v1 (rule-based) stays available behind this flag; v2 adds blocking,
/// Japanese-name comparators and EM-estimated Fellegi–Sunter weights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErVersion {
    V1,
    #[default]
    V2,
}

impl ErVersion {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "v1" | "1" => Some(ErVersion::V1),
            "v2" | "2" => Some(ErVersion::V2),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Match,
    PossibleMatch,
    NoMatch,
    RequiresHumanReview,
}

/// Safe normalisation: full-width ASCII → ASCII, ideographic space → space, trim, collapse spaces, upper-case.
/// These transformations never change which part is meant, so equal results are an automatic `Match`.
pub fn safe_normalize(s: &str) -> String {
    let mapped: String = s
        .chars()
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3000}' => ' ',
            _ => c,
        })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ").to_uppercase()
}

/// Loose key: also drops separators. Equal loose keys are only a *possible* match (BRK-100 vs BRK100 may differ).
pub fn loose_key(s: &str) -> String {
    safe_normalize(s).chars().filter(|c| c.is_alphanumeric()).collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Strips a trailing revision marker such as " REV C", "-REV.B", "_R3".
fn strip_revision(s: &str) -> Option<String> {
    let n = safe_normalize(s);
    for marker in [" REV ", "-REV", "_REV", " REV.", " R", "-R", "_R"] {
        if let Some(pos) = n.rfind(marker) {
            let tail = n[pos + marker.len()..].trim_start_matches(['.', ' ']);
            if !tail.is_empty() && tail.len() <= 3 && tail.chars().all(|c| c.is_ascii_alphanumeric()) && pos > 0 {
                return Some(n[..pos].to_string());
            }
        }
    }
    None
}

pub fn compare(a: &str, b: &str) -> (Outcome, Vec<&'static str>) {
    if safe_normalize(a) == safe_normalize(b) {
        let reason = if a == b {
            "identical"
        } else {
            "case_or_width_difference"
        };
        return (Outcome::Match, vec![reason]);
    }
    let (la, lb) = (loose_key(a), loose_key(b));
    if !la.is_empty() && la == lb {
        return (Outcome::PossibleMatch, vec!["separator_difference"]);
    }
    let (ra, rb) = (strip_revision(a), strip_revision(b));
    if ra.as_deref().map(loose_key).as_deref() == Some(lb.as_str())
        || rb.as_deref().map(loose_key).as_deref() == Some(la.as_str())
    {
        return (Outcome::PossibleMatch, vec!["revision_suffix_in_identifier"]);
    }
    if la.len() >= 6 && lb.len() >= 6 && edit_distance(&la, &lb) == 1 {
        return (Outcome::PossibleMatch, vec!["one_character_difference"]);
    }
    (Outcome::NoMatch, vec![])
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub left: String,
    pub right: String,
    pub outcome: Outcome,
    pub reasons: Vec<&'static str>,
}

/// Blocking keys: two identifiers can only be a (possible) match if they share one of these keys, so candidate
/// search is near-linear instead of all-pairs. Covers every rule in `compare`: safe-normalised equality, loose-key
/// equality, revision suffix stripped, and one-character edits (shared single-deletion variants, ids >= 6 chars).
fn block_keys(k: &str) -> Vec<String> {
    let loose = loose_key(k);
    let mut out = vec![format!("S:{}", safe_normalize(k)), format!("L:{loose}")];
    if let Some(r) = strip_revision(k) {
        out.push(format!("L:{}", loose_key(&r)));
    }
    let chars: Vec<char> = loose.chars().collect();
    if chars.len() >= 5 {
        out.push(format!("D:{loose}"));
        if chars.len() >= 6 {
            for i in 0..chars.len() {
                let v: String = chars
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, c)| c)
                    .collect();
                out.push(format!("D:{v}"));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Candidate pairs among distinct identifiers. Automatic `Match` pairs are reported too (for transparency).
/// A key with possible matches to two or more different keys is ambiguous: its pairs become `RequiresHumanReview`.
pub fn candidates(keys: &[String]) -> Vec<Candidate> {
    candidates_with_stats(keys).0
}

/// `candidates` plus the number of pairs the blocking passed to `compare` (for v1/v2 comparisons).
pub fn candidates_with_stats(keys: &[String]) -> (Vec<Candidate>, usize) {
    let mut ks: Vec<&String> = keys.iter().collect();
    ks.sort();
    ks.dedup();
    let mut blocks: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    for (i, k) in ks.iter().enumerate() {
        for b in block_keys(k) {
            blocks.entry(b).or_default().push(i);
        }
    }
    let mut pairs: std::collections::BTreeSet<(usize, usize)> = std::collections::BTreeSet::new();
    for members in blocks.values() {
        for x in 0..members.len() {
            for y in x + 1..members.len() {
                pairs.insert((members[x].min(members[y]), members[x].max(members[y])));
            }
        }
    }
    let compared = pairs.len();
    let mut out = Vec::new();
    for (i, j) in pairs {
        let (o, r) = compare(ks[i], ks[j]);
        if o != Outcome::NoMatch {
            out.push(Candidate {
                left: ks[i].clone(),
                right: ks[j].clone(),
                outcome: o,
                reasons: r,
            });
        }
    }
    let mut possible_count: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for c in out.iter().filter(|c| c.outcome == Outcome::PossibleMatch) {
        *possible_count.entry(c.left.as_str()).or_default() += 1;
        *possible_count.entry(c.right.as_str()).or_default() += 1;
    }
    let ambiguous: std::collections::HashSet<String> = possible_count
        .into_iter()
        .filter(|(_, n)| *n >= 2)
        .map(|(k, _)| k.to_string())
        .collect();
    for c in &mut out {
        if c.outcome == Outcome::PossibleMatch && (ambiguous.contains(&c.left) || ambiguous.contains(&c.right)) {
            c.outcome = Outcome::RequiresHumanReview;
            c.reasons.push("ambiguous_cluster");
        }
    }
    (out, compared)
}

/// Reference all-pairs implementation (tests only): the blocked search must return exactly the same pairs.
#[cfg(test)]
fn candidates_all_pairs(keys: &[String]) -> Vec<(String, String, Outcome)> {
    let mut ks: Vec<&String> = keys.iter().collect();
    ks.sort();
    ks.dedup();
    let mut out = Vec::new();
    for i in 0..ks.len() {
        for j in i + 1..ks.len() {
            let (o, _) = compare(ks[i], ks[j]);
            if o != Outcome::NoMatch {
                out.push((ks[i].clone(), ks[j].clone(), o));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn outcomes() {
        assert_eq!(compare("BRK-100", "BRK-100").0, Outcome::Match);
        assert_eq!(
            compare("ｂｒｋ－１００", "BRK-100").0,
            Outcome::Match,
            "full-width is safe"
        );
        assert_eq!(compare(" brk-100 ", "BRK-100").0, Outcome::Match);
        assert_eq!(compare("BRK-100", "BRK100").0, Outcome::PossibleMatch);
        assert_eq!(compare("BRK-100 REV C", "BRK-100").0, Outcome::PossibleMatch);
        assert_eq!(compare("BRK-1000", "BRK-1001").0, Outcome::PossibleMatch);
        assert_eq!(compare("BRK-100", "SHF-200").0, Outcome::NoMatch);
        assert_eq!(
            compare("A-1", "A-2").0,
            Outcome::NoMatch,
            "short ids never fuzzy-matched"
        );
    }

    #[test]
    fn ambiguous_clusters_need_review() {
        let keys: Vec<String> = ["BRK-1000", "BRK-1001", "BRK-1002"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let c = candidates(&keys);
        assert!(c.iter().all(|c| c.outcome == Outcome::RequiresHumanReview), "{c:?}");
        let keys: Vec<String> = ["BRK-100", "BRK100", "SHF-1"].iter().map(|s| s.to_string()).collect();
        assert_eq!(candidates(&keys).len(), 1);
    }

    #[test]
    fn blocked_search_is_fast_on_5000_keys() {
        let keys: Vec<String> = (0..5000)
            .map(|i| format!("PN-{:05}-{}", i * 7919 % 100_000, ["A", "B", "C"][i % 3]))
            .collect();
        let t = std::time::Instant::now();
        let _ = candidates(&keys);
        // Debug build bound; release timings are recorded by kizuna-bench (BUILD_STATUS).
        assert!(t.elapsed() < std::time::Duration::from_secs(5), "{:?}", t.elapsed());
    }

    proptest! {
        #[test]
        fn blocking_finds_exactly_the_all_pairs_candidates(keys in proptest::collection::vec("[A-Ca-c0-2 \\-]{1,9}( REV [A-C])?", 0..40)) {
            let blocked: Vec<(String, String)> = candidates(&keys).into_iter().map(|c| (c.left, c.right)).collect();
            let all: Vec<(String, String)> = candidates_all_pairs(&keys).into_iter().map(|(l, r, _)| (l, r)).collect();
            let (mut b, mut a) = (blocked, all);
            b.sort();
            a.sort();
            prop_assert_eq!(b, a);
        }

        #[test]
        fn normalize_is_idempotent_and_compare_symmetric(a in "\\PC{0,20}", b in "\\PC{0,20}") {
            prop_assert_eq!(safe_normalize(&safe_normalize(&a)), safe_normalize(&a));
            prop_assert_eq!(compare(&a, &b).0, compare(&b, &a).0);
        }
    }
}

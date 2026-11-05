//! Blocking for ER v2: the union of sorted-neighbourhood windows (forward and reversed keys), normalised-prefix
//! blocks and a q-gram index. Only pairs produced here are compared; the evaluation reports how many.

use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone, Default)]
pub struct RecordKeys {
    /// Keys sorted for the sorted-neighbourhood passes (forward and character-reversed).
    pub sort_keys: Vec<String>,
    /// Normalised prefixes; records sharing one are compared (block size capped).
    pub prefixes: Vec<String>,
    /// Distinct q-grams (bigrams) of the comparison keys.
    pub qgrams: Vec<String>,
    /// Exact keys (e.g. single-deletion variants): records sharing one are compared.
    pub exact: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct BlockConfig {
    pub window: usize,
    /// Prefix blocks larger than this are skipped (the other two methods still cover them).
    pub prefix_cap: usize,
    /// Q-grams occurring in more records than this are stop-grams and ignored.
    pub qgram_cap: usize,
    /// Pairs must share at least this share of the smaller record's q-grams.
    pub qgram_min_share: f64,
}

impl Default for BlockConfig {
    fn default() -> Self {
        BlockConfig {
            window: 4,
            prefix_cap: 64,
            qgram_cap: 256,
            qgram_min_share: 0.5,
        }
    }
}

pub fn qgrams(s: &str) -> Vec<String> {
    let c: Vec<char> = s.chars().collect();
    let mut out: Vec<String> = if c.len() < 2 {
        vec![s.to_string()]
    } else {
        c.windows(2).map(|w| w.iter().collect()).collect()
    };
    out.retain(|q| !q.is_empty());
    out.sort();
    out.dedup();
    out
}

fn add(pairs: &mut BTreeSet<(u32, u32)>, a: u32, b: u32) {
    if a != b {
        pairs.insert((a.min(b), a.max(b)));
    }
}

/// Candidate record pairs (i < j), sorted.
pub fn candidate_pairs(recs: &[RecordKeys], cfg: &BlockConfig) -> Vec<(u32, u32)> {
    let mut pairs = BTreeSet::new();
    // 1. Sorted neighbourhood, forward and reversed keys (the reversed pass catches early-character typos).
    for reversed in [false, true] {
        let mut entries: Vec<(String, u32)> = recs
            .iter()
            .enumerate()
            .flat_map(|(i, r)| {
                r.sort_keys.iter().map(move |k| {
                    let k = if reversed { k.chars().rev().collect() } else { k.clone() };
                    (k, i as u32)
                })
            })
            .collect();
        entries.sort();
        for x in 0..entries.len() {
            for y in x + 1..(x + cfg.window).min(entries.len()) {
                add(&mut pairs, entries[x].1, entries[y].1);
            }
        }
    }
    // 2. Normalised-prefix blocks.
    let mut blocks: HashMap<&str, Vec<u32>> = HashMap::new();
    for (i, r) in recs.iter().enumerate() {
        for p in &r.prefixes {
            blocks.entry(p.as_str()).or_default().push(i as u32);
        }
    }
    for members in blocks.values().filter(|m| m.len() <= cfg.prefix_cap) {
        for x in 0..members.len() {
            for y in x + 1..members.len() {
                add(&mut pairs, members[x], members[y]);
            }
        }
    }
    // 2b. Exact-key blocks (deletion neighbourhood: catches one substitution, insertion, deletion or
    // transposition even where prefix blocks are too large and q-grams are stop-grams).
    let mut exact: HashMap<&str, Vec<u32>> = HashMap::new();
    for (i, r) in recs.iter().enumerate() {
        for k in &r.exact {
            exact.entry(k.as_str()).or_default().push(i as u32);
        }
    }
    for members in exact.values().filter(|m| m.len() <= cfg.qgram_cap) {
        for x in 0..members.len() {
            for y in x + 1..members.len() {
                add(&mut pairs, members[x], members[y]);
            }
        }
    }
    // 3. Q-gram index with stop-grams removed.
    let mut postings: HashMap<&str, Vec<u32>> = HashMap::new();
    for (i, r) in recs.iter().enumerate() {
        for q in &r.qgrams {
            postings.entry(q.as_str()).or_default().push(i as u32);
        }
    }
    let mut shared: HashMap<u32, u32> = HashMap::new();
    for (i, r) in recs.iter().enumerate() {
        shared.clear();
        for q in &r.qgrams {
            let Some(list) = postings.get(q.as_str()).filter(|l| l.len() <= cfg.qgram_cap) else {
                continue;
            };
            for &j in list.iter().filter(|&&j| j as usize > i) {
                *shared.entry(j).or_insert(0) += 1;
            }
        }
        for (&j, &n) in &shared {
            let smaller = r.qgrams.len().min(recs[j as usize].qgrams.len()).max(1);
            if f64::from(n) >= (cfg.qgram_min_share * smaller as f64).ceil().max(1.0) {
                add(&mut pairs, i as u32, j);
            }
        }
    }
    pairs.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(k: &str) -> RecordKeys {
        RecordKeys {
            sort_keys: vec![k.into()],
            prefixes: vec![k.chars().take(3).collect()],
            qgrams: qgrams(k),
            exact: vec![],
        }
    }

    #[test]
    fn each_method_contributes() {
        let cfg = BlockConfig {
            window: 2,
            ..BlockConfig::default()
        };
        // Sorted neighbourhood: adjacent keys; reversed pass: same tail, different head.
        let r = [rec("AAA1"), rec("AAA2"), rec("ZZZ9"), rec("XB1234"), rec("YB1234")];
        let p = candidate_pairs(&r, &cfg);
        assert!(p.contains(&(0, 1)), "{p:?}");
        assert!(p.contains(&(3, 4)), "q-gram or reversed SN: {p:?}");
        assert!(!p.contains(&(0, 2)) || !p.contains(&(1, 2)), "{p:?}");
    }

    #[test]
    fn output_is_sorted_unique_and_ordered() {
        let r: Vec<RecordKeys> = (0..50).map(|i| rec(&format!("K{:03}", i * 7 % 50))).collect();
        let p = candidate_pairs(&r, &BlockConfig::default());
        assert!(p.windows(2).all(|w| w[0] < w[1]));
        assert!(p.iter().all(|(a, b)| a < b));
    }
}

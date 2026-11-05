//! Minimal document cover: given items that must be re-evidenced and, per item, the documents that can carry it,
//! choose the fewest documents that cover every coverable item (a set-cover / hitting-set problem).
//! Exact (smallest, then lexicographically first) when at most `EXACT_LIMIT` optional documents remain after
//! forced choices; otherwise greedy followed by redundancy pruning, which guarantees no chosen document can be
//! dropped (irredundant) but not global minimality. The result says which one applied.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

pub const EXACT_LIMIT: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cover {
    pub docs: Vec<String>,
    /// True when `docs` is a proven minimum.
    pub exact: bool,
    /// Items no document can carry (reported, never silently dropped).
    pub uncovered: Vec<String>,
}

fn covers(chosen: &BTreeSet<&str>, needs: &BTreeMap<&str, &BTreeSet<String>>) -> bool {
    needs.values().all(|c| c.iter().any(|d| chosen.contains(d.as_str())))
}

pub fn minimal_cover(needs: &BTreeMap<String, BTreeSet<String>>) -> Cover {
    let uncovered: Vec<String> = needs
        .iter()
        .filter(|(_, c)| c.is_empty())
        .map(|(i, _)| i.clone())
        .collect();
    let mut chosen: BTreeSet<&str> = BTreeSet::new();
    // Forced: an item with one candidate document needs that document.
    for c in needs.values().filter(|c| c.len() == 1) {
        if let Some(d) = c.iter().next() {
            chosen.insert(d.as_str());
        }
    }
    let rest: BTreeMap<&str, &BTreeSet<String>> = needs
        .iter()
        .filter(|(_, c)| !c.is_empty() && !c.iter().any(|d| chosen.contains(d.as_str())))
        .map(|(i, c)| (i.as_str(), c))
        .collect();
    let optional: Vec<&str> = rest
        .values()
        .flat_map(|c| c.iter().map(String::as_str))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let exact = optional.len() <= EXACT_LIMIT;
    if !rest.is_empty() {
        if exact {
            // Smallest subset first; within a size, the lexicographically first by bit order of sorted names.
            let n = optional.len();
            let mut best: Option<u32> = None;
            for mask in 1u32..(1u32 << n) {
                if best.is_some_and(|b| mask.count_ones() >= b.count_ones()) {
                    continue;
                }
                let pick: BTreeSet<&str> = (0..n).filter(|i| mask & (1 << i) != 0).map(|i| optional[i]).collect();
                if covers(&pick, &rest) {
                    best = Some(mask);
                }
            }
            if let Some(m) = best {
                chosen.extend((0..n).filter(|i| m & (1 << i) != 0).map(|i| optional[i]));
            }
        } else {
            let mut open: BTreeMap<&str, &BTreeSet<String>> = rest.clone();
            let mut picked: Vec<&str> = Vec::new();
            while !open.is_empty() {
                let mut best: Option<(&str, usize)> = None;
                for d in &optional {
                    let n = open.values().filter(|c| c.contains(*d)).count();
                    if n > 0 && best.is_none_or(|(_, b)| n > b) {
                        best = Some((d, n));
                    }
                }
                let Some((d, _)) = best else { break };
                picked.push(d);
                open.retain(|_, c| !c.contains(d));
            }
            // Prune: drop any picked document whose items stay covered without it (latest picks first).
            let mut keep: BTreeSet<&str> = picked.iter().copied().collect();
            for d in picked.iter().rev() {
                keep.remove(d);
                let all: BTreeSet<&str> = keep.iter().chain(chosen.iter()).copied().collect();
                if !covers(&all, &rest) {
                    keep.insert(d);
                }
            }
            chosen.extend(keep);
        }
    }
    Cover {
        docs: chosen.into_iter().map(str::to_string).collect(),
        exact,
        uncovered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn needs(v: &[(&str, &[&str])]) -> BTreeMap<String, BTreeSet<String>> {
        v.iter()
            .map(|(i, c)| (i.to_string(), c.iter().map(|d| d.to_string()).collect()))
            .collect()
    }

    #[test]
    fn forced_and_shared_documents() {
        let c = minimal_cover(&needs(&[
            ("a", &["d1"]),
            ("b", &["d1", "d2"]),
            ("c", &["d2", "d3"]),
            ("x", &[]),
        ]));
        assert_eq!(c.docs, ["d1", "d2"]);
        assert!(c.exact);
        assert_eq!(c.uncovered, ["x"]);
    }

    #[test]
    fn exact_beats_greedy_trap() {
        // Greedy picks the 3-item document first and then needs two more; the optimum is two documents.
        let c = minimal_cover(&needs(&[
            ("1", &["A", "C"]),
            ("2", &["A", "C"]),
            ("3", &["A", "D"]),
            ("4", &["B", "D"]),
            ("5", &["B", "D"]),
            ("6", &["B", "C"]),
        ]));
        assert_eq!(c.docs.len(), 2);
        assert!(c.exact);
    }

    #[test]
    fn large_inputs_use_irredundant_greedy() {
        let mut n = BTreeMap::new();
        for i in 0..40 {
            n.insert(
                format!("i{i:02}"),
                [format!("d{:02}", i / 2), format!("d{:02}", (i / 2 + 1) % 20 + 20)]
                    .into_iter()
                    .collect(),
            );
        }
        let c = minimal_cover(&n);
        assert!(!c.exact);
        let chosen: BTreeSet<&str> = c.docs.iter().map(String::as_str).collect();
        let n2: BTreeMap<&str, &BTreeSet<String>> = n.iter().map(|(k, v)| (k.as_str(), v)).collect();
        assert!(covers(&chosen, &n2));
        for d in &c.docs {
            let mut less = chosen.clone();
            less.remove(d.as_str());
            assert!(!covers(&less, &n2), "{d} is redundant");
        }
    }

    #[test]
    fn empty_needs_choose_nothing() {
        let c = minimal_cover(&BTreeMap::new());
        assert!(c.docs.is_empty() && c.exact && c.uncovered.is_empty());
    }
}

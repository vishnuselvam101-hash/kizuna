//! Near-duplicate measurement tables across certificates: MinHash signatures + LSH banding.
//!
//! A table (rows of measured values) becomes a set of tokens: one token per row (all canonical cell values in
//! order) and one per cell (column index + canonical value). Copying a table and editing a few cells keeps most
//! tokens, so the Jaccard similarity J = |A ∩ B| / |A ∪ B| stays high; independent measurements share few tokens.
//!
//! MinHash (Broder 1997): for k hash functions h_i, P(min h_i(A) = min h_i(B)) = J, so the fraction of equal
//! signature slots estimates J with standard error ≈ √(J(1 − J)/k). Here h_i(t) = mix(fnv1a64(t) ⊕ seed_i), with
//! `mix` the SplitMix64 finaliser (a bijection on u64) and seed_i from a SplitMix64 stream of a fixed seed, so
//! signatures are identical on every machine and run (approximate min-wise independence; exact permutations are
//! not needed in practice, Leskovec, Rajaraman & Ullman, *Mining of Massive Datasets*, ch. 3).
//!
//! LSH banding (MMDS §3.4): split the k slots into b bands of r rows; two tables become a candidate pair when any
//! band matches exactly, which happens with probability 1 − (1 − J^r)^b (an S-curve with threshold ≈ (1/b)^{1/r}).
//! Candidates are then kept only if the estimated J ≥ `threshold`. Defaults (k = 128, b = 32, r = 4) put the S-curve
//! threshold at ≈ 0.42 and detect J ≥ 0.6 with probability > 0.99.

use std::collections::{BTreeMap, BTreeSet};

use crate::decimal::Measurement;
use crate::error::{StatsError, StatsResult};

/// Default seed for the hash family (any fixed value works; changing it changes every signature).
pub const DEFAULT_SEED: u64 = 0x4b49_5a55_4e41_0001;

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    mix64(*state)
}

/// SplitMix64 finaliser (Steele, Lea & Flood 2014): a bijective 64-bit mixer.
pub fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// FNV-1a 64-bit hash of bytes (Fowler–Noll–Vo).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Token set of a table (sorted, de-duplicated hashes). Values are compared canonically ("12.30" = "12.3").
pub fn table_tokens(rows: &[Vec<Measurement>]) -> Vec<u64> {
    let mut out = BTreeSet::new();
    for row in rows {
        let cells: Vec<String> = row.iter().map(Measurement::canonical).collect();
        out.insert(fnv1a64(format!("R|{}", cells.join("\u{1f}")).as_bytes()));
        for (j, c) in cells.iter().enumerate() {
            out.insert(fnv1a64(format!("C|{j}|{c}").as_bytes()));
        }
    }
    out.into_iter().collect()
}

/// A MinHash signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    slots: Vec<u64>,
    empty: bool,
}

impl Signature {
    /// Estimated Jaccard similarity in [0, 1]; 0 if either set is empty or lengths differ.
    pub fn jaccard(&self, other: &Signature) -> f64 {
        if self.empty || other.empty || self.slots.len() != other.slots.len() || self.slots.is_empty() {
            return 0.0;
        }
        let eq = self.slots.iter().zip(&other.slots).filter(|(a, b)| a == b).count();
        eq as f64 / self.slots.len() as f64
    }

    /// Signature slots.
    pub fn slots(&self) -> &[u64] {
        &self.slots
    }
}

/// A seeded family of k hash functions.
#[derive(Debug, Clone)]
pub struct MinHasher {
    seeds: Vec<u64>,
}

impl MinHasher {
    /// `k` hash functions from `seed` (k ≥ 1).
    pub fn new(k: usize, seed: u64) -> StatsResult<Self> {
        if k == 0 {
            return Err(StatsError::InvalidParameter("minhash needs k >= 1"));
        }
        let mut s = seed;
        Ok(MinHasher {
            seeds: (0..k).map(|_| splitmix64(&mut s)).collect(),
        })
    }

    /// Signature of a token set.
    pub fn signature(&self, tokens: &[u64]) -> Signature {
        let slots = self
            .seeds
            .iter()
            .map(|s| tokens.iter().map(|t| mix64(t ^ s)).min().unwrap_or(u64::MAX))
            .collect();
        Signature {
            slots,
            empty: tokens.is_empty(),
        }
    }
}

/// Exact Jaccard similarity of two sorted, de-duplicated token sets (for verification and tests).
pub fn exact_jaccard(a: &[u64], b: &[u64]) -> f64 {
    let (mut i, mut j, mut inter) = (0, 0, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                inter += 1;
                i += 1;
                j += 1;
            }
        }
    }
    let union = a.len() + b.len() - inter;
    if union == 0 { 0.0 } else { inter as f64 / union as f64 }
}

/// A pair of tables (indices into the input) with their estimated similarity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NearDuplicate {
    pub a: usize,
    pub b: usize,
    pub jaccard_estimate: f64,
}

/// LSH over `signatures` with `bands` × `rows` = k; returns pairs (a < b) with estimated J ≥ `threshold`, sorted.
pub fn find_near_duplicates(
    signatures: &[Signature],
    bands: usize,
    rows: usize,
    threshold: f64,
) -> StatsResult<Vec<NearDuplicate>> {
    let k = signatures.first().map_or(bands * rows, |s| s.slots.len());
    if bands == 0 || rows == 0 || bands * rows != k || signatures.iter().any(|s| s.slots.len() != k) {
        return Err(StatsError::InvalidParameter(
            "bands × rows must equal the signature length of every table",
        ));
    }
    if !(0.0..=1.0).contains(&threshold) {
        return Err(StatsError::InvalidParameter("threshold must be in [0, 1]"));
    }
    let mut buckets: BTreeMap<(usize, u64), Vec<usize>> = BTreeMap::new();
    for (doc, sig) in signatures.iter().enumerate().filter(|(_, s)| !s.empty) {
        for (band, chunk) in sig.slots.chunks(rows).enumerate() {
            let key = chunk.iter().fold(band as u64, |h, v| mix64(h ^ v));
            buckets.entry((band, key)).or_default().push(doc);
        }
    }
    let mut pairs = BTreeSet::new();
    for docs in buckets.values() {
        for (x, &a) in docs.iter().enumerate() {
            for &b in &docs[x + 1..] {
                pairs.insert((a.min(b), a.max(b)));
            }
        }
    }
    Ok(pairs
        .into_iter()
        .filter_map(|(a, b)| {
            let j = signatures[a].jaccard(&signatures[b]);
            (j >= threshold).then_some(NearDuplicate {
                a,
                b,
                jaccard_estimate: j,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(rows: &[&[&str]]) -> Vec<Vec<Measurement>> {
        rows.iter().map(|r| Measurement::parse_all(r).unwrap()).collect()
    }

    #[test]
    fn tokens_hand_counted() {
        // 2 rows × 2 cols: 2 row tokens + 4 cell tokens; "1.50" and "1.5" are the same value.
        let t = table(&[&["1.50", "2"], &["3", "4"]]);
        assert_eq!(table_tokens(&t).len(), 6);
        assert_eq!(table_tokens(&t), table_tokens(&table(&[&["1.5", "2.0"], &["3", "4"]])));
        // Same values in the other column are different tokens.
        let u = table(&[&["2", "1.5"], &["3", "4"]]);
        // shared: C|0|3, C|1|4, row "3,4" → 3 of 9 distinct → J = 1/3.
        assert!((exact_jaccard(&table_tokens(&t), &table_tokens(&u)) - 1.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn estimate_tracks_exact_jaccard() {
        let h = MinHasher::new(256, DEFAULT_SEED).unwrap();
        let a: Vec<u64> = (0..1000).collect();
        let b: Vec<u64> = (500..1500).collect(); // J = 500/1500 = 1/3
        let est = h.signature(&a).jaccard(&h.signature(&b));
        // 4 standard errors of √(J(1−J)/k) = 0.0295.
        assert!((est - 1.0 / 3.0).abs() < 4.0 * 0.0295, "{est}");
        assert_eq!(h.signature(&a).jaccard(&h.signature(&a)), 1.0);
        assert_eq!(h.signature(&[]).jaccard(&h.signature(&[])), 0.0);
    }

    #[test]
    fn deterministic_signatures() {
        let a = MinHasher::new(4, DEFAULT_SEED).unwrap().signature(&[1, 2, 3]);
        let b = MinHasher::new(4, DEFAULT_SEED).unwrap().signature(&[3, 2, 1]);
        assert_eq!(a, b);
        assert_ne!(a, MinHasher::new(4, 7).unwrap().signature(&[1, 2, 3]));
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c); // published FNV-1a test vector
    }

    #[test]
    fn lsh_finds_copied_table_only() {
        let h = MinHasher::new(128, DEFAULT_SEED).unwrap();
        let base: Vec<u64> = (0..300).collect();
        let mut copy = base.clone();
        copy[..20].iter_mut().for_each(|t| *t += 10_000); // ~87% similar
        let other: Vec<u64> = (5000..5300).collect();
        let sigs = [h.signature(&base), h.signature(&other), h.signature(&copy)];
        let found = find_near_duplicates(&sigs, 32, 4, 0.5).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].a, found[0].b), (0, 2));
        assert!(find_near_duplicates(&sigs, 30, 4, 0.5).is_err());
        assert!(MinHasher::new(0, 1).is_err());
    }

    proptest::proptest! {
        #[test]
        fn estimate_bounded_and_symmetric(a in proptest::collection::btree_set(0u64..500, 0..80), b in proptest::collection::btree_set(0u64..500, 0..80)) {
            let h = MinHasher::new(64, DEFAULT_SEED).unwrap();
            let (a, b): (Vec<u64>, Vec<u64>) = (a.into_iter().collect(), b.into_iter().collect());
            let (sa, sb) = (h.signature(&a), h.signature(&b));
            let j = sa.jaccard(&sb);
            proptest::prop_assert!((0.0..=1.0).contains(&j));
            proptest::prop_assert_eq!(j, sb.jaccard(&sa));
            if exact_jaccard(&a, &b) == 0.0 { proptest::prop_assert!(j < 0.2); }
        }
    }
}

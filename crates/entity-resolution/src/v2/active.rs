//! Active learning for entity resolution from reviewer decisions.
//!
//! The Fellegi–Sunter parameters estimated by EM (`m`, `u`, `p`) seed a conjugate model: each field's `m` and `u`
//! distributions become Dirichlet pseudo-counts and the match share `p` a Beta pseudo-count, all with the same
//! configurable prior strength `s` (pseudo-pairs). A reviewer decision on a pair with comparison vector γ adds one
//! count to the match (or non-match) class for every observed field level of γ and to the class total. The
//! posterior-mean parameters are closed form (counts divided by totals), so a refit is O(1), exact and
//! deterministic; with no decisions the posterior equals the plain Fellegi–Sunter posterior of the EM parameters.
//!
//! Uncertainty sampling ([`order_queue`], [`next_item`]) orders the unreviewed items by |log-odds| ascending (the
//! same order as |P(match) − 0.5|), ties broken by a seeded stable hash of the item index. [`baseline_order`] is
//! the current queue order (weight descending) for comparison.
//!
//! **Humans decide.** The model only orders the review queue and offers a suggestion ([`ActiveModel::suggest`]).
//! It never merges anything and never changes, reverses or reorders a recorded decision: decisions are immutable,
//! append-only inputs ([`ActiveModel::decisions`]), and a reviewed item never re-enters the queue. A suggestion for
//! an unreviewed item is advice for the reviewer, not an outcome.
//!
//! Field levels outside the model (index ≥ number of levels for that field) and missing fields contribute nothing,
//! as in [`super::em::weight`].

use super::em::{FsParams, Gamma};

/// Floor for posterior-mean probabilities inside logarithms (a prior with an exact zero stays finite).
const FLOOR: f64 = 1e-12;

/// A reviewer decision as recorded: the comparison vector of the pair and the human's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub gamma: Gamma,
    pub is_match: bool,
}

/// A suggestion for an unreviewed pair. Advice only; the reviewer's decision is what counts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Suggestion {
    pub p_match: f64,
    /// `p_match >= 0.5`.
    pub is_match: bool,
}

/// Conjugate Fellegi–Sunter model updated by reviewer decisions.
#[derive(Debug, Clone)]
pub struct ActiveModel {
    prior: FsParams,
    strength: f64,
    m_counts: Vec<Vec<f64>>,
    u_counts: Vec<Vec<f64>>,
    n_match: f64,
    n_non: f64,
    decisions: Vec<Decision>,
}

/// Per-field, per-level ln(m̂/û) and the prior log-odds, computed once per query batch.
struct LogTable {
    prior: f64,
    fields: Vec<Vec<f64>>,
}

impl LogTable {
    fn log_odds(&self, g: &Gamma) -> f64 {
        self.prior
            + g.iter()
                .zip(&self.fields)
                .filter_map(|(l, f)| l.and_then(|l| f.get(l as usize)))
                .sum::<f64>()
    }
}

impl ActiveModel {
    /// `prior_strength` (> 0) is the weight of the EM parameters in pseudo-pairs, for every field's m and u and
    /// for p. Non-positive or non-finite values are replaced by a tiny positive strength.
    pub fn new(prior: &FsParams, prior_strength: f64) -> Self {
        let strength = if prior_strength.is_finite() && prior_strength > 0.0 {
            prior_strength
        } else {
            1e-6
        };
        let zeros = |v: &[Vec<f64>]| v.iter().map(|f| vec![0.0; f.len()]).collect::<Vec<_>>();
        ActiveModel {
            m_counts: zeros(&prior.m),
            u_counts: zeros(&prior.u),
            prior: prior.clone(),
            strength,
            n_match: 0.0,
            n_non: 0.0,
            decisions: Vec::new(),
        }
    }

    /// Records one reviewer decision (append-only; earlier decisions are never touched).
    pub fn record(&mut self, gamma: &Gamma, is_match: bool) {
        let counts = if is_match {
            self.n_match += 1.0;
            &mut self.m_counts
        } else {
            self.n_non += 1.0;
            &mut self.u_counts
        };
        for (l, f) in gamma.iter().zip(counts.iter_mut()) {
            if let Some(c) = l.and_then(|l| f.get_mut(l as usize)) {
                *c += 1.0;
            }
        }
        self.decisions.push(Decision {
            gamma: gamma.clone(),
            is_match,
        });
    }

    /// Every recorded decision, in the order recorded.
    pub fn decisions(&self) -> &[Decision] {
        &self.decisions
    }

    /// Posterior-mean parameters after the recorded decisions (Dirichlet/Beta means).
    pub fn params(&self) -> FsParams {
        let s = self.strength;
        let mean = |prior: &[Vec<f64>], counts: &[Vec<f64>]| -> Vec<Vec<f64>> {
            prior
                .iter()
                .zip(counts)
                .map(|(pf, cf)| {
                    let total = s * pf.iter().sum::<f64>() + cf.iter().sum::<f64>();
                    pf.iter().zip(cf).map(|(p, c)| (s * p + c) / total).collect()
                })
                .collect()
        };
        FsParams {
            p: (s * self.prior.p + self.n_match) / (s + self.n_match + self.n_non),
            m: mean(&self.prior.m, &self.m_counts),
            u: mean(&self.prior.u, &self.u_counts),
        }
    }

    fn table(&self) -> LogTable {
        let par = self.params();
        let p = par.p.clamp(FLOOR, 1.0 - FLOOR);
        LogTable {
            prior: (p / (1.0 - p)).ln(),
            fields: par
                .m
                .iter()
                .zip(&par.u)
                .map(|(m, u)| {
                    m.iter()
                        .zip(u)
                        .map(|(a, b)| (a.max(FLOOR) / b.max(FLOOR)).ln())
                        .collect()
                })
                .collect(),
        }
    }

    /// Natural-log posterior odds of a match for γ.
    pub fn log_odds(&self, g: &Gamma) -> f64 {
        self.table().log_odds(g)
    }

    /// P(match | γ) in [0, 1].
    pub fn posterior(&self, g: &Gamma) -> f64 {
        sigmoid(self.log_odds(g))
    }

    pub fn suggest(&self, g: &Gamma) -> Suggestion {
        let p_match = self.posterior(g);
        Suggestion {
            p_match,
            is_match: p_match >= 0.5,
        }
    }

    /// Suggested label (posterior ≥ 0.5) for every item, sharing one parameter table.
    pub fn suggest_all(&self, items: &[Gamma]) -> Vec<bool> {
        let t = self.table();
        items.iter().map(|g| sigmoid(t.log_odds(g)) >= 0.5).collect()
    }
}

fn sigmoid(x: f64) -> f64 {
    (1.0 / (1.0 + (-x).exp())).clamp(0.0, 1.0)
}

/// Seeded stable tie-break key of an item index.
pub fn tie_key(seed: u64, index: usize) -> u64 {
    super::Rng::new(seed ^ (index as u64).wrapping_mul(0xD1B5_4A32_D192_ED03)).next_u64()
}

fn unreviewed(reviewed: &[bool], i: usize) -> bool {
    !reviewed.get(i).copied().unwrap_or(false)
}

/// Uncertainty sampling: indices of the unreviewed items (`reviewed[i] == false`, missing entries count as
/// unreviewed) sorted by |log-odds| ascending, ties by [`tie_key`].
pub fn order_queue(model: &ActiveModel, items: &[Gamma], reviewed: &[bool], seed: u64) -> Vec<usize> {
    let t = model.table();
    let mut keyed: Vec<(f64, u64, usize)> = (0..items.len())
        .filter(|&i| unreviewed(reviewed, i))
        .map(|i| (t.log_odds(&items[i]).abs(), tie_key(seed, i), i))
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    keyed.into_iter().map(|k| k.2).collect()
}

/// The first item of [`order_queue`] in O(n) (for refitting after every decision).
pub fn next_item(model: &ActiveModel, items: &[Gamma], reviewed: &[bool], seed: u64) -> Option<usize> {
    let t = model.table();
    (0..items.len())
        .filter(|&i| unreviewed(reviewed, i))
        .map(|i| (t.log_odds(&items[i]).abs(), tie_key(seed, i), i))
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)))
        .map(|k| k.2)
}

/// Baseline queue order: unreviewed items by Fellegi–Sunter weight descending, ties by index ascending.
pub fn baseline_order(weights: &[f64], reviewed: &[bool]) -> Vec<usize> {
    let mut v: Vec<usize> = (0..weights.len()).filter(|&i| unreviewed(reviewed, i)).collect();
    v.sort_by(|&a, &b| weights[b].total_cmp(&weights[a]).then(a.cmp(&b)));
    v
}

// ---------------------------------------------------------------------------------------------------------------
// Pattern-stratified active learning (ER v3)
// ---------------------------------------------------------------------------------------------------------------
//
// Why plain uncertainty sampling fails on the large part sets: the comparison vectors take only a dozen distinct
// patterns, and every item of one pattern has the same log-odds. The factorized model above is confident (and wrong)
// on some patterns — e.g. "one character apart, digits differ" is 30 % true matches but predicted non-match with a
// large |log-odds| — so uncertainty sampling never queries them; it spends its reviews on the least-confident
// patterns (all true matches there), and those biased labels push the global match share p and the m counts further.
//
// The stratified model estimates the match rate of each pattern directly: θ_γ ~ Beta(s·q(γ), s·(1 − q(γ))), with
// q(γ) the EM Fellegi–Sunter posterior (clamped to [floor, 1 − floor]) and s a weak prior strength, updated only by decisions on that pattern. Items
// of one pattern are exchangeable under this model and are reviewed in seeded random order, so each pattern's
// estimate comes from a random sample of that pattern (no cross-pattern sampling bias). Queue: first `probes` items
// of every pattern (largest pattern first), then the pattern with the highest min(θ̂, 1 − θ̂) (expected errors fixed
// per review), ties by more remaining items. Humans decide; the model only orders and suggests.

/// Settings of the stratified strategy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StratifiedConfig {
    /// Pseudo-counts of the Fellegi–Sunter posterior in each pattern's Beta prior.
    pub prior_strength: f64,
    /// Seeded random reviews per pattern before uncertainty ordering (exploration of confident patterns).
    pub probes: u32,
    /// The FS posterior used as each pattern's prior mean is clamped to [floor, 1 − floor]: EM posteriors of
    /// 1e-19 would otherwise keep a pattern at "certain" after a few probes even when 1 in 10 of it matches.
    pub prior_floor: f64,
}

impl Default for StratifiedConfig {
    fn default() -> Self {
        StratifiedConfig {
            prior_strength: 2.0,
            probes: 3,
            prior_floor: 0.05,
        }
    }
}

/// Per-pattern Beta model seeded by the EM Fellegi–Sunter posterior.
#[derive(Debug, Clone)]
pub struct PatternModel {
    base: ActiveModel,
    cfg: StratifiedConfig,
    /// (matches, non-matches) decided per pattern.
    counts: std::collections::BTreeMap<Gamma, (u32, u32)>,
    decisions: Vec<Decision>,
}

impl PatternModel {
    pub fn new(prior: &FsParams, cfg: StratifiedConfig) -> Self {
        let strength = if cfg.prior_strength.is_finite() && cfg.prior_strength > 0.0 {
            cfg.prior_strength
        } else {
            1e-6
        };
        PatternModel {
            // Strength is irrelevant for the base: it is never updated, so it equals the FS posterior.
            base: ActiveModel::new(prior, 1.0),
            cfg: StratifiedConfig {
                prior_strength: strength,
                ..cfg
            },
            counts: std::collections::BTreeMap::new(),
            decisions: Vec::new(),
        }
    }

    /// Records one reviewer decision (append-only).
    pub fn record(&mut self, gamma: &Gamma, is_match: bool) {
        let c = self.counts.entry(gamma.clone()).or_insert((0, 0));
        if is_match {
            c.0 += 1;
        } else {
            c.1 += 1;
        }
        self.decisions.push(Decision {
            gamma: gamma.clone(),
            is_match,
        });
    }

    pub fn decisions(&self) -> &[Decision] {
        &self.decisions
    }

    fn reviews(&self, g: &Gamma) -> u32 {
        self.counts.get(g).map_or(0, |c| c.0 + c.1)
    }

    fn mean(&self, q: f64, g: &Gamma) -> f64 {
        let (a, b) = self.counts.get(g).copied().unwrap_or((0, 0));
        let s = self.cfg.prior_strength;
        let f = self.cfg.prior_floor.clamp(0.0, 0.5);
        let q = if q.is_finite() { q.clamp(f, 1.0 - f) } else { 0.5 };
        (s * q + f64::from(a)) / (s + f64::from(a) + f64::from(b))
    }

    /// Posterior mean match rate of pattern γ.
    pub fn posterior(&self, g: &Gamma) -> f64 {
        self.mean(self.base.posterior(g), g)
    }

    pub fn suggest(&self, g: &Gamma) -> Suggestion {
        let p_match = self.posterior(g);
        Suggestion {
            p_match,
            is_match: p_match >= 0.5,
        }
    }

    /// Suggested label (posterior mean ≥ 0.5) for every item.
    pub fn suggest_all(&self, items: &[Gamma]) -> Vec<bool> {
        let t = self.base.table();
        items
            .iter()
            .map(|g| self.mean(sigmoid(t.log_odds(g)), g) >= 0.5)
            .collect()
    }
}

/// Next item of the stratified strategy (see the section comment), or None when everything is reviewed.
/// Deterministic for (model, items, reviewed, seed).
pub fn next_item_stratified(model: &PatternModel, items: &[Gamma], reviewed: &[bool], seed: u64) -> Option<usize> {
    // Per pattern: remaining items and the unreviewed item with the smallest seeded key.
    let mut groups: std::collections::BTreeMap<&Gamma, (usize, (u64, usize))> = std::collections::BTreeMap::new();
    for i in (0..items.len()).filter(|&i| unreviewed(reviewed, i)) {
        let key = (tie_key(seed, i), i);
        let e = groups.entry(&items[i]).or_insert((0, key));
        e.0 += 1;
        e.1 = e.1.min(key);
    }
    let probing = groups
        .iter()
        .filter(|(g, _)| model.reviews(g) < model.cfg.probes)
        .max_by(|a, b| a.1.0.cmp(&b.1.0).then(b.1.1.cmp(&a.1.1)));
    if let Some((_, (_, (_, i)))) = probing {
        return Some(*i);
    }
    let t = model.base.table();
    groups
        .iter()
        .map(|(g, (left, key))| {
            let q = model.mean(sigmoid(t.log_odds(g)), g);
            (q.min(1.0 - q), *left, *key)
        })
        .max_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(b.2.cmp(&a.2)))
        .map(|(_, _, (_, i))| i)
}

/// Sort key of [`order_queue_stratified`]: (phase, probe index, −uncertainty, MAX − remaining, (tie key, item)).
type QueueKey = (u8, u32, f64, usize, (u64, usize));

/// Whole queue in stratified order for the current model (no refit between items; the API recomputes it after
/// every decision). Probe items first — the j-th probe of every pattern before any (j+1)-th, larger patterns first
/// — then the rest by min(θ̂, 1 − θ̂) descending, more remaining items, seeded key. Its first item is
/// [`next_item_stratified`]. Deterministic for (model, items, reviewed, seed).
pub fn order_queue_stratified(model: &PatternModel, items: &[Gamma], reviewed: &[bool], seed: u64) -> Vec<usize> {
    let mut groups: std::collections::BTreeMap<&Gamma, Vec<(u64, usize)>> = std::collections::BTreeMap::new();
    for i in (0..items.len()).filter(|&i| unreviewed(reviewed, i)) {
        groups.entry(&items[i]).or_default().push((tie_key(seed, i), i));
    }
    let t = model.base.table();
    let mut keyed: Vec<QueueKey> = Vec::new();
    for (g, mut members) in groups {
        members.sort_unstable();
        let left = members.len();
        let need = model.cfg.probes.saturating_sub(model.reviews(g)) as usize;
        let q = model.mean(sigmoid(t.log_odds(g)), g);
        for (j, key) in members.into_iter().enumerate() {
            if j < need {
                keyed.push((0, j as u32, 0.0, usize::MAX - left, key));
            } else {
                keyed.push((1, 0, -q.min(1.0 - q), usize::MAX - left, key));
            }
        }
    }
    keyed.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.total_cmp(&b.2))
            .then(a.3.cmp(&b.3))
            .then(a.4.cmp(&b.4))
    });
    keyed.into_iter().map(|k| k.4.1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn params() -> FsParams {
        FsParams {
            p: 0.05,
            m: vec![vec![0.7, 0.2, 0.1], vec![0.9, 0.1], vec![0.8, 0.2]],
            u: vec![vec![0.05, 0.15, 0.8], vec![0.3, 0.7], vec![0.4, 0.6]],
        }
    }

    fn fs_posterior(par: &FsParams, g: &Gamma) -> f64 {
        let (mut a, mut b) = (par.p, 1.0 - par.p);
        for (f, l) in g.iter().enumerate() {
            if let Some(l) = l {
                a *= par.m[f][*l as usize];
                b *= par.u[f][*l as usize];
            }
        }
        a / (a + b)
    }

    fn all_patterns() -> Vec<Gamma> {
        let mut out = Vec::new();
        for a in [None, Some(0), Some(1), Some(2)] {
            for b in [None, Some(0), Some(1)] {
                for c in [None, Some(0), Some(1)] {
                    out.push(vec![a, b, c]);
                }
            }
        }
        out
    }

    #[test]
    fn zero_labels_equals_fs_posterior() {
        let par = params();
        for s in [0.5, 10.0, 1000.0] {
            let m = ActiveModel::new(&par, s);
            for g in all_patterns() {
                let (x, y) = (m.posterior(&g), fs_posterior(&par, &g));
                assert!((x - y).abs() < 1e-12, "{g:?}: {x} vs {y}");
            }
            let q = m.params();
            assert!((q.p - par.p).abs() < 1e-12);
            for (a, b) in
                q.m.iter()
                    .flatten()
                    .chain(q.u.iter().flatten())
                    .zip(par.m.iter().flatten().chain(par.u.iter().flatten()))
            {
                assert!((a - b).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn posterior_moves_toward_labels() {
        let par = params();
        let g: Gamma = vec![Some(1), Some(0), Some(1)];
        let mut m = ActiveModel::new(&par, 10.0);
        let p0 = m.posterior(&g);
        for _ in 0..20 {
            m.record(&g, true);
        }
        assert!(
            m.posterior(&g) > p0 && m.posterior(&g) > 0.9,
            "{p0} -> {}",
            m.posterior(&g)
        );
        let mut n = ActiveModel::new(&par, 10.0);
        let strong: Gamma = vec![Some(0), Some(0), Some(0)];
        let q0 = n.posterior(&strong);
        for _ in 0..50 {
            n.record(&strong, false);
        }
        assert!(
            n.posterior(&strong) < q0 && n.posterior(&strong) < 0.5,
            "{q0} -> {}",
            n.posterior(&strong)
        );
        assert_eq!(n.decisions().len(), 50);
    }

    #[test]
    fn ordering_puts_most_uncertain_first() {
        let par = params();
        let m = ActiveModel::new(&par, 10.0);
        let items = all_patterns();
        let order = order_queue(&m, &items, &[], 7);
        let first = m.posterior(&items[order[0]]);
        let best = items
            .iter()
            .map(|g| (m.posterior(g) - 0.5).abs())
            .fold(f64::INFINITY, f64::min);
        assert!(((first - 0.5).abs() - best).abs() < 1e-12);
        let d: Vec<f64> = order.iter().map(|&i| m.log_odds(&items[i]).abs()).collect();
        assert!(d.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(next_item(&m, &items, &[], 7), Some(order[0]));
    }

    #[test]
    fn baseline_is_weight_descending_and_skips_reviewed() {
        let w = [1.0, 5.0, -2.0, 5.0, 3.0];
        assert_eq!(baseline_order(&w, &[]), vec![1, 3, 4, 0, 2]);
        assert_eq!(baseline_order(&w, &[false, true, false, false, true]), vec![3, 0, 2]);
    }

    #[test]
    fn out_of_range_levels_and_odd_strength_are_harmless() {
        let par = params();
        let m = ActiveModel::new(&par, f64::NAN);
        let g: Gamma = vec![Some(9), None, Some(7), Some(1)];
        let p = m.posterior(&g);
        assert!((p - par.p).abs() < 1e-12, "{p}");
        let mut z = ActiveModel::new(&par, 0.0);
        z.record(&g, true);
        assert!((0.0..=1.0).contains(&z.posterior(&g)));
    }

    fn gamma_strategy() -> impl Strategy<Value = Gamma> {
        (
            proptest::option::of(0u8..3),
            proptest::option::of(0u8..2),
            proptest::option::of(0u8..2),
        )
            .prop_map(|(a, b, c)| vec![a, b, c])
    }

    fn labels_strategy() -> impl Strategy<Value = Vec<(Gamma, bool)>> {
        proptest::collection::vec((gamma_strategy(), any::<bool>()), 0..40)
    }

    fn fitted(labels: &[(Gamma, bool)], s: f64) -> ActiveModel {
        let mut m = ActiveModel::new(&params(), s);
        for (g, y) in labels {
            m.record(g, *y);
        }
        m
    }

    #[test]
    fn stratified_probes_a_confident_but_wrong_pattern() {
        // Pattern `bad` has FS posterior ≈ 0 but 1 in 5 of it matches; `good` is all matches.
        let par = params();
        let bad: Gamma = vec![Some(2), Some(1), Some(1)];
        let good: Gamma = vec![Some(0), Some(0), Some(0)];
        assert!(ActiveModel::new(&par, 10.0).posterior(&bad) < 0.01);
        let items: Vec<Gamma> = (0..50)
            .map(|i| if i % 2 == 0 { bad.clone() } else { good.clone() })
            .collect();
        let m = PatternModel::new(&par, StratifiedConfig::default());
        let order = order_queue_stratified(&m, &items, &[], 3);
        assert_eq!(order.len(), items.len());
        // The first two probes per pattern come first, so the `bad` pattern is queried at once.
        assert!(
            order[..6].iter().filter(|&&i| items[i] == bad).count() >= 3,
            "{order:?}"
        );
        // Plain uncertainty sampling never reaches it while `good` items are left.
        let plain = order_queue(&ActiveModel::new(&par, 10.0), &items, &[], 3);
        assert!(plain[..25].iter().all(|&i| items[i] == good));
        // After probes with one match, the estimate moves off zero (the clamped prior is what allows it).
        let mut m = m;
        m.record(&bad, true);
        m.record(&bad, false);
        m.record(&bad, false);
        assert!(m.posterior(&bad) > 0.2, "{}", m.posterior(&bad));
        assert!(next_item_stratified(&m, &items, &[], 3).is_some());
    }

    proptest! {
        #[test]
        fn posterior_in_unit_interval(labels in labels_strategy(), g in gamma_strategy(), s in 0.01f64..1000.0) {
            let m = fitted(&labels, s);
            let p = m.posterior(&g);
            prop_assert!((0.0..=1.0).contains(&p), "{p}");
            prop_assert_eq!(m.suggest(&g).is_match, p >= 0.5);
        }

        #[test]
        fn ordering_is_deterministic_permutation_of_unreviewed(
            labels in labels_strategy(),
            items in proptest::collection::vec(gamma_strategy(), 0..60),
            mask in proptest::collection::vec(any::<bool>(), 0..60),
            seed in any::<u64>(),
        ) {
            let m = fitted(&labels, 10.0);
            let a = order_queue(&m, &items, &mask, seed);
            prop_assert_eq!(&a, &order_queue(&m.clone(), &items, &mask, seed));
            let mut sorted = a.clone();
            sorted.sort_unstable();
            let want: Vec<usize> = (0..items.len()).filter(|&i| !mask.get(i).copied().unwrap_or(false)).collect();
            prop_assert_eq!(sorted, want);
            prop_assert_eq!(next_item(&m, &items, &mask, seed), a.first().copied());
            let w: Vec<f64> = items.iter().map(|g| m.log_odds(g)).collect();
            let mut b = baseline_order(&w, &mask);
            b.sort_unstable();
            prop_assert_eq!(b, (0..items.len()).filter(|&i| !mask.get(i).copied().unwrap_or(false)).collect::<Vec<_>>());
        }

        #[test]
        fn recorded_decisions_are_never_altered(
            labels in labels_strategy(),
            more in labels_strategy(),
            items in proptest::collection::vec(gamma_strategy(), 0..30),
        ) {
            let mut m = fitted(&labels, 10.0);
            let before: Vec<Decision> = m.decisions().to_vec();
            prop_assert_eq!(before.len(), labels.len());
            for (d, (g, y)) in before.iter().zip(&labels) {
                prop_assert_eq!(&d.gamma, g);
                prop_assert_eq!(d.is_match, *y);
            }
            let _ = order_queue(&m, &items, &[], 1);
            let _ = next_item(&m, &items, &[], 1);
            for g in &items {
                let _ = m.suggest(g);
            }
            for (g, y) in &more {
                m.record(g, *y);
            }
            prop_assert_eq!(&m.decisions()[..before.len()], &before[..]);
            prop_assert_eq!(m.decisions().len(), labels.len() + more.len());
        }

        #[test]
        fn match_label_never_lowers_posterior(labels in labels_strategy(), g in gamma_strategy(), s in 0.01f64..1000.0) {
            let mut m = fitted(&labels, s);
            let before = m.posterior(&g);
            m.record(&g, true);
            prop_assert!(m.posterior(&g) >= before - 1e-12, "{before} -> {}", m.posterior(&g));
        }

        #[test]
        fn stratified_order_is_a_deterministic_permutation(
            labels in labels_strategy(),
            items in proptest::collection::vec(gamma_strategy(), 0..60),
            mask in proptest::collection::vec(any::<bool>(), 0..60),
            seed in any::<u64>(),
            s in 0.01f64..50.0,
            probes in 0u32..5,
        ) {
            let mut m = PatternModel::new(&params(), StratifiedConfig { prior_strength: s, probes, prior_floor: 0.05 });
            for (g, y) in &labels {
                m.record(g, *y);
            }
            let a = order_queue_stratified(&m, &items, &mask, seed);
            prop_assert_eq!(&a, &order_queue_stratified(&m.clone(), &items, &mask, seed));
            let mut sorted = a.clone();
            sorted.sort_unstable();
            let open: Vec<usize> = (0..items.len()).filter(|&i| !mask.get(i).copied().unwrap_or(false)).collect();
            prop_assert_eq!(sorted, open);
            prop_assert_eq!(a.first().copied(), next_item_stratified(&m, &items, &mask, seed));
            for g in &items {
                let p = m.posterior(g);
                prop_assert!((0.0..=1.0).contains(&p));
            }
            // Decisions are append-only and untouched by ordering or suggesting.
            prop_assert_eq!(m.decisions().len(), labels.len());
            for (d, (g, y)) in m.decisions().iter().zip(&labels) {
                prop_assert_eq!(&d.gamma, g);
                prop_assert_eq!(d.is_match, *y);
            }
        }
    }
}

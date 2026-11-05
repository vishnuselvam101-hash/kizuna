//! Selective invalidation (directive §8). Given one semantic change to evidence, find the decisions that depend on
//! it, decide materiality per dependency, and propagate only material impacts through decision→decision edges.
//! Nothing is invalidated by blanket downstream traversal; decisions are flagged, never re-decided.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::str::FromStr;

use conflict_engine::units::{QtyCmp, compare, parse_unit, to_base};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub mod cover;

pub const ENGINE_VERSION: &str = "dep-engine@1";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub text: Option<String>,
    pub num: Option<String>,
    pub unit: Option<String>,
    pub revision: Option<String>,
    #[serde(default)]
    pub scope: BTreeMap<String, String>,
}

impl Snapshot {
    fn display(&self) -> String {
        match (&self.num, &self.text, &self.unit) {
            (Some(n), _, Some(u)) => format!("{n} {u}"),
            (Some(n), _, None) => n.clone(),
            (None, Some(t), _) => t.clone(),
            _ => String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Materiality {
    /// Any change of value (unit-aware: 25.4 mm == 1 in) is material.
    AnyChange,
    /// Numeric changes up to `threshold` (in the snapshot's unit) are immaterial.
    NumericAbs { threshold: String },
    /// Changes are recorded but never flag the decision.
    Informational,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dep {
    pub decision: String,
    pub claim: Option<String>,
    pub on_decision: Option<String>,
    pub subject: String,
    pub predicate: Option<String>,
    pub applicability: BTreeMap<String, String>,
    pub materiality: Materiality,
    pub snapshot: Snapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Change {
    /// A claim was corrected by a newer claim.
    Superseded {
        old_claim: String,
        new_claim: String,
        new: Snapshot,
    },
    /// A claim is no longer usable evidence (e.g. rejected in review).
    Withdrawn { old_claim: String, reason: String },
    /// A new claim about a subject/predicate appeared (not a correction).
    NewEvidence {
        new_claim: String,
        subject: String,
        predicate: String,
        new: Snapshot,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Impact {
    pub decision: String,
    pub material: bool,
    pub cause_kind: &'static str,
    pub reason: String,
    pub path: Vec<String>,
    pub semantic_change: Value,
}

fn norm_text(s: &Option<String>) -> Option<String> {
    s.as_ref()
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase())
}

/// Returns (material, explanation).
pub fn materiality(m: &Materiality, old: &Snapshot, new: &Snapshot) -> (bool, String) {
    if *m == Materiality::Informational {
        return (false, "informational dependency".into());
    }
    let dec = |s: &Option<String>| s.as_deref().and_then(|v| Decimal::from_str(v).ok());
    match (dec(&old.num), dec(&new.num)) {
        (Some(a), Some(b)) => match compare(a, old.unit.as_deref(), b, new.unit.as_deref()) {
            QtyCmp::Equal => (false, "same quantity (unit-aware)".into()),
            QtyCmp::Different { a_base, b_base } => match m {
                Materiality::NumericAbs { threshold } => {
                    let t = Decimal::from_str(threshold).unwrap_or(Decimal::ZERO);
                    let t_base = match old.unit.as_deref().and_then(parse_unit) {
                        Some(u) => to_base(t, &u).unwrap_or(t),
                        None => t,
                    };
                    let diff = (a_base - b_base).abs();
                    if diff > t_base {
                        (true, format!("change exceeds threshold {threshold}"))
                    } else {
                        (false, format!("change within threshold {threshold}"))
                    }
                }
                _ => (true, "value changed".into()),
            },
            QtyCmp::Incompatible | QtyCmp::UnknownUnit => (true, "units no longer comparable".into()),
        },
        (None, None) if norm_text(&old.text) == norm_text(&new.text) => (false, "same value".into()),
        _ => (true, "value changed".into()),
    }
}

pub(crate) fn applicable(app: &BTreeMap<String, String>, new: &Snapshot) -> bool {
    app.iter().all(|(k, v)| {
        if k == "revision" {
            new.revision.as_ref().is_none_or(|r| r.eq_ignore_ascii_case(v))
        } else {
            new.scope.get(k).is_none_or(|x| x.eq_ignore_ascii_case(v))
        }
    })
}

/// Explicit result when an invalidation would touch more decisions than the caller allows: the work stops at the
/// bound (deterministically, at the same point every time) and the change must be handled as a review batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NeedsReviewBatch {
    pub limit: usize,
    /// Decisions already reached when the bound was crossed (always `limit + 1`).
    pub reached: usize,
}

impl fmt::Display for NeedsReviewBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalidation fan-out exceeds {} decisions: needs a review batch",
            self.limit
        )
    }
}

impl std::error::Error for NeedsReviewBatch {}

/// Unbounded [`assess_bounded`].
pub fn assess(deps: &[Dep], change: &Change) -> Vec<Impact> {
    assess_bounded(deps, change, usize::MAX).unwrap_or_else(|e| unreachable!("{e}"))
}

/// Unbounded [`upstream_withdrawn_bounded`].
pub fn upstream_withdrawn(deps: &[Dep], decision: &str, reason: &str) -> Vec<Impact> {
    upstream_withdrawn_bounded(deps, decision, reason, usize::MAX).unwrap_or_else(|e| unreachable!("{e}"))
}

/// Impacts of one evidence change, at most one per decision, sorted by decision id. Same results as
/// [`reference::assess`] (property-tested), in O(deps + affected paths) instead of O(affected x deps).
pub fn assess_bounded(deps: &[Dep], change: &Change, max_affected: usize) -> Result<Vec<Impact>, NeedsReviewBatch> {
    let mut run = Run::new(deps, max_affected);
    let mut memo: HashMap<MemoKey<'_>, Option<(bool, String)>> = HashMap::new();
    for d in deps {
        if let Some(i) = direct_impact(d, change, &mut memo) {
            run.seed(&d.decision, i)?;
        }
    }
    run.propagate()?;
    Ok(run.into_impacts())
}

/// Decisions that depend on a revoked/withdrawn decision (bounded like [`assess_bounded`]).
pub fn upstream_withdrawn_bounded(
    deps: &[Dep],
    decision: &str,
    reason: &str,
    max_affected: usize,
) -> Result<Vec<Impact>, NeedsReviewBatch> {
    let mut run = Run::new(deps, max_affected);
    run.index_children();
    let children = match run.ids.get(decision) {
        Some(&p) => run.children_of(p).to_vec(),
        None => Vec::new(),
    };
    for k in children {
        let d = &deps[k as usize];
        run.seed(
            &d.decision,
            Impact {
                decision: d.decision.clone(),
                material: d.materiality != Materiality::Informational,
                cause_kind: "upstream_decision",
                reason: format!("upstream decision {decision} {reason}"),
                path: vec![decision.to_string(), d.decision.clone()],
                semantic_change: json!({ "upstream_decision": decision }),
            },
        )?;
    }
    run.propagate()?;
    Ok(run.into_impacts())
}

/// FxHash-style hasher for the interning tables. Keys are server-generated decision ids (UUIDs), not attacker-chosen
/// strings, so the DoS resistance of SipHash buys nothing here while it is the largest single cost of indexing.
#[derive(Default, Clone, Copy)]
struct FxHasher(u64);

impl std::hash::Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut b = [0u8; 8];
            b[..chunk.len()].copy_from_slice(chunk);
            self.0 = (self.0.rotate_left(5) ^ u64::from_le_bytes(b)).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }

    fn finish(&self) -> u64 {
        // murmur3 fmix64: the multiply-rotate state is weak in the low bits hashbrown uses for bucket selection.
        let mut h = self.0;
        h = (h ^ (h >> 33)).wrapping_mul(0xff51_afd7_ed55_8ccd);
        h = (h ^ (h >> 33)).wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        h ^ (h >> 33)
    }
}

type FastMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<FxHasher>>;

type MemoKey<'a> = (&'a Materiality, &'a Snapshot, &'a BTreeMap<String, String>);

/// The direct impact of `change` on one dependency edge. The verdict (material, why) depends only on the edge's
/// (materiality, snapshot, applicability) and the change, so it is memoised per distinct key for this change:
/// the many edges that recorded the same claim value cost one evaluation.
fn direct_impact<'a>(
    d: &'a Dep,
    change: &Change,
    memo: &mut HashMap<MemoKey<'a>, Option<(bool, String)>>,
) -> Option<Impact> {
    let mut verdict = |new: &Snapshot, superseded: bool| {
        memo.entry((&d.materiality, &d.snapshot, &d.applicability))
            .or_insert_with(|| {
                if superseded {
                    // A correction that moves the claim to another revision or applicability scope is material
                    // regardless of the value (review finding M1).
                    let up = |r: &Option<String>| r.as_deref().map(str::to_uppercase);
                    let moved = up(&d.snapshot.revision) != up(&new.revision)
                        || d.snapshot.scope != new.scope
                        || !applicable(&d.applicability, new);
                    Some(if moved && d.materiality != Materiality::Informational {
                        (true, "applicability changed (revision or scope)".to_string())
                    } else {
                        materiality(&d.materiality, &d.snapshot, new)
                    })
                } else if applicable(&d.applicability, new) {
                    // Confirming evidence (same value) is not a change at all.
                    Some(materiality(&d.materiality, &d.snapshot, new))
                        .filter(|(_, why)| why != "same quantity (unit-aware)" && why != "same value")
                } else {
                    None
                }
            })
            .clone()
    };
    match change {
        Change::Superseded {
            old_claim,
            new_claim,
            new,
        } if d.claim.as_deref() == Some(old_claim) => {
            let (material, why) = verdict(new, true)?;
            Some(Impact {
                decision: d.decision.clone(),
                material,
                cause_kind: "superseded",
                reason: format!("evidence corrected: {why}"),
                path: vec![old_claim.clone(), new_claim.clone(), d.decision.clone()],
                semantic_change: json!({ "from": d.snapshot.display(), "to": new.display(), "predicate": d.predicate }),
            })
        }
        Change::Withdrawn { old_claim, reason } if d.claim.as_deref() == Some(old_claim) => Some(Impact {
            decision: d.decision.clone(),
            material: d.materiality != Materiality::Informational,
            cause_kind: "withdrawn",
            reason: format!("evidence withdrawn: {reason}"),
            path: vec![old_claim.clone(), d.decision.clone()],
            semantic_change: json!({ "from": d.snapshot.display(), "to": null, "predicate": d.predicate }),
        }),
        Change::NewEvidence {
            new_claim,
            subject,
            predicate,
            new,
        } if d.claim.is_some()
            && d.claim.as_deref() != Some(new_claim)
            && &d.subject == subject
            && d.predicate.as_deref() == Some(predicate) =>
        {
            let (material, why) = verdict(new, false)?;
            Some(Impact {
                decision: d.decision.clone(),
                material,
                cause_kind: "new_evidence",
                reason: format!("new applicable evidence disagrees: {why}"),
                path: vec![new_claim.clone(), d.decision.clone()],
                semantic_change: json!({ "from": d.snapshot.display(), "to": new.display(), "predicate": predicate }),
            })
        }
        _ => None,
    }
}

/// How a decision got its impact: a direct (seed) impact, or propagation from a material parent decision.
enum Origin {
    Seed(Impact),
    Via(u32),
}

struct Slot {
    material: bool,
    origin: Origin,
}

/// One invalidation run over interned decision ids. A decision's slot is its dirty state: empty, immaterial or
/// material. A material slot is final, so every decision is queued at most once (cycle-safe, O(edges) total).
struct Run<'a> {
    deps: &'a [Dep],
    ids: FastMap<&'a str, u32>,
    names: Vec<&'a str>,
    slots: Vec<Option<Slot>>,
    /// CSR reverse index over interned ids: edges `child_edges[child_start[p]..child_start[p + 1]]` point at
    /// decision `p`, in input order. Built once, on demand; ids interned later have no children.
    child_start: Vec<u32>,
    child_edges: Vec<u32>,
    limit: usize,
    reached: usize,
}

impl<'a> Run<'a> {
    fn new(deps: &'a [Dep], limit: usize) -> Self {
        Run {
            deps,
            ids: FastMap::default(),
            names: Vec::new(),
            slots: Vec::new(),
            child_start: Vec::new(),
            child_edges: Vec::new(),
            limit,
            reached: 0,
        }
    }

    fn index_children(&mut self) {
        if !self.child_start.is_empty() {
            return;
        }
        let deps = self.deps;
        let n = deps.iter().filter(|d| d.on_decision.is_some()).count();
        self.ids.reserve(n);
        let mut pairs: Vec<(u32, u32)> = Vec::with_capacity(n);
        for (k, d) in deps.iter().enumerate() {
            if let Some(on) = d.on_decision.as_deref() {
                pairs.push((self.id(on), k as u32));
            }
        }
        let mut start = vec![0u32; self.names.len() + 1];
        for &(p, _) in &pairs {
            start[p as usize + 1] += 1;
        }
        for i in 1..start.len() {
            start[i] += start[i - 1];
        }
        let mut fill = start.clone();
        let mut edges = vec![0u32; pairs.len()];
        for (p, k) in pairs {
            edges[fill[p as usize] as usize] = k;
            fill[p as usize] += 1;
        }
        (self.child_start, self.child_edges) = (start, edges);
    }

    fn children_of(&self, p: u32) -> &[u32] {
        match (self.child_start.get(p as usize), self.child_start.get(p as usize + 1)) {
            (Some(&a), Some(&b)) => &self.child_edges[a as usize..b as usize],
            _ => &[],
        }
    }

    fn id(&mut self, name: &'a str) -> u32 {
        *self.ids.entry(name).or_insert_with(|| {
            self.names.push(name);
            self.slots.push(None);
            (self.names.len() - 1) as u32
        })
    }

    /// Sets a slot unless it already holds a material impact or the new one is immaterial (material wins; the
    /// first impact wins among equals). Returns whether the slot changed.
    fn offer(&mut self, id: u32, material: bool, origin: Origin) -> Result<bool, NeedsReviewBatch> {
        let slot = &mut self.slots[id as usize];
        match slot {
            Some(prev) if prev.material || !material => return Ok(false),
            Some(_) => {}
            None => {
                self.reached += 1;
                if self.reached > self.limit {
                    return Err(NeedsReviewBatch {
                        limit: self.limit,
                        reached: self.reached,
                    });
                }
            }
        }
        *slot = Some(Slot { material, origin });
        Ok(true)
    }

    fn seed(&mut self, decision: &'a str, impact: Impact) -> Result<(), NeedsReviewBatch> {
        let id = self.id(decision);
        self.offer(id, impact.material, Origin::Seed(impact)).map(drop)
    }

    /// Breadth-first from the material seeds (in decision-id order), through decision→decision edges in input order.
    fn propagate(&mut self) -> Result<(), NeedsReviewBatch> {
        let mut start: Vec<u32> = (0..self.slots.len() as u32)
            .filter(|&i| self.slots[i as usize].as_ref().is_some_and(|s| s.material))
            .collect();
        if start.is_empty() {
            return Ok(());
        }
        start.sort_by_key(|&i| self.names[i as usize]);
        let mut queue: VecDeque<u32> = start.into();
        self.index_children();
        let (deps, child_start, child_edges) = (
            self.deps,
            std::mem::take(&mut self.child_start),
            std::mem::take(&mut self.child_edges),
        );
        while let Some(dec) = queue.pop_front() {
            let (a, b) = match (child_start.get(dec as usize), child_start.get(dec as usize + 1)) {
                (Some(&a), Some(&b)) => (a as usize, b as usize),
                _ => (0, 0),
            };
            for &k in &child_edges[a..b] {
                let d = &deps[k as usize];
                let material = d.materiality != Materiality::Informational;
                let t = self.id(&d.decision);
                if self.offer(t, material, Origin::Via(dec))? && material {
                    queue.push_back(t);
                }
            }
        }
        Ok(())
    }

    /// Explanation path of a decision: its seed's path extended by the propagation chain that reached it.
    fn path(&self, id: u32) -> Vec<String> {
        let mut chain = Vec::new();
        let mut cur = id;
        loop {
            match self.slots[cur as usize].as_ref().map(|s| &s.origin) {
                Some(Origin::Via(p)) => {
                    chain.push(self.names[cur as usize]);
                    cur = *p;
                }
                Some(Origin::Seed(i)) => {
                    let mut path = i.path.clone();
                    path.extend(chain.iter().rev().map(|s| s.to_string()));
                    return path;
                }
                None => unreachable!("propagation parents always hold a material impact"),
            }
        }
    }

    fn into_impacts(self) -> Vec<Impact> {
        let mut order: Vec<u32> = (0..self.slots.len() as u32)
            .filter(|&i| self.slots[i as usize].is_some())
            .collect();
        order.sort_by_key(|&i| self.names[i as usize]);
        order
            .into_iter()
            .map(
                |i| match self.slots[i as usize].as_ref().map(|s| (s.material, &s.origin)) {
                    Some((_, Origin::Seed(imp))) => imp.clone(),
                    Some((material, Origin::Via(p))) => {
                        let parent = self.names[*p as usize];
                        Impact {
                            decision: self.names[i as usize].to_string(),
                            material,
                            cause_kind: "upstream_decision",
                            reason: format!("depends on potentially affected decision {parent}"),
                            path: self.path(i),
                            semantic_change: json!({ "upstream_decision": parent }),
                        }
                    }
                    None => unreachable!("filtered to filled slots"),
                },
            )
            .collect()
    }
}

#[doc(hidden)]
pub mod reference;

#[cfg(test)]
mod tests;

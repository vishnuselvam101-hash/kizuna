//! Evidence debt ledger (I10, `mfg-debt@1`): for every accepted decision, the evidence weaknesses it rests on,
//! kept as separate components (never folded into one score) and evaluated "as known on" a transaction time `t`.
//!
//! The caller supplies, per decision, the time spans (transaction time, microseconds since the Unix epoch) during
//! which each fact held: when the decision was accepted, when each dependency claim was confirmed or rejected, and
//! when each linked signal, finding, conflict or 8D case was open. Everything here is pure: `components_at(d, t)`
//! only looks at spans and times `<= t`, so the debt of a past date never changes when later history is added.

use std::collections::BTreeSet;

use serde::Serialize;

pub const DEBT_VERSION: &str = "mfg-debt@1";
/// Claims below this extraction confidence that no person has confirmed count as low-confidence debt.
pub const LOW_CONFIDENCE_BPS: i32 = 8_000;
/// Evidence expiring within this many days (or already expired) counts as expiry debt.
pub const EXPIRY_WARN_DAYS: i64 = 30;
/// One day in the time unit used here (microseconds).
pub const DAY: i64 = 86_400_000_000;

/// A half-open transaction-time interval `[from, to)`; `to = None` means "still holds".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Span {
    pub from: i64,
    pub to: Option<i64>,
}

impl Span {
    pub fn open(from: i64) -> Self {
        Self { from, to: None }
    }
    pub fn at(&self, t: i64) -> bool {
        self.from <= t && self.to.is_none_or(|e| t < e)
    }
}

pub fn holds_at(spans: &[Span], t: i64) -> bool {
    spans.iter().any(|s| s.at(t))
}

/// Rebuilds the spans during which an item was open from its creation time and timestamped status events
/// (`true` = (re)opened, `false` = closed). Events before creation are clamped to it.
pub fn spans_from_events(created: i64, events: &[(i64, bool)]) -> Vec<Span> {
    let mut ev = Vec::with_capacity(events.len() + 1);
    ev.push((created, true));
    ev.extend(events.iter().map(|&(t, o)| (t.max(created), o)));
    spans_from_toggles(&ev)
}

/// Spans during which a state held, starting "not held", from timestamped on/off events applied in time order
/// (ties keep input order). Zero-length spans are dropped.
pub fn spans_from_toggles(events: &[(i64, bool)]) -> Vec<Span> {
    let mut ev = events.to_vec();
    ev.sort_by_key(|e| e.0);
    let mut out = Vec::new();
    let mut start: Option<i64> = None;
    for (t, on) in ev {
        match (start, on) {
            (Some(s), false) => {
                if t > s {
                    out.push(Span { from: s, to: Some(t) });
                }
                start = None;
            }
            (None, true) => start = Some(t),
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push(Span::open(s));
    }
    out
}

/// One claim a decision depends on.
#[derive(Clone, Debug)]
pub struct DepClaim<Id> {
    pub id: Id,
    pub predicate: String,
    pub confidence_bps: i32,
    /// When the claim was recorded.
    pub recorded: i64,
    /// When a person had confirmed (accepted) the claim.
    pub confirmed: Vec<Span>,
    /// When a reviewer's request for more evidence on the claim was pending (until the next approve or reject).
    pub requested: Vec<Span>,
    /// When the claim was rejected (rejection is final).
    pub rejected_from: Option<i64>,
    /// When a newer claim superseded this one (supersession is final).
    pub superseded_from: Option<i64>,
    /// Valid-time end of the evidence (claim `valid_to` or a certificate expiry date), if any.
    pub expiry: Option<i64>,
}

impl<Id> DepClaim<Id> {
    pub fn rejected_at(&self, t: i64) -> bool {
        self.rejected_from.is_some_and(|r| r <= t)
    }
    pub fn superseded_at(&self, t: i64) -> bool {
        self.superseded_from.is_some_and(|r| r <= t)
    }
    pub fn confirmed_at(&self, t: i64) -> bool {
        !self.rejected_at(t) && holds_at(&self.confirmed, t)
    }
    /// The claim's single debt class at `t` (`None`: confirmed and current). Precedence: rejected, superseded,
    /// re-requested, then unconfirmed by confidence.
    pub fn class_at(&self, t: i64) -> Option<ClaimDebt> {
        if self.rejected_at(t) {
            Some(ClaimDebt::Rejected)
        } else if self.superseded_at(t) {
            Some(ClaimDebt::Superseded)
        } else if self.confirmed_at(t) {
            None
        } else if holds_at(&self.requested, t) {
            Some(ClaimDebt::ReRequested)
        } else if self.confidence_bps < LOW_CONFIDENCE_BPS {
            Some(ClaimDebt::LowConfidence)
        } else {
            Some(ClaimDebt::Unconfirmed)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimDebt {
    Unconfirmed,
    LowConfidence,
    ReRequested,
    Rejected,
    Superseded,
}

/// A linked signal, finding, conflict or case and when it was open.
#[derive(Clone, Debug)]
pub struct Item<Id> {
    pub id: Id,
    pub open: Vec<Span>,
}

#[derive(Clone, Debug)]
pub struct DecisionFacts<Id> {
    pub kind: String,
    /// From the decision time until it was revoked or superseded.
    pub accepted: Span,
    pub deps: Vec<DepClaim<Id>>,
    /// Integrity signals (I3) and revision drift findings (I2) about the decision's evidence, part or supplier.
    pub integrity: Vec<Item<Id>>,
    /// Three-way match findings of the latest evaluation of the decision's PO line(s).
    pub match_findings: Vec<Item<Id>>,
    pub conflicts: Vec<Item<Id>>,
    pub capa: Vec<Item<Id>>,
    /// Supplier re-requests (resend drafts) covering the decision, open until cancelled.
    pub requests: Vec<Item<Id>>,
}

/// A required kind of evidence, satisfied by any dependency whose predicate is listed (an entry ending in `.` is
/// a prefix).
#[derive(Clone, Copy, Debug)]
pub struct Requirement {
    pub name: &'static str,
    pub any_of: &'static [&'static str],
}

impl Requirement {
    pub fn met_by(&self, predicate: &str) -> bool {
        self.any_of.iter().any(|p| {
            if p.ends_with('.') {
                predicate.starts_with(p)
            } else {
                predicate == *p
            }
        })
    }
}

const RELEASE: &[Requirement] = &[
    Requirement {
        name: "drawing_revision",
        any_of: &["mfg.revision"],
    },
    Requirement {
        name: "specified_material",
        any_of: &["mfg.material"],
    },
    Requirement {
        name: "certified_material",
        any_of: &["qa.material"],
    },
    Requirement {
        name: "inspection_result",
        any_of: &["qa.result", "qa.measured."],
    },
    Requirement {
        name: "supplier_identity",
        any_of: &["po.supplier", "qa.supplier", "mfg.supplier"],
    },
];

/// Evidence an accepted decision of this kind must rest on (`mfg-debt@1`). Release and acceptance decisions about
/// a part need the drawing revision, both material statements, an inspection result and the supplier's identity.
/// Other kinds have no fixed list yet, so they carry no missing-claim debt.
pub fn requirements(kind: &str) -> &'static [Requirement] {
    if kind.ends_with("_release") || kind.ends_with("_acceptance") || kind == "first_article" {
        RELEASE
    } else {
        &[]
    }
}

/// The debt of one decision at one transaction time. Lists are ids so every component links to its evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Components<Id> {
    pub missing_required: Vec<&'static str>,
    /// Unconfirmed claims at or above the confidence floor.
    pub unconfirmed: Vec<Id>,
    /// Unconfirmed claims below the confidence floor.
    pub low_confidence: Vec<Id>,
    /// Rejected claims the decision still cites.
    pub rejected: Vec<Id>,
    /// Cited claims a newer claim has superseded.
    pub superseded: Vec<Id>,
    /// Pending re-requests: cited claims a reviewer asked more evidence for, and supplier re-requests covering
    /// the decision.
    pub re_requested: Vec<Id>,
    pub integrity: Vec<Id>,
    pub match_findings: Vec<Id>,
    /// Whole days from `t` to the nearest evidence expiry (negative: already expired).
    pub expiry_days: Option<i64>,
    pub conflicts: Vec<Id>,
    pub capa: Vec<Id>,
}

impl<Id> Components<Id> {
    pub fn expiring(&self) -> bool {
        self.expiry_days.is_some_and(|d| d <= EXPIRY_WARN_DAYS)
    }
    pub fn expired(&self) -> bool {
        self.expiry_days.is_some_and(|d| d < 0)
    }
    pub fn is_clear(&self) -> bool {
        self.missing_required.is_empty()
            && self.unconfirmed.is_empty()
            && self.low_confidence.is_empty()
            && self.rejected.is_empty()
            && self.superseded.is_empty()
            && self.re_requested.is_empty()
            && self.integrity.is_empty()
            && self.match_findings.is_empty()
            && self.conflicts.is_empty()
            && self.capa.is_empty()
            && !self.expiring()
    }
}

fn open_at<Id: Copy>(items: &[Item<Id>], t: i64) -> Vec<Id> {
    items.iter().filter(|i| holds_at(&i.open, t)).map(|i| i.id).collect()
}

/// Debt of `d` as known at `t`, or `None` when the decision was not an accepted decision at `t`.
pub fn components_at<Id: Copy>(d: &DecisionFacts<Id>, t: i64) -> Option<Components<Id>> {
    if !d.accepted.at(t) {
        return None;
    }
    let usable = |c: &&DepClaim<Id>| !c.rejected_at(t);
    let missing_required = requirements(&d.kind)
        .iter()
        .filter(|r| !d.deps.iter().filter(usable).any(|c| r.met_by(&c.predicate)))
        .map(|r| r.name)
        .collect();
    let (mut unconfirmed, mut low_confidence, mut rejected) = (Vec::new(), Vec::new(), Vec::new());
    let (mut superseded, mut re_requested) = (Vec::new(), Vec::new());
    for c in &d.deps {
        match c.class_at(t) {
            Some(ClaimDebt::Rejected) => rejected.push(c.id),
            Some(ClaimDebt::Superseded) => superseded.push(c.id),
            Some(ClaimDebt::ReRequested) => re_requested.push(c.id),
            Some(ClaimDebt::LowConfidence) => low_confidence.push(c.id),
            Some(ClaimDebt::Unconfirmed) => unconfirmed.push(c.id),
            None => {}
        }
    }
    re_requested.extend(open_at(&d.requests, t));
    let expiry_days = d
        .deps
        .iter()
        .filter_map(|c| c.expiry)
        .min()
        .map(|e| (e - t).div_euclid(DAY));
    Some(Components {
        missing_required,
        unconfirmed,
        low_confidence,
        rejected,
        superseded,
        re_requested,
        integrity: open_at(&d.integrity, t),
        match_findings: open_at(&d.match_findings, t),
        expiry_days,
        conflicts: open_at(&d.conflicts, t),
        capa: open_at(&d.capa, t),
    })
}

/// Component totals of a group (part, supplier, whole tenant). Linked items are counted once per group even when
/// several decisions share them; missing evidence is counted per (decision, requirement).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub decisions: u32,
    pub with_debt: u32,
    pub missing_required: u32,
    pub unconfirmed: u32,
    pub low_confidence: u32,
    pub rejected: u32,
    pub superseded: u32,
    pub re_requested: u32,
    pub integrity: u32,
    pub match_findings: u32,
    pub conflicts: u32,
    pub capa: u32,
    /// Decisions whose nearest expiry is within `EXPIRY_WARN_DAYS` or past.
    pub expiring: u32,
    /// Decisions resting on evidence that has already expired.
    pub expired: u32,
    pub nearest_expiry_days: Option<i64>,
}

/// Accumulates decisions' components into group totals.
#[derive(Clone, Debug)]
pub struct Ledger<Id: Ord> {
    decisions: u32,
    with_debt: u32,
    missing: u32,
    expiring: u32,
    expired: u32,
    nearest: Option<i64>,
    unconfirmed: BTreeSet<Id>,
    low_confidence: BTreeSet<Id>,
    rejected: BTreeSet<Id>,
    superseded: BTreeSet<Id>,
    re_requested: BTreeSet<Id>,
    integrity: BTreeSet<Id>,
    match_findings: BTreeSet<Id>,
    conflicts: BTreeSet<Id>,
    capa: BTreeSet<Id>,
}

impl<Id: Ord> Default for Ledger<Id> {
    fn default() -> Self {
        Self {
            decisions: 0,
            with_debt: 0,
            missing: 0,
            expiring: 0,
            expired: 0,
            nearest: None,
            unconfirmed: BTreeSet::new(),
            low_confidence: BTreeSet::new(),
            rejected: BTreeSet::new(),
            superseded: BTreeSet::new(),
            re_requested: BTreeSet::new(),
            integrity: BTreeSet::new(),
            match_findings: BTreeSet::new(),
            conflicts: BTreeSet::new(),
            capa: BTreeSet::new(),
        }
    }
}

fn n(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

impl<Id: Ord + Copy> Ledger<Id> {
    pub fn add(&mut self, c: &Components<Id>) {
        self.decisions += 1;
        self.with_debt += u32::from(!c.is_clear());
        self.missing += n(c.missing_required.len());
        self.expiring += u32::from(c.expiring());
        self.expired += u32::from(c.expired());
        if let Some(d) = c.expiry_days {
            self.nearest = Some(self.nearest.map_or(d, |x| x.min(d)));
        }
        self.unconfirmed.extend(c.unconfirmed.iter().copied());
        self.low_confidence.extend(c.low_confidence.iter().copied());
        self.rejected.extend(c.rejected.iter().copied());
        self.superseded.extend(c.superseded.iter().copied());
        self.re_requested.extend(c.re_requested.iter().copied());
        self.integrity.extend(c.integrity.iter().copied());
        self.match_findings.extend(c.match_findings.iter().copied());
        self.conflicts.extend(c.conflicts.iter().copied());
        self.capa.extend(c.capa.iter().copied());
    }

    pub fn totals(&self) -> Totals {
        Totals {
            decisions: self.decisions,
            with_debt: self.with_debt,
            missing_required: self.missing,
            unconfirmed: n(self.unconfirmed.len()),
            low_confidence: n(self.low_confidence.len()),
            rejected: n(self.rejected.len()),
            superseded: n(self.superseded.len()),
            re_requested: n(self.re_requested.len()),
            integrity: n(self.integrity.len()),
            match_findings: n(self.match_findings.len()),
            conflicts: n(self.conflicts.len()),
            capa: n(self.capa.len()),
            expiring: self.expiring,
            expired: self.expired,
            nearest_expiry_days: self.nearest,
        }
    }
}

/// What a single debt entry points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "type", content = "ref", rename_all = "snake_case")]
pub enum Target<Id> {
    Claim(Id),
    /// A linked signal, finding, conflict, case or supplier re-request.
    Item(Id),
    /// A required kind of evidence the decision has no usable claim for.
    Requirement(&'static str),
}

/// One unit of evidence debt of one decision, with when it began (for its age). The `kind` is the component name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Debt<Id> {
    pub kind: &'static str,
    pub target: Target<Id>,
    /// Transaction time the debt began (for `expiring`/`expired`: when the evidence entered the warning window /
    /// expired, both valid-time facts recorded with the claim).
    pub since: i64,
}

/// Start of the span in `spans` holding at `t`.
fn open_since(spans: &[Span], t: i64) -> Option<i64> {
    spans.iter().filter(|s| s.at(t)).map(|s| s.from).max()
}

/// When `c` (unconfirmed at `t`) last stopped being confirmed, or was recorded.
fn unconfirmed_since<Id>(c: &DepClaim<Id>, t: i64) -> i64 {
    c.confirmed
        .iter()
        .filter_map(|s| s.to.filter(|e| *e <= t))
        .chain(c.requested.iter().filter_map(|s| s.to.filter(|e| *e <= t)))
        .max()
        .unwrap_or(c.recorded)
        .max(c.recorded)
}

/// Every debt entry of `d` as known at `t` (empty when the decision was not accepted at `t`). The entries are the
/// same facts as `components_at`, one per item, each with the time it began; "age" is `t - since`.
pub fn debts_at<Id: Copy>(d: &DecisionFacts<Id>, t: i64) -> Vec<Debt<Id>> {
    let Some(comp) = components_at(d, t) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for name in &comp.missing_required {
        // Missing since acceptance, or since the last matching claim was rejected.
        let lost = requirements(&d.kind)
            .iter()
            .find(|r| r.name == *name)
            .map(|r| {
                d.deps
                    .iter()
                    .filter(|c| r.met_by(&c.predicate))
                    .filter_map(|c| c.rejected_from.filter(|x| *x <= t))
                    .max()
            })
            .unwrap_or_default();
        out.push(Debt {
            kind: "missing_required",
            target: Target::Requirement(name),
            since: lost.unwrap_or(d.accepted.from).max(d.accepted.from),
        });
    }
    for c in &d.deps {
        let Some(class) = c.class_at(t) else { continue };
        let (kind, since) = match class {
            ClaimDebt::Rejected => ("rejected", c.rejected_from.unwrap_or(c.recorded)),
            ClaimDebt::Superseded => ("superseded", c.superseded_from.unwrap_or(c.recorded)),
            ClaimDebt::ReRequested => ("re_requested", open_since(&c.requested, t).unwrap_or(c.recorded)),
            ClaimDebt::LowConfidence => ("low_confidence", unconfirmed_since(c, t)),
            ClaimDebt::Unconfirmed => ("unconfirmed", unconfirmed_since(c, t)),
        };
        out.push(Debt {
            kind,
            target: Target::Claim(c.id),
            since,
        });
    }
    for c in &d.deps {
        if let Some(e) = c.expiry {
            if e < t {
                out.push(Debt {
                    kind: "expired",
                    target: Target::Claim(c.id),
                    since: e,
                });
            } else if (e - t).div_euclid(DAY) <= EXPIRY_WARN_DAYS {
                out.push(Debt {
                    kind: "expiring",
                    target: Target::Claim(c.id),
                    since: e - (EXPIRY_WARN_DAYS + 1) * DAY + 1,
                });
            }
        }
    }
    for (kind, items) in [
        ("integrity", &d.integrity),
        ("match_findings", &d.match_findings),
        ("conflicts", &d.conflicts),
        ("capa", &d.capa),
        ("re_requested", &d.requests),
    ] {
        for i in items {
            if let Some(since) = open_since(&i.open, t) {
                out.push(Debt {
                    kind,
                    target: Target::Item(i.id),
                    since,
                });
            }
        }
    }
    out
}

/// `count` sample times ending at `end`, `step` apart, oldest first.
pub fn trend_points(end: i64, step: i64, count: usize) -> Vec<i64> {
    let count = i64::try_from(count).unwrap_or(0);
    (0..count).rev().map(|k| end - k * step).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const D: i64 = DAY;

    fn claim(id: u32, pred: &str, conf: i32) -> DepClaim<u32> {
        DepClaim {
            id,
            predicate: pred.into(),
            confidence_bps: conf,
            recorded: 0,
            confirmed: vec![],
            requested: vec![],
            rejected_from: None,
            superseded_from: None,
            expiry: None,
        }
    }

    fn release(deps: Vec<DepClaim<u32>>) -> DecisionFacts<u32> {
        DecisionFacts {
            kind: "part_release".into(),
            accepted: Span { from: 10 * D, to: None },
            deps,
            integrity: vec![],
            match_findings: vec![],
            conflicts: vec![],
            capa: vec![],
            requests: vec![],
        }
    }

    fn full_deps() -> Vec<DepClaim<u32>> {
        [
            "mfg.revision",
            "mfg.material",
            "qa.material",
            "qa.measured.length",
            "po.supplier",
        ]
        .iter()
        .enumerate()
        .map(|(i, p)| DepClaim {
            confirmed: vec![Span::open(5 * D)],
            ..claim(i as u32 + 1, p, 9_500)
        })
        .collect()
    }

    #[test]
    fn complete_confirmed_release_is_clear() {
        let c = components_at(&release(full_deps()), 20 * D).unwrap();
        assert!(c.is_clear(), "{c:?}");
    }

    #[test]
    fn missing_required_names_each_gap() {
        let mut deps = full_deps();
        deps.retain(|c| c.predicate != "qa.material" && c.predicate != "po.supplier");
        let c = components_at(&release(deps), 20 * D).unwrap();
        assert_eq!(c.missing_required, vec!["certified_material", "supplier_identity"]);
        assert!(
            components_at(
                &DecisionFacts {
                    kind: "control_plan".into(),
                    ..release(vec![])
                },
                20 * D
            )
            .unwrap()
            .missing_required
            .is_empty()
        );
    }

    #[test]
    fn prefix_requirement_matches_measurements() {
        let r = RELEASE[3];
        assert!(r.met_by("qa.measured.outer_diameter") && r.met_by("qa.result") && !r.met_by("qa.material"));
    }

    #[test]
    fn unconfirmed_and_low_confidence_are_disjoint() {
        let mut deps = full_deps();
        deps[0].confirmed.clear();
        deps[1].confirmed.clear();
        deps[1].confidence_bps = 6_000;
        // Confirmed low-confidence claims are not debt: a person checked them.
        deps[2].confidence_bps = 5_000;
        let c = components_at(&release(deps), 20 * D).unwrap();
        assert_eq!((c.unconfirmed, c.low_confidence), (vec![1], vec![2]));
    }

    #[test]
    fn rejected_claim_is_debt_and_does_not_satisfy_requirement() {
        let mut deps = full_deps();
        deps[2].rejected_from = Some(15 * D);
        let before = components_at(&release(deps.clone()), 14 * D).unwrap();
        assert!(before.is_clear());
        let after = components_at(&release(deps), 15 * D).unwrap();
        assert_eq!(after.rejected, vec![3]);
        assert_eq!(after.missing_required, vec!["certified_material"]);
    }

    #[test]
    fn expiry_days_floor_and_negative() {
        let mut deps = full_deps();
        deps[0].expiry = Some(40 * D);
        deps[1].expiry = Some(50 * D);
        let d = release(deps);
        assert_eq!(components_at(&d, 20 * D).unwrap().expiry_days, Some(20));
        assert!(components_at(&d, 20 * D).unwrap().expiring());
        assert_eq!(components_at(&d, 40 * D - 1).unwrap().expiry_days, Some(0));
        assert_eq!(components_at(&d, 40 * D + 1).unwrap().expiry_days, Some(-1));
        let mut far = full_deps();
        far[0].expiry = Some(100 * D);
        assert!(!components_at(&release(far), 20 * D).unwrap().expiring());
    }

    #[test]
    fn not_accepted_outside_span() {
        let mut d = release(full_deps());
        d.accepted.to = Some(30 * D);
        assert!(components_at(&d, 9 * D).is_none());
        assert!(components_at(&d, 29 * D).is_some());
        assert!(components_at(&d, 30 * D).is_none());
    }

    #[test]
    fn items_count_only_while_open() {
        let mut d = release(full_deps());
        d.conflicts = vec![Item {
            id: 9,
            open: spans_from_events(12 * D, &[(14 * D, false), (16 * D, true)]),
        }];
        d.capa = vec![Item {
            id: 8,
            open: vec![Span {
                from: 11 * D,
                to: Some(13 * D),
            }],
        }];
        let at = |t| components_at(&d, t).unwrap();
        assert_eq!((at(12 * D).conflicts, at(12 * D).capa), (vec![9], vec![8]));
        assert!(at(15 * D).conflicts.is_empty() && at(15 * D).capa.is_empty());
        assert_eq!(at(17 * D).conflicts, vec![9]);
    }

    #[test]
    fn spans_from_events_handles_reopen_and_clamp() {
        assert_eq!(spans_from_events(10, &[]), vec![Span::open(10)]);
        assert_eq!(
            spans_from_events(10, &[(30, true), (20, false), (5, false)]),
            vec![Span { from: 30, to: None }]
        );
        assert_eq!(
            spans_from_events(10, &[(20, false), (20, false), (30, true), (40, false)]),
            vec![Span { from: 10, to: Some(20) }, Span { from: 30, to: Some(40) }]
        );
    }

    #[test]
    fn ledger_counts_shared_items_once() {
        let mut a = release(full_deps());
        a.integrity = vec![Item {
            id: 100,
            open: vec![Span::open(0)],
        }];
        let mut b = a.clone();
        b.deps.retain(|c| c.predicate != "mfg.material");
        let mut l = Ledger::default();
        for d in [&a, &b] {
            l.add(&components_at(d, 20 * D).unwrap());
        }
        let t = l.totals();
        assert_eq!(
            (t.decisions, t.with_debt, t.integrity, t.missing_required),
            (2, 2, 1, 1)
        );
    }

    #[test]
    fn trend_points_end_at_as_of() {
        assert_eq!(trend_points(100, 10, 3), vec![80, 90, 100]);
        assert!(trend_points(100, 10, 0).is_empty());
    }

    #[test]
    fn superseded_and_re_requested_classes() {
        let mut deps = full_deps();
        deps[0].superseded_from = Some(15 * D);
        deps[1].confirmed = vec![Span {
            from: 5 * D,
            to: Some(12 * D),
        }];
        deps[1].requested = vec![Span::open(12 * D)];
        let mut d = release(deps);
        d.requests = vec![Item {
            id: 77,
            open: vec![Span {
                from: 13 * D,
                to: Some(18 * D),
            }],
        }];
        let at = |t| components_at(&d, t).unwrap();
        assert_eq!((at(14 * D).superseded, at(14 * D).re_requested), (vec![], vec![2, 77]));
        // Superseded evidence is debt but still counts as present (not also missing).
        assert_eq!((at(16 * D).superseded, at(16 * D).missing_required), (vec![1], vec![]));
        assert_eq!(at(20 * D).re_requested, vec![2]);
        let t = {
            let mut l = Ledger::default();
            l.add(&at(16 * D));
            l.totals()
        };
        assert_eq!((t.superseded, t.re_requested, t.with_debt), (1, 2, 1));
    }

    #[test]
    fn debts_carry_since_for_age() {
        let mut deps = full_deps();
        deps[0].superseded_from = Some(15 * D);
        deps[1].confirmed = vec![Span {
            from: 5 * D,
            to: Some(12 * D),
        }];
        deps[1].requested = vec![Span::open(12 * D)];
        deps[2].confirmed.clear();
        deps[2].recorded = 3 * D;
        deps[3].expiry = Some(18 * D);
        deps[4].rejected_from = Some(16 * D);
        let mut d = release(deps);
        d.conflicts = vec![Item {
            id: 9,
            open: spans_from_events(12 * D, &[(13 * D, false), (14 * D, true)]),
        }];
        let mut got: Vec<(&str, Target<u32>, i64)> = debts_at(&d, 20 * D)
            .into_iter()
            .map(|x| (x.kind, x.target, x.since / D))
            .collect();
        got.sort();
        let mut want = vec![
            ("missing_required", Target::Requirement("supplier_identity"), 16),
            ("superseded", Target::Claim(1), 15),
            ("re_requested", Target::Claim(2), 12),
            ("unconfirmed", Target::Claim(3), 3),
            ("expired", Target::Claim(4), 18),
            ("rejected", Target::Claim(5), 16),
            ("conflicts", Target::Item(9), 14),
        ];
        want.sort();
        assert_eq!(got, want);
        assert!(debts_at(&d, 9 * D).is_empty(), "not accepted yet");
        // Expiring: within the warning window, dated from when it entered it.
        let mut far = full_deps();
        far[0].expiry = Some(40 * D);
        let e = debts_at(&release(far), 20 * D);
        assert_eq!(e.len(), 1);
        assert_eq!((e[0].kind, e[0].since), ("expiring", 40 * D - 31 * D + 1));
    }

    #[test]
    fn re_confirmation_after_request_clears_and_resets_age() {
        let mut deps = full_deps();
        deps[0].confirmed = vec![Span {
            from: 5 * D,
            to: Some(12 * D),
        }];
        deps[0].requested = vec![Span {
            from: 12 * D,
            to: Some(14 * D),
        }];
        let d = release(deps);
        // After the request closed without approval the claim is plain unconfirmed, aged from the close.
        let x = debts_at(&d, 20 * D);
        assert_eq!((x[0].kind, x[0].since), ("unconfirmed", 14 * D));
    }

    fn arb_spans() -> impl Strategy<Value = (i64, Vec<(i64, bool)>)> {
        (0i64..50, prop::collection::vec((0i64..100, any::<bool>()), 0..6))
    }

    fn arb_decision() -> impl Strategy<Value = DecisionFacts<u32>> {
        let preds = prop::sample::select(vec![
            "mfg.revision",
            "mfg.material",
            "qa.material",
            "qa.result",
            "po.supplier",
            "x.other",
        ]);
        let dep = (
            0u32..1000,
            preds,
            0i32..=10_000,
            arb_spans(),
            prop::option::of(0i64..100),
            prop::option::of(0i64..200),
        )
            .prop_map(|(id, p, conf, (c0, ev), rej, exp)| DepClaim {
                id,
                predicate: p.to_string(),
                confidence_bps: conf,
                recorded: 0,
                confirmed: spans_from_events(c0, &ev),
                requested: vec![],
                rejected_from: rej,
                superseded_from: None,
                expiry: exp,
            });
        let item = || {
            (0u32..1000, arb_spans()).prop_map(|(id, (c, ev))| Item {
                id,
                open: spans_from_events(c, &ev),
            })
        };
        (
            0i64..50,
            prop::option::of(50i64..120),
            prop::collection::vec(dep, 0..6),
            prop::collection::vec(item(), 0..4),
            prop::collection::vec(item(), 0..4),
            prop::collection::vec((arb_spans(), prop::option::of(0i64..120)), 6),
            prop::collection::vec(item(), 0..3),
        )
            .prop_map(|(from, to, mut deps, integrity, conflicts, extra, requests)| {
                for (c, ((r0, rev), sup)) in deps.iter_mut().zip(extra) {
                    c.requested = spans_from_events(r0, &rev);
                    c.superseded_from = sup;
                }
                DecisionFacts {
                    kind: "part_release".into(),
                    accepted: Span { from, to },
                    deps,
                    integrity,
                    match_findings: vec![],
                    conflicts,
                    capa: vec![],
                    requests,
                }
            })
    }

    /// Cuts history at `t`: drops every fact recorded after `t` (what a reader at `t` could have known).
    fn known_at(d: &DecisionFacts<u32>, t: i64) -> DecisionFacts<u32> {
        let cut = |s: &[Span]| -> Vec<Span> {
            s.iter()
                .filter(|x| x.from <= t)
                .map(|x| Span {
                    from: x.from,
                    to: x.to.filter(|e| *e <= t),
                })
                .collect()
        };
        let items = |v: &[Item<u32>]| {
            v.iter()
                .map(|i| Item {
                    id: i.id,
                    open: cut(&i.open),
                })
                .collect()
        };
        DecisionFacts {
            kind: d.kind.clone(),
            accepted: Span {
                from: d.accepted.from,
                to: d.accepted.to.filter(|e| *e <= t),
            },
            deps: d
                .deps
                .iter()
                .map(|c| DepClaim {
                    confirmed: cut(&c.confirmed),
                    requested: cut(&c.requested),
                    rejected_from: c.rejected_from.filter(|r| *r <= t),
                    superseded_from: c.superseded_from.filter(|r| *r <= t),
                    ..c.clone()
                })
                .collect(),
            integrity: items(&d.integrity),
            match_findings: items(&d.match_findings),
            conflicts: items(&d.conflicts),
            capa: items(&d.capa),
            requests: items(&d.requests),
        }
    }

    proptest! {
        /// Bitemporal correctness: the debt "as known at t" is the same whether or not history after t exists.
        #[test]
        fn as_of_ignores_later_history(d in arb_decision(), t in 0i64..130) {
            prop_assert_eq!(components_at(&d, t), components_at(&known_at(&d, t), t));
            prop_assert_eq!(debts_at(&d, t), debts_at(&known_at(&d, t), t));
        }

        /// Every debt entry began no later than `t`, and an accepted decision has entries iff it is not clear.
        #[test]
        fn debts_match_components(d in arb_decision(), t in 0i64..130) {
            let debts = debts_at(&d, t);
            prop_assert!(debts.iter().all(|x| x.since <= t), "{:?}", debts);
            match components_at(&d, t) {
                None => prop_assert!(debts.is_empty()),
                Some(c) => {
                    prop_assert_eq!(c.is_clear(), debts.is_empty());
                    let count = |k: &str| debts.iter().filter(|x| x.kind == k).count();
                    prop_assert_eq!(count("missing_required"), c.missing_required.len());
                    prop_assert_eq!(count("superseded"), c.superseded.len());
                    prop_assert_eq!(count("re_requested"), c.re_requested.len());
                    prop_assert_eq!(count("rejected"), c.rejected.len());
                    prop_assert_eq!(count("expired") > 0, c.expired());
                }
            }
        }
    }
}

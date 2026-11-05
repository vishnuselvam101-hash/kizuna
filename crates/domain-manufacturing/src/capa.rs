//! Corrective-action cases (8D / 是正処置): the case state machine and the evidence requirements of each discipline.
//! Pure: no I/O. The API resolves cited claims/documents into [`Cited`] facts and asks [`missing`] what a step still
//! lacks; transitions go through [`Case::apply`], which keeps the invariants below for every event sequence:
//!
//! - completed disciplines always form a prefix D1..Dk (a step needs every earlier step);
//! - `Open` ⇒ D8 not done and no closure request; `ClosurePending` ⇒ D1–D7 done, D8 not done, requester recorded;
//!   `Closed` ⇒ all eight done;
//! - closure is approved only by someone other than the requester (four-eyes);
//! - evidence contradicting the effectiveness check (D6) reopens D6–D8, also on a closed case.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub const CAPA_VERSION: &str = "kizuna-8d@1";

/// D1..D8. Stored as 1..=8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Discipline(u8);

impl Discipline {
    pub const ALL: [Discipline; 8] = [
        Discipline(1),
        Discipline(2),
        Discipline(3),
        Discipline(4),
        Discipline(5),
        Discipline(6),
        Discipline(7),
        Discipline(8),
    ];
    pub const TEAM: Discipline = Discipline(1);
    pub const PROBLEM: Discipline = Discipline(2);
    pub const CONTAINMENT: Discipline = Discipline(3);
    pub const ROOT_CAUSE: Discipline = Discipline(4);
    pub const ACTIONS: Discipline = Discipline(5);
    pub const VERIFICATION: Discipline = Discipline(6);
    pub const PREVENTION: Discipline = Discipline(7);
    pub const CLOSURE: Discipline = Discipline(8);

    pub fn new(n: u8) -> Option<Self> {
        (1..=8).contains(&n).then_some(Discipline(n))
    }
    pub fn number(self) -> u8 {
        self.0
    }
    fn idx(self) -> usize {
        usize::from(self.0 - 1)
    }
    pub fn code(self) -> &'static str {
        [
            "team",
            "problem",
            "containment",
            "root_cause",
            "actions",
            "verification",
            "prevention",
            "closure",
        ][self.idx()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseStatus {
    Open,
    ClosurePending,
    Closed,
}

impl CaseStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CaseStatus::Open => "open",
            CaseStatus::ClosurePending => "closure_pending",
            CaseStatus::Closed => "closed",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(CaseStatus::Open),
            "closure_pending" => Some(CaseStatus::ClosurePending),
            "closed" => Some(CaseStatus::Closed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case<U> {
    pub status: CaseStatus,
    /// `done[i]` = discipline i+1 completed.
    pub done: [bool; 8],
    pub closure_requested_by: Option<U>,
    pub reopened: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event<U> {
    Complete(Discipline),
    ReopenStep(Discipline),
    RequestClosure {
        by: U,
    },
    ApproveClosure {
        by: U,
    },
    RejectClosure,
    /// Later evidence contradicts the effectiveness check.
    Contradicted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionError {
    CaseNotOpen,
    NotPending,
    AlreadyDone,
    NotDone,
    EarlierStepOpen,
    /// D8 is completed by an approved closure, not directly.
    UseClosure,
    OwnClosure,
}

impl TransitionError {
    pub fn code(self) -> &'static str {
        match self {
            TransitionError::CaseNotOpen => "case_not_open",
            TransitionError::NotPending => "closure_not_pending",
            TransitionError::AlreadyDone => "step_already_done",
            TransitionError::NotDone => "step_not_done",
            TransitionError::EarlierStepOpen => "earlier_step_open",
            TransitionError::UseClosure => "use_closure",
            TransitionError::OwnClosure => "own_closure_request",
        }
    }
}

impl<U: Copy + Eq> Case<U> {
    pub fn new() -> Self {
        Case {
            status: CaseStatus::Open,
            done: [false; 8],
            closure_requested_by: None,
            reopened: 0,
        }
    }

    pub fn is_done(&self, d: Discipline) -> bool {
        self.done[d.idx()]
    }

    /// The next transition, or why it is refused. Never mutates on error.
    pub fn apply(&self, e: &Event<U>) -> Result<Case<U>, TransitionError> {
        let mut c = self.clone();
        match *e {
            Event::Complete(d) => {
                if c.status != CaseStatus::Open {
                    return Err(TransitionError::CaseNotOpen);
                }
                if d == Discipline::CLOSURE {
                    return Err(TransitionError::UseClosure);
                }
                if c.done[d.idx()] {
                    return Err(TransitionError::AlreadyDone);
                }
                if c.done[..d.idx()].iter().any(|x| !x) {
                    return Err(TransitionError::EarlierStepOpen);
                }
                c.done[d.idx()] = true;
            }
            Event::ReopenStep(d) => {
                if c.status != CaseStatus::Open {
                    return Err(TransitionError::CaseNotOpen);
                }
                if !c.done[d.idx()] {
                    return Err(TransitionError::NotDone);
                }
                c.done[d.idx()..].iter_mut().for_each(|x| *x = false);
            }
            Event::RequestClosure { by } => {
                if c.status != CaseStatus::Open {
                    return Err(TransitionError::CaseNotOpen);
                }
                if c.done[..7].iter().any(|x| !x) {
                    return Err(TransitionError::EarlierStepOpen);
                }
                c.status = CaseStatus::ClosurePending;
                c.closure_requested_by = Some(by);
            }
            Event::ApproveClosure { by } => {
                if c.status != CaseStatus::ClosurePending {
                    return Err(TransitionError::NotPending);
                }
                if c.closure_requested_by == Some(by) {
                    return Err(TransitionError::OwnClosure);
                }
                c.status = CaseStatus::Closed;
                c.done[7] = true;
            }
            Event::RejectClosure => {
                if c.status != CaseStatus::ClosurePending {
                    return Err(TransitionError::NotPending);
                }
                c.status = CaseStatus::Open;
                c.closure_requested_by = None;
            }
            Event::Contradicted => {
                if c.status == CaseStatus::Closed {
                    c.reopened += 1;
                }
                c.status = CaseStatus::Open;
                c.closure_requested_by = None;
                c.done[5..].iter_mut().for_each(|x| *x = false);
            }
        }
        Ok(c)
    }

    /// Whether the invariants in the module docs hold.
    pub fn invariants_hold(&self) -> bool {
        let prefix = self.done.windows(2).all(|w| w[0] || !w[1]);
        let shape = match self.status {
            CaseStatus::Open => !self.done[7] && self.closure_requested_by.is_none(),
            CaseStatus::ClosurePending => {
                self.done[..7].iter().all(|x| *x) && !self.done[7] && self.closure_requested_by.is_some()
            }
            CaseStatus::Closed => self.done.iter().all(|x| *x),
        };
        prefix && shape
    }
}

impl<U: Copy + Eq> Default for Case<U> {
    fn default() -> Self {
        Self::new()
    }
}

/// A citation of a claim or a stored document. Exactly one of the two.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Citation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
}

impl Citation {
    pub fn key(&self) -> Option<&str> {
        match (&self.claim, &self.artifact) {
            (Some(c), None) => Some(c),
            (None, Some(a)) => Some(a),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Recorded or uploaded by the tenant's own people.
    Internal,
    /// A supplier's 8D response document (or a claim from it). Evidence, never an approval.
    Supplier,
}

/// What the API found out about one cited id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cited {
    pub origin: Origin,
    /// Unix microseconds when the evidence entered KIZUNA.
    pub recorded_at: i64,
    /// False when a cited claim has been superseded or rejected.
    pub usable: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub user: String,
    pub role: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IsIsNot {
    pub dimension: String,
    #[serde(default)]
    pub is: String,
    #[serde(default)]
    pub is_not: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Item {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub evidence: Vec<Citation>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Horizontal {
    pub target: String,
    #[serde(default)]
    pub note: String,
}

/// The editable content of one discipline (all fields optional so drafts can be saved).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Content {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Member>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub statement: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub is_is_not: Vec<IsIsNot>,
    /// D3 containment actions, D4 causes, D7 prevention measures.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Item>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Citation>,
    /// D6: also require the monitored characteristic to be back in statistical control.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub spc: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub horizontal: Vec<Horizontal>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub horizontal_na_reason: String,
}

impl Content {
    /// Every citation in the content.
    pub fn citations(&self) -> impl Iterator<Item = &Citation> {
        self.evidence
            .iter()
            .chain(self.items.iter().flat_map(|i| i.evidence.iter()))
    }
}

/// One corrective action (D5) as the checker sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionFact {
    pub causes: Vec<u32>,
    /// Unix microseconds when implementation was recorded.
    pub done_at: Option<i64>,
}

/// State of the SPC re-check for D6 (computed by the API from new evidence).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpcCheck {
    NotApplicable,
    TooFewPoints,
    OutOfControl,
    InControl,
}

/// Everything the requirement checker needs besides the content.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub cited: BTreeMap<String, Cited>,
    pub tenant_users: BTreeSet<String>,
    /// Number of D4 causes (for D5 coverage).
    pub causes: u32,
    pub actions: Vec<ActionFact>,
    pub spc: Option<SpcCheck>,
}

pub const MAX_TEXT: usize = 4000;
const DIMENSIONS: [&str; 4] = ["what", "where", "when", "how_many"];
const ROLES: [&str; 3] = ["leader", "member", "champion"];
const CAUSE_KINDS: [&str; 2] = ["occurrence", "escape"];
const MEASURE_KINDS: [&str; 5] = ["standard", "fmea", "control_plan", "training", "other"];

fn blank(s: &str) -> bool {
    s.trim().is_empty()
}

/// Structural problems that make content unsaveable (wrong enums, oversized text, malformed citations).
pub fn malformed(c: &Content) -> Option<&'static str> {
    let texts = [c.statement.as_str(), c.horizontal_na_reason.as_str()]
        .into_iter()
        .chain(c.items.iter().map(|i| i.description.as_str()))
        .chain(c.is_is_not.iter().flat_map(|r| [r.is.as_str(), r.is_not.as_str()]))
        .chain(c.horizontal.iter().flat_map(|h| [h.target.as_str(), h.note.as_str()]));
    if texts.into_iter().any(|t| t.chars().count() > MAX_TEXT) {
        return Some("text_too_long");
    }
    if c.members.len() > 50 || c.items.len() > 50 || c.is_is_not.len() > 20 || c.horizontal.len() > 50 {
        return Some("too_many_entries");
    }
    if c.citations().count() > 200 {
        return Some("too_many_citations");
    }
    if c.citations().any(|x| x.key().is_none()) {
        return Some("invalid_citation");
    }
    if c.members.iter().any(|m| !ROLES.contains(&m.role.as_str())) {
        return Some("invalid_role");
    }
    if c.is_is_not.iter().any(|r| !DIMENSIONS.contains(&r.dimension.as_str())) {
        return Some("invalid_dimension");
    }
    None
}

/// What discipline `d` still lacks before it may be completed (stable codes; empty = complete-able).
pub fn missing(d: Discipline, c: &Content, f: &Facts) -> Vec<String> {
    let mut m: Vec<String> = Vec::new();
    let mut need = |cond: bool, code: &str| {
        if cond {
            m.push(code.to_string());
        }
    };
    let cited_ok = |x: &Citation| x.key().and_then(|k| f.cited.get(k)).is_some_and(|e| e.usable);
    let all_cited_ok = |v: &[Citation]| !v.is_empty() && v.iter().all(cited_ok);
    match d.number() {
        1 => {
            let users: BTreeSet<&str> = c.members.iter().map(|x| x.user.as_str()).collect();
            need(users.len() < 2, "team_needs_two_people");
            need(users.len() != c.members.len(), "team_duplicate_member");
            need(!c.members.iter().any(|x| x.role == "leader"), "team_needs_leader");
            need(users.iter().any(|u| !f.tenant_users.contains(*u)), "team_unknown_user");
        }
        2 => {
            need(blank(&c.statement), "problem_statement_required");
            need(
                !c.is_is_not
                    .iter()
                    .any(|r| r.dimension == "what" && !blank(&r.is) && !blank(&r.is_not)),
                "is_is_not_what_required",
            );
            need(
                c.is_is_not.iter().any(|r| blank(&r.is) || blank(&r.is_not)),
                "is_is_not_incomplete",
            );
            need(!all_cited_ok(&c.evidence), "problem_evidence_required");
        }
        3 => {
            need(c.items.is_empty(), "containment_action_required");
            need(
                c.items.iter().any(|i| blank(&i.description)),
                "containment_description_required",
            );
            need(
                c.items.iter().any(|i| !all_cited_ok(&i.evidence)),
                "containment_evidence_required",
            );
        }
        4 => {
            need(c.items.is_empty(), "root_cause_required");
            need(
                c.items
                    .iter()
                    .any(|i| !CAUSE_KINDS.contains(&i.kind.as_str()) || blank(&i.description)),
                "root_cause_incomplete",
            );
            need(
                !c.items.iter().any(|i| i.kind == "occurrence"),
                "occurrence_cause_required",
            );
            // Every cause must cite the claims/documents that support it.
            need(
                c.items.iter().any(|i| !all_cited_ok(&i.evidence)),
                "root_cause_evidence_required",
            );
        }
        5 => {
            need(f.actions.is_empty(), "corrective_action_required");
            let covered: BTreeSet<u32> = f.actions.iter().flat_map(|a| a.causes.iter().copied()).collect();
            need((0..f.causes).any(|i| !covered.contains(&i)), "cause_without_action");
        }
        6 => {
            need(f.actions.iter().any(|a| a.done_at.is_none()), "actions_not_implemented");
            let since = f.actions.iter().filter_map(|a| a.done_at).max();
            need(blank(&c.statement), "verification_summary_required");
            need(!all_cited_ok(&c.evidence), "verification_evidence_required");
            // Effectiveness rests on evidence that appeared after the fix (older citations may add context), and a
            // supplier's own 8D response cannot by itself prove its fix works.
            let fresh: Vec<&Cited> = c
                .evidence
                .iter()
                .filter_map(|x| x.key().and_then(|k| f.cited.get(k)))
                .filter(|e| e.usable && since.is_some_and(|s| e.recorded_at > s))
                .collect();
            need(
                !c.evidence.is_empty() && fresh.is_empty(),
                "verification_evidence_not_new",
            );
            need(
                !fresh.is_empty() && !fresh.iter().any(|e| e.origin == Origin::Internal),
                "verification_needs_independent_evidence",
            );
            if c.spc {
                match f.spc.unwrap_or(SpcCheck::NotApplicable) {
                    SpcCheck::InControl => {}
                    SpcCheck::NotApplicable => need(true, "spc_not_applicable"),
                    SpcCheck::TooFewPoints => need(true, "spc_too_few_new_points"),
                    SpcCheck::OutOfControl => need(true, "spc_still_out_of_control"),
                }
            }
        }
        7 => {
            need(c.items.is_empty(), "prevention_measure_required");
            need(
                c.items
                    .iter()
                    .any(|i| !MEASURE_KINDS.contains(&i.kind.as_str()) || blank(&i.description)),
                "prevention_incomplete",
            );
            need(
                c.horizontal.is_empty() && blank(&c.horizontal_na_reason),
                "horizontal_deployment_required",
            );
            need(
                c.horizontal.iter().any(|h| blank(&h.target)),
                "horizontal_target_required",
            );
        }
        _ => {
            need(blank(&c.statement), "closure_summary_required");
        }
    }
    m
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn d(n: u8) -> Discipline {
        Discipline::new(n).unwrap()
    }

    fn arb_event() -> impl Strategy<Value = Event<u8>> {
        prop_oneof![
            (1u8..=8).prop_map(|n| Event::Complete(d(n))),
            (1u8..=8).prop_map(|n| Event::ReopenStep(d(n))),
            (0u8..3).prop_map(|by| Event::RequestClosure { by }),
            (0u8..3).prop_map(|by| Event::ApproveClosure { by }),
            Just(Event::RejectClosure),
            Just(Event::Contradicted),
        ]
    }

    proptest! {
        #[test]
        fn invariants_hold_for_every_sequence(events in prop::collection::vec(arb_event(), 0..80)) {
            let mut c = Case::<u8>::new();
            for e in &events {
                if let Ok(n) = c.apply(e) {
                prop_assert!(n.invariants_hold(), "{:?} after {:?}", n, e);
                if let Event::ApproveClosure { by } = e {
                    prop_assert_ne!(c.closure_requested_by, Some(*by));
                    prop_assert_eq!(n.status, CaseStatus::Closed);
                }
                if matches!(e, Event::Contradicted) {
                    prop_assert_eq!(n.status, CaseStatus::Open);
                    prop_assert!(!n.done[5] && !n.done[6] && !n.done[7]);
                    prop_assert_eq!(&n.done[..5], &c.done[..5]);
                    prop_assert_eq!(n.reopened, c.reopened + u32::from(c.status == CaseStatus::Closed));
                }
                c = n;
                }
            }
        }

        #[test]
        fn errors_never_change_state(events in prop::collection::vec(arb_event(), 0..40), probe in arb_event()) {
            let mut c = Case::<u8>::new();
            for e in &events {
                if let Ok(n) = c.apply(e) { c = n; }
            }
            let before = c.clone();
            let _ = c.apply(&probe);
            prop_assert_eq!(before, c);
        }

        #[test]
        fn closed_only_through_two_people(events in prop::collection::vec(arb_event(), 0..120)) {
            let mut c = Case::<u8>::new();
            let mut requester = None;
            for e in &events {
                if let Ok(n) = c.apply(e) {
                    if let Event::RequestClosure { by } = e { requester = Some(*by); }
                    if n.status == CaseStatus::Closed && c.status != CaseStatus::Closed {
                        let Event::ApproveClosure { by } = e else { panic!("closed by {e:?}") };
                        prop_assert_ne!(requester, Some(*by));
                    }
                    c = n;
                }
            }
        }
    }

    #[test]
    fn happy_path_and_refusals() {
        let mut c = Case::<u8>::new();
        assert_eq!(c.apply(&Event::Complete(d(2))), Err(TransitionError::EarlierStepOpen));
        assert_eq!(c.apply(&Event::Complete(d(8))), Err(TransitionError::UseClosure));
        for n in 1..=7 {
            c = c.apply(&Event::Complete(d(n))).unwrap();
        }
        c = c.apply(&Event::RequestClosure { by: 1 }).unwrap();
        assert_eq!(
            c.apply(&Event::ApproveClosure { by: 1 }),
            Err(TransitionError::OwnClosure)
        );
        assert_eq!(c.apply(&Event::Complete(d(7))), Err(TransitionError::CaseNotOpen));
        c = c.apply(&Event::ApproveClosure { by: 2 }).unwrap();
        assert_eq!(c.status, CaseStatus::Closed);
        c = c.apply(&Event::Contradicted).unwrap();
        assert_eq!(
            (c.status, c.reopened, c.done),
            (CaseStatus::Open, 1, [true, true, true, true, true, false, false, false])
        );
    }

    fn cited(origin: Origin, at: i64) -> Cited {
        Cited {
            origin,
            recorded_at: at,
            usable: true,
        }
    }
    fn cite(k: &str) -> Citation {
        Citation {
            claim: Some(k.into()),
            artifact: None,
        }
    }

    #[test]
    fn root_cause_must_cite_evidence() {
        let mut f = Facts::default();
        f.cited.insert("c1".into(), cited(Origin::Internal, 1));
        let mut c = Content {
            items: vec![Item {
                kind: "occurrence".into(),
                description: "worn die".into(),
                evidence: vec![],
            }],
            ..Content::default()
        };
        assert_eq!(missing(d(4), &c, &f), vec!["root_cause_evidence_required"]);
        c.items[0].evidence.push(cite("c1"));
        assert!(missing(d(4), &c, &f).is_empty());
        c.items[0].evidence.push(cite("unknown"));
        assert_eq!(missing(d(4), &c, &f), vec!["root_cause_evidence_required"]);
    }

    #[test]
    fn verification_needs_new_independent_evidence() {
        let mut f = Facts {
            actions: vec![ActionFact {
                causes: vec![0],
                done_at: Some(100),
            }],
            ..Facts::default()
        };
        f.cited.insert("old".into(), cited(Origin::Internal, 50));
        f.cited.insert("sup".into(), cited(Origin::Supplier, 200));
        f.cited.insert("new".into(), cited(Origin::Internal, 300));
        let mut c = Content {
            statement: "ok".into(),
            evidence: vec![cite("old")],
            ..Content::default()
        };
        assert_eq!(missing(d(6), &c, &f), vec!["verification_evidence_not_new"]);
        c.evidence = vec![cite("sup")];
        assert_eq!(missing(d(6), &c, &f), vec!["verification_needs_independent_evidence"]);
        c.evidence = vec![cite("sup"), cite("new")];
        assert!(missing(d(6), &c, &f).is_empty());
        c.spc = true;
        f.spc = Some(SpcCheck::OutOfControl);
        assert_eq!(missing(d(6), &c, &f), vec!["spc_still_out_of_control"]);
        f.actions[0].done_at = None;
        assert!(missing(d(6), &c, &f).contains(&"actions_not_implemented".to_string()));
    }

    #[test]
    fn actions_cover_every_cause_and_team_has_leader() {
        let f = Facts {
            causes: 2,
            actions: vec![ActionFact {
                causes: vec![0],
                done_at: None,
            }],
            ..Facts::default()
        };
        assert_eq!(missing(d(5), &Content::default(), &f), vec!["cause_without_action"]);
        let mut f = Facts::default();
        f.tenant_users.extend(["a".to_string(), "b".to_string()]);
        let c = Content {
            members: vec![
                Member {
                    user: "a".into(),
                    role: "member".into(),
                },
                Member {
                    user: "b".into(),
                    role: "member".into(),
                },
            ],
            ..Content::default()
        };
        assert_eq!(missing(d(1), &c, &f), vec!["team_needs_leader"]);
        assert_eq!(
            malformed(&Content {
                members: vec![Member {
                    user: "a".into(),
                    role: "boss".into()
                }],
                ..Content::default()
            }),
            Some("invalid_role")
        );
    }
}

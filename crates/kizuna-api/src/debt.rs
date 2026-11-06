//! Evidence debt ledger (I10): read-only view of the evidence weaknesses accepted decisions rest on, per decision,
//! part and supplier, as known on any past date (transaction time). Components are reported separately and are
//! never folded into one score (`domain_manufacturing::debt`, `mfg-debt@1`).
//!
//! History comes from append-only records: claim reviews (`review_items`), decision revocations and successors,
//! conflict reviews, integrity reviews, match runs, claim supersession, supplier re-requests and 8D case events.
//! Findings whose current status is `obsolete`
//! carry no time of that change, so they are left out of every date (listed in `limitations`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use axum::Json;
use axum::extract::{Path, State};
use domain_manufacturing::debt::{
    Components, DAY, DEBT_VERSION, DecisionFacts, DepClaim, EXPIRY_WARN_DAYS, Item, LOW_CONFIDENCE_BPS, Ledger, Span,
    Target, Totals, components_at, debts_at, requirements, spans_from_events, spans_from_toggles, trend_points,
};
use entity_resolution::safe_normalize;
use policy_engine::Permission;
use serde::Deserialize;
use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime, Time};
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::Principal;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiQuery, ApiResult};

/// Transaction time in microseconds since the Unix epoch (the unit of `debt::Span`).
fn us(col: &str) -> String {
    format!("(extract(epoch FROM {col}) * 1000000)::int8")
}

fn to_us(t: OffsetDateTime) -> i64 {
    i64::try_from(t.unix_timestamp_nanos() / 1000).unwrap_or(i64::MAX)
}

fn from_us(t: i64) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(t) * 1000)
        .ok()
        .and_then(|d| d.format(&Rfc3339).ok())
        .unwrap_or_default()
}

const LIMITATIONS: &[&str] = &["debt.limit.obsolete_untimed"];
const SUPPLIER_PREDICATES: &[&str] = &["po.supplier", "qa.supplier", "mfg.supplier"];

/// What a linked item is, for labels and navigation.
#[derive(Clone)]
struct ItemInfo {
    kind: &'static str,
    label: String,
    detail: Value,
}

struct Entry {
    id: Uuid,
    title: String,
    kind: String,
    status: String,
    decided: i64,
    part: String,
    supplier: String,
    facts: DecisionFacts<Uuid>,
}

struct Loaded {
    entries: Vec<Entry>,
    items: HashMap<Uuid, ItemInfo>,
}

#[derive(sqlx::FromRow)]
struct DecRow {
    id: Uuid,
    kind: String,
    title: String,
    subject_type: String,
    subject_key: String,
    status: String,
    decided: i64,
    revoked: Option<i64>,
    superseded: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct DepRow {
    decision_id: Uuid,
    id: Uuid,
    predicate: String,
    subject_type: String,
    subject_key: String,
    artifact_id: Option<Uuid>,
    confidence_bps: i32,
    recorded: i64,
    superseded: Option<i64>,
    valid_to: Option<i64>,
    expiry_text: Option<String>,
    supplier: Option<String>,
}

/// End of a certificate's last valid day (UTC) from a `YYYY-MM-DD…` value; anything else is ignored.
fn date_end(s: &str) -> Option<i64> {
    let d = Date::parse(s.get(..10)?, time::macros::format_description!("[year]-[month]-[day]")).ok()?;
    Some(to_us(d.next_day()?.with_time(Time::MIDNIGHT).assume_utc()))
}

async fn aliases(tx: &mut Tx, subject_type: &str) -> ApiResult<HashMap<String, String>> {
    Ok(sqlx::query_as::<_, (String, String)>(
        "SELECT alias_key, canonical_key FROM entity_aliases WHERE subject_type = $1",
    )
    .bind(subject_type)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .collect())
}

fn canon(map: &HashMap<String, String>, k: &str) -> String {
    let n = safe_normalize(k);
    map.get(&n).cloned().unwrap_or(n)
}

/// Loads every accepted decision of the tenant (or one) with the timestamped history its debt is computed from.
async fn load(tx: &mut Tx, only: Option<Uuid>) -> ApiResult<Loaded> {
    let parts = aliases(tx, "part").await?;
    let suppliers = aliases(tx, "supplier").await?;
    let decs: Vec<DecRow> = sqlx::query_as(&format!(
        "SELECT d.id, d.kind, d.title, d.subject_type, d.subject_key, d.status, {} AS decided, rv.t AS revoked, sp.t AS superseded \
         FROM decisions d \
         LEFT JOIN (SELECT target_id, min({}) AS t FROM review_items WHERE target_type = 'decision' AND decision = 'revoke' GROUP BY 1) rv \
           ON rv.target_id = d.id \
         LEFT JOIN (SELECT supersedes, min({}) AS t FROM decisions WHERE supersedes IS NOT NULL GROUP BY 1) sp ON sp.supersedes = d.id \
         WHERE d.outcome IN ('approved', 'conditional') AND ($1::uuid IS NULL OR d.id = $1)",
        us("d.decided_at"),
        us("decided_at"),
        us("decided_at")
    ))
    .bind(only)
    .fetch_all(&mut **tx)
    .await?;

    let deps: Vec<DepRow> = sqlx::query_as(&format!(
        "SELECT dd.decision_id, c.id, c.predicate, c.subject_type, c.subject_key, c.artifact_id, c.confidence_bps, \
         {} AS recorded, {} AS superseded, {} AS valid_to, CASE WHEN c.predicate = 'mfg.certificate.expiry' THEN c.value_text END AS expiry_text, \
         CASE WHEN c.predicate = ANY($2) THEN c.value_text ELSE c.scope->>'supplier' END AS supplier \
         FROM decision_dependencies dd JOIN claims c ON c.tenant_id = dd.tenant_id AND c.id = dd.depends_on_claim \
         JOIN decisions d ON d.tenant_id = dd.tenant_id AND d.id = dd.decision_id \
         WHERE d.outcome IN ('approved', 'conditional') AND ($1::uuid IS NULL OR dd.decision_id = $1) \
         ORDER BY dd.decision_id, dd.id",
        us("c.recorded_at"),
        us("c.superseded_at"),
        us("c.valid_to")
    ))
    .bind(only)
    .bind(SUPPLIER_PREDICATES)
    .fetch_all(&mut **tx)
    .await?;
    let dep_claims: Vec<Uuid> = {
        let s: HashSet<Uuid> = deps.iter().map(|d| d.id).collect();
        s.into_iter().collect()
    };

    // Claim review history: approve confirms (and answers a pending request), request_evidence un-confirms and
    // opens a request, reject is final.
    #[derive(Default, Clone)]
    struct ClaimEv {
        confirm: Vec<(i64, bool)>,
        request: Vec<(i64, bool)>,
        rejected: Option<i64>,
    }
    let mut claim_events: HashMap<Uuid, ClaimEv> = HashMap::new();
    for (id, decision, at) in sqlx::query_as::<_, (Uuid, String, i64)>(&format!(
        "SELECT target_id, decision, {} FROM review_items WHERE target_type = 'claim' AND target_id = ANY($1) ORDER BY decided_at, id",
        us("decided_at")
    ))
    .bind(&dep_claims)
    .fetch_all(&mut **tx)
    .await?
    {
        let e = claim_events.entry(id).or_default();
        match decision.as_str() {
            "approve" => {
                e.confirm.push((at, true));
                e.request.push((at, false));
            }
            "reject" => {
                e.rejected = Some(e.rejected.map_or(at, |r| r.min(at)));
                e.request.push((at, false));
            }
            _ => {
                e.confirm.push((at, false));
                e.request.push((at, true));
            }
        }
    }

    let mut items: HashMap<Uuid, ItemInfo> = HashMap::new();

    // Conflicts on the cited claims; approve/reject resolves, request_evidence keeps them open.
    let conflict_rows: Vec<(Uuid, String, Uuid, Option<Uuid>, i64)> = sqlx::query_as(&format!(
        "SELECT id, kind, claim_a, claim_b, {} FROM conflicts WHERE status <> 'obsolete' \
         AND (claim_a = ANY($1) OR claim_b = ANY($1))",
        us("detected_at")
    ))
    .bind(&dep_claims)
    .fetch_all(&mut **tx)
    .await?;
    let conflict_ids: Vec<Uuid> = conflict_rows.iter().map(|r| r.0).collect();
    let mut closes: HashMap<Uuid, Vec<(i64, bool)>> = HashMap::new();
    for (id, at) in sqlx::query_as::<_, (Uuid, i64)>(&format!(
        "SELECT target_id, {} FROM review_items WHERE target_type = 'conflict' AND target_id = ANY($1) \
         AND decision IN ('approve', 'reject')",
        us("decided_at")
    ))
    .bind(&conflict_ids)
    .fetch_all(&mut **tx)
    .await?
    {
        closes.entry(id).or_default().push((at, false));
    }
    let mut conflicts_by_claim: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    let mut conflict_open: HashMap<Uuid, Vec<Span>> = HashMap::new();
    for (id, kind, a, b, at) in conflict_rows {
        conflicts_by_claim.entry(a).or_default().push(id);
        if let Some(b) = b {
            conflicts_by_claim.entry(b).or_default().push(id);
        }
        conflict_open.insert(
            id,
            spans_from_events(at, closes.get(&id).map_or(&[][..], Vec::as_slice)),
        );
        items.insert(
            id,
            ItemInfo {
                kind: "conflict",
                label: kind.clone(),
                detail: json!({ "kind": kind }),
            },
        );
    }

    // Integrity signals and drift findings; explained / accepted change / confirmed drift close them.
    let mut int_close: HashMap<Uuid, Vec<(i64, bool)>> = HashMap::new();
    for (id, decision, at) in sqlx::query_as::<_, (Uuid, String, i64)>(&format!(
        "SELECT target_id, decision, {} FROM integrity_reviews",
        us("decided_at")
    ))
    .fetch_all(&mut **tx)
    .await?
    {
        if matches!(decision.as_str(), "explained" | "accept_change" | "confirm_drift") {
            int_close.entry(id).or_default().push((at, false));
        }
    }
    let int_spans = |id: Uuid, at: i64| spans_from_events(at, int_close.get(&id).map_or(&[][..], Vec::as_slice));
    let mut sig_by_supplier: HashMap<String, Vec<(Uuid, Option<String>)>> = HashMap::new();
    let mut int_by_artifact: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    let mut int_by_part: HashMap<String, Vec<Uuid>> = HashMap::new();
    let mut int_open: HashMap<Uuid, Vec<Span>> = HashMap::new();
    for (id, kind, supplier, part, arts, strength, at) in
        sqlx::query_as::<_, (Uuid, String, String, Option<String>, Vec<Uuid>, String, i64)>(&format!(
            "SELECT id, kind, supplier_key, part_key, artifact_ids, strength, {} FROM integrity_signals WHERE status <> 'obsolete'",
            us("detected_at")
        ))
        .fetch_all(&mut **tx)
        .await?
    {
        let part = part.map(|p| canon(&parts, &p));
        sig_by_supplier.entry(supplier.clone()).or_default().push((id, part));
        for a in arts {
            int_by_artifact.entry(a).or_default().push(id);
        }
        int_open.insert(id, int_spans(id, at));
        items.insert(
            id,
            ItemInfo {
                kind: "integrity_signal",
                label: kind.clone(),
                detail: json!({ "kind": kind, "supplier_key": supplier, "strength": strength }),
            },
        );
    }
    for (id, kind, drawing, part, earlier, later, at) in
        sqlx::query_as::<_, (Uuid, String, String, String, Uuid, Uuid, i64)>(&format!(
            "SELECT id, kind, drawing_key, part_key, earlier_artifact, later_artifact, {} FROM revision_drift_findings \
             WHERE status <> 'obsolete'",
            us("detected_at")
        ))
        .fetch_all(&mut **tx)
        .await?
    {
        int_by_part.entry(canon(&parts, &part)).or_default().push(id);
        int_by_artifact.entry(earlier).or_default().push(id);
        int_by_artifact.entry(later).or_default().push(id);
        int_open.insert(id, int_spans(id, at));
        items.insert(
            id,
            ItemInfo {
                kind: "drift_finding",
                label: kind.clone(),
                detail: json!({ "kind": kind, "drawing_key": drawing }),
            },
        );
    }

    // Three-way match: every evaluation of the PO lines a decision was accepted on. A run's findings are open from
    // that run until the next evaluation of the same line.
    let runs: Vec<(Uuid, Uuid, String, String, String, i64)> = sqlx::query_as(&format!(
        "WITH lines AS (SELECT DISTINCT decision_id, purchase_order, po_line FROM match_runs \
                        WHERE decision_id IS NOT NULL AND ($1::uuid IS NULL OR decision_id = $1)) \
         SELECT l.decision_id, r.id, r.purchase_order, r.po_line, r.part_key, {} FROM lines l \
         JOIN match_runs r ON r.purchase_order = l.purchase_order AND r.po_line = l.po_line ORDER BY r.created_at, r.id",
        us("r.created_at")
    ))
    .bind(only)
    .fetch_all(&mut **tx)
    .await?;
    let run_ids: Vec<Uuid> = runs.iter().map(|r| r.1).collect::<HashSet<_>>().into_iter().collect();
    let mut findings_by_run: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (id, run, kind, severity, check) in sqlx::query_as::<_, (Uuid, Uuid, String, String, String)>(
        "SELECT id, run_id, kind, severity, check_name FROM match_findings WHERE run_id = ANY($1)",
    )
    .bind(&run_ids)
    .fetch_all(&mut **tx)
    .await?
    {
        findings_by_run.entry(run).or_default().push(id);
        items.insert(
            id,
            ItemInfo {
                kind: "match_finding",
                label: check.clone(),
                detail: json!({ "kind": kind, "severity": severity, "check": check }),
            },
        );
    }
    let mut line_runs: BTreeMap<(String, String), Vec<(Uuid, i64)>> = BTreeMap::new();
    let mut lines_of: HashMap<Uuid, Vec<(String, String)>> = HashMap::new();
    let mut match_part: HashMap<Uuid, String> = HashMap::new();
    for (dec, run, po, line, part, at) in &runs {
        let k = (po.clone(), line.clone());
        let v = line_runs.entry(k.clone()).or_default();
        if !v.iter().any(|r| r.0 == *run) {
            v.push((*run, *at));
        }
        let l = lines_of.entry(*dec).or_default();
        if !l.contains(&k) {
            l.push(k);
        }
        match_part.entry(*dec).or_insert_with(|| canon(&parts, part));
    }
    let mut match_items: HashMap<(String, String), Vec<Item<Uuid>>> = HashMap::new();
    for (k, rs) in &line_runs {
        let mut v = Vec::new();
        for (i, (run, at)) in rs.iter().enumerate() {
            let span = Span {
                from: *at,
                to: rs.get(i + 1).map(|r| r.1),
            };
            for f in findings_by_run.get(run).into_iter().flatten() {
                v.push(Item {
                    id: *f,
                    open: vec![span],
                });
            }
        }
        match_items.insert(k.clone(), v);
    }
    for (_, run, po, line, ..) in &runs {
        for f in findings_by_run.get(run).into_iter().flatten() {
            if let Some(i) = items.get_mut(f) {
                i.detail["purchase_order"] = json!(po);
                i.detail["po_line"] = json!(line);
            }
        }
    }

    // 8D cases with their close / reopen history.
    let mut capa_ev: HashMap<Uuid, Vec<(i64, bool)>> = HashMap::new();
    for (id, kind, at) in sqlx::query_as::<_, (Uuid, String, i64)>(&format!(
        "SELECT case_id, kind, {} FROM capa_events WHERE kind IN ('capa.closed', 'capa.reopened') ORDER BY at, id",
        us("at")
    ))
    .fetch_all(&mut **tx)
    .await?
    {
        capa_ev.entry(id).or_default().push((at, kind == "capa.reopened"));
    }
    let mut capa_by_part: HashMap<String, Vec<Uuid>> = HashMap::new();
    let mut capa_by_source: HashMap<String, Vec<Uuid>> = HashMap::new();
    let mut capa_open: HashMap<Uuid, Vec<Span>> = HashMap::new();
    for (id, reference, title, source_kind, source_ref, stype, skey, at) in sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            i64,
        ),
    >(&format!(
        "SELECT id, reference, title, source_kind, source_ref, subject_type, subject_key, {} FROM capa_cases",
        us("created_at")
    ))
    .fetch_all(&mut **tx)
    .await?
    {
        if let (Some("part"), Some(k)) = (stype.as_deref(), skey.as_deref()) {
            capa_by_part.entry(canon(&parts, k)).or_default().push(id);
        }
        capa_by_source.entry(source_ref).or_default().push(id);
        capa_open.insert(
            id,
            spans_from_events(at, capa_ev.get(&id).map_or(&[][..], Vec::as_slice)),
        );
        items.insert(
            id,
            ItemInfo {
                kind: "capa_case",
                label: reference.clone(),
                detail: json!({ "reference": reference, "title": title, "source_kind": source_kind }),
            },
        );
    }

    // Supplier re-requests (resend drafts): pending from creation until cancelled; an approved request stays
    // pending until the decision it covers is reconfirmed (a new version) or revoked.
    let mut requests_of: HashMap<Uuid, Vec<Item<Uuid>>> = HashMap::new();
    for (id, reference, supplier, decision_ids, status, at, reviewed) in
        sqlx::query_as::<_, (Uuid, String, String, Vec<Uuid>, String, i64, Option<i64>)>(&format!(
            "SELECT id, reference, supplier, decision_ids, status, {}, {} FROM evidence_requests \
             WHERE ($1::uuid IS NULL OR $1 = ANY(decision_ids))",
            us("created_at"),
            us("reviewed_at")
        ))
        .bind(only)
        .fetch_all(&mut **tx)
        .await?
    {
        let open = match (status.as_str(), reviewed) {
            ("cancelled", Some(r)) => spans_from_events(at, &[(r, false)]),
            _ => vec![Span::open(at)],
        };
        for d in decision_ids {
            requests_of.entry(d).or_default().push(Item { id, open: open.clone() });
        }
        items.insert(
            id,
            ItemInfo {
                kind: "evidence_request",
                label: reference.clone(),
                detail: json!({ "reference": reference, "supplier": supplier, "status": status }),
            },
        );
    }

    // Assemble per decision.
    let mut deps_of: HashMap<Uuid, Vec<&DepRow>> = HashMap::new();
    for d in &deps {
        deps_of.entry(d.decision_id).or_default().push(d);
    }
    let item = |id: Uuid, open: &HashMap<Uuid, Vec<Span>>| Item {
        id,
        open: open.get(&id).cloned().unwrap_or_default(),
    };
    let mut entries = Vec::with_capacity(decs.len());
    for d in decs {
        let ds = deps_of.remove(&d.id).unwrap_or_default();
        let part = if d.subject_type == "part" {
            canon(&parts, &d.subject_key)
        } else {
            ds.iter()
                .find(|c| c.subject_type == "part")
                .map(|c| canon(&parts, &c.subject_key))
                .or_else(|| match_part.get(&d.id).cloned())
                .unwrap_or_default()
        };
        let supplier = ds
            .iter()
            .filter(|c| SUPPLIER_PREDICATES.contains(&c.predicate.as_str()))
            .chain(ds.iter())
            .find_map(|c| c.supplier.as_deref())
            .map(|s| canon(&suppliers, s))
            .unwrap_or_default();
        let mut claims = Vec::with_capacity(ds.len());
        let mut seen = HashSet::new();
        for c in &ds {
            if !seen.insert(c.id) {
                continue;
            }
            let ev = claim_events.get(&c.id).cloned().unwrap_or_default();
            let expiry = [c.valid_to, c.expiry_text.as_deref().and_then(date_end)]
                .into_iter()
                .flatten()
                .min();
            claims.push(DepClaim {
                id: c.id,
                predicate: c.predicate.clone(),
                confidence_bps: c.confidence_bps,
                recorded: c.recorded,
                confirmed: spans_from_toggles(&ev.confirm),
                requested: spans_from_toggles(&ev.request),
                rejected_from: ev.rejected,
                superseded_from: c.superseded,
                expiry,
            });
        }
        let mut integrity: Vec<Uuid> = Vec::new();
        for c in &ds {
            if let Some(a) = c.artifact_id {
                integrity.extend(int_by_artifact.get(&a).into_iter().flatten());
            }
        }
        if !supplier.is_empty() {
            for (id, p) in sig_by_supplier.get(&supplier).into_iter().flatten() {
                if p.as_ref().is_none_or(|p| *p == part) {
                    integrity.push(*id);
                }
            }
        }
        if !part.is_empty() {
            integrity.extend(int_by_part.get(&part).into_iter().flatten());
        }
        integrity.sort_unstable();
        integrity.dedup();
        let mut conflicts: Vec<Uuid> = ds
            .iter()
            .flat_map(|c| conflicts_by_claim.get(&c.id).into_iter().flatten().copied())
            .collect();
        conflicts.sort_unstable();
        conflicts.dedup();
        let mut matches: Vec<Item<Uuid>> = Vec::new();
        for l in lines_of.get(&d.id).into_iter().flatten() {
            matches.extend(match_items.get(l).into_iter().flatten().cloned());
        }
        let mut capa: Vec<Uuid> = capa_by_part.get(&part).cloned().unwrap_or_default();
        for src in integrity.iter().chain(&conflicts).chain(matches.iter().map(|m| &m.id)) {
            capa.extend(capa_by_source.get(&src.to_string()).into_iter().flatten());
        }
        capa.sort_unstable();
        capa.dedup();
        let end = [d.revoked, d.superseded].into_iter().flatten().min();
        entries.push(Entry {
            id: d.id,
            title: d.title,
            kind: d.kind.clone(),
            status: d.status,
            decided: d.decided,
            part,
            supplier,
            facts: DecisionFacts {
                kind: d.kind,
                accepted: Span {
                    from: d.decided,
                    to: end,
                },
                deps: claims,
                integrity: integrity.into_iter().map(|i| item(i, &int_open)).collect(),
                match_findings: matches,
                conflicts: conflicts.into_iter().map(|i| item(i, &conflict_open)).collect(),
                capa: capa.into_iter().map(|i| item(i, &capa_open)).collect(),
                requests: requests_of.remove(&d.id).unwrap_or_default(),
            },
        });
    }
    Ok(Loaded { entries, items })
}

// ---------------------------------------------------------------------------------------------------------------
// Query parameters

fn parse_as_of(s: Option<&str>, now: OffsetDateTime) -> ApiResult<i64> {
    let now_us = to_us(now);
    let t = match s {
        None | Some("") => return Ok(now_us),
        Some(s) if s.len() == 10 => date_end(s)
            .map(|e| e - 1)
            .ok_or(ApiError::BadRequest("invalid_as_of"))?,
        Some(s) => to_us(OffsetDateTime::parse(s, &Rfc3339).map_err(|_| ApiError::BadRequest("invalid_as_of"))?),
    };
    // The end of today is "now": a reader cannot know anything recorded later.
    Ok(t.min(now_us))
}

#[derive(Deserialize)]
pub struct LedgerQuery {
    pub as_of: Option<String>,
    pub group: Option<String>,
    pub points: Option<u32>,
    pub step_days: Option<u32>,
    pub sort: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub part: Option<String>,
    pub supplier: Option<String>,
    pub q: Option<String>,
}

/// Components in display order; `expiring` stands for the expiry component in counts and trends.
pub const COMPONENTS: &[&str] = &[
    "missing_required",
    "unconfirmed",
    "low_confidence",
    "rejected",
    "superseded",
    "re_requested",
    "integrity",
    "match_findings",
    "conflicts",
    "capa",
    "expiring",
    "expired",
];

fn component(t: &Totals, k: &str) -> u32 {
    match k {
        "missing_required" => t.missing_required,
        "unconfirmed" => t.unconfirmed,
        "low_confidence" => t.low_confidence,
        "rejected" => t.rejected,
        "superseded" => t.superseded,
        "re_requested" => t.re_requested,
        "integrity" => t.integrity,
        "match_findings" => t.match_findings,
        "conflicts" => t.conflicts,
        "capa" => t.capa,
        "expiring" => t.expiring,
        "expired" => t.expired,
        "with_debt" => t.with_debt,
        _ => t.decisions,
    }
}

fn trend_json(series: &[Totals]) -> Value {
    let mut m = serde_json::Map::new();
    for k in COMPONENTS.iter().chain(&["decisions", "with_debt"]) {
        m.insert(
            (*k).into(),
            json!(series.iter().map(|t| component(t, k)).collect::<Vec<_>>()),
        );
    }
    Value::Object(m)
}

fn totals_at(entries: &[&Entry], t: i64) -> Totals {
    let mut l = Ledger::default();
    for e in entries {
        if let Some(c) = components_at(&e.facts, t) {
            l.add(&c);
        }
    }
    l.totals()
}

struct Group<'a> {
    key: String,
    label: String,
    members: Vec<&'a Entry>,
    now: Totals,
}

/// GET /evidence-debt: the ledger grouped by part, supplier or decision, with per-component trends.
pub async fn ledger(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<LedgerQuery>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionRead).await?;
    p.require(&st, Permission::ClaimRead).await?;
    let started = Instant::now();
    let t = parse_as_of(q.as_of.as_deref(), st.clock.now())?;
    let group = q.group.as_deref().unwrap_or("part");
    if !matches!(group, "part" | "supplier" | "decision") {
        return Err(ApiError::BadRequest("invalid_group"));
    }
    let sort = q.sort.as_deref().unwrap_or("with_debt");
    if !(COMPONENTS.contains(&sort) || matches!(sort, "with_debt" | "decisions" | "key" | "nearest_expiry")) {
        return Err(ApiError::BadRequest("invalid_sort"));
    }
    let points = q.points.unwrap_or(12);
    let step_days = q.step_days.unwrap_or(7);
    if !(1..=52).contains(&points) || !(1..=366).contains(&step_days) {
        return Err(ApiError::BadRequest("invalid_trend"));
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 500) as usize;
    let offset = q.offset.unwrap_or(0) as usize;

    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let loaded = load(&mut tx, None).await?;
    tx.commit().await?;
    let load_ms = started.elapsed().as_millis();

    let needle = q.q.as_deref().map(str::to_lowercase).filter(|s| !s.is_empty());
    let entries: Vec<&Entry> = loaded
        .entries
        .iter()
        .filter(|e| q.part.as_deref().is_none_or(|p| e.part == p))
        .filter(|e| q.supplier.as_deref().is_none_or(|s| e.supplier == s))
        .filter(|e| {
            needle.as_deref().is_none_or(|n| {
                e.part.to_lowercase().contains(n)
                    || e.supplier.to_lowercase().contains(n)
                    || e.title.to_lowercase().contains(n)
            })
        })
        .collect();

    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for e in &entries {
        let (key, label) = match group {
            "part" => (e.part.clone(), e.part.clone()),
            "supplier" => (e.supplier.clone(), e.supplier.clone()),
            _ => (e.id.to_string(), e.title.clone()),
        };
        groups
            .entry(key.clone())
            .or_insert_with(|| Group {
                key,
                label,
                members: Vec::new(),
                now: Totals::default(),
            })
            .members
            .push(e);
    }
    let mut rows: Vec<Group> = groups
        .into_values()
        .map(|mut g| {
            g.now = totals_at(&g.members, t);
            g
        })
        .filter(|g| g.now.decisions > 0)
        .collect();
    match sort {
        "key" => rows.sort_by(|a, b| a.label.cmp(&b.label)),
        "nearest_expiry" => rows.sort_by(|a, b| {
            (a.now.nearest_expiry_days.unwrap_or(i64::MAX), &a.key)
                .cmp(&(b.now.nearest_expiry_days.unwrap_or(i64::MAX), &b.key))
        }),
        k => rows.sort_by(|a, b| {
            (component(&b.now, k), component(&b.now, "with_debt"), &a.key).cmp(&(
                component(&a.now, k),
                component(&a.now, "with_debt"),
                &b.key,
            ))
        }),
    }
    let total_rows = rows.len();
    let pts = trend_points(t, i64::from(step_days) * DAY, points as usize);
    let page: Vec<Value> = rows
        .iter()
        .skip(offset)
        .take(limit)
        .map(|g| {
            let series: Vec<Totals> = pts.iter().map(|&x| totals_at(&g.members, x)).collect();
            let mut row = json!({ "key": g.key, "label": g.label, "totals": g.now, "trend": trend_json(&series) });
            if group == "decision" {
                let e = g.members[0];
                row["decision"] = json!({ "id": e.id, "kind": e.kind, "status": e.status, "part": e.part,
                    "supplier": e.supplier, "decided_at": from_us(e.decided) });
            }
            row
        })
        .collect();
    let overall: Vec<Totals> = pts.iter().map(|&x| totals_at(&entries, x)).collect();
    Ok(Json(json!({
        "version": DEBT_VERSION,
        "as_of": from_us(t),
        "group": group,
        "sort": sort,
        "points": pts.iter().map(|&x| from_us(x)).collect::<Vec<_>>(),
        "step_days": step_days,
        "thresholds": { "low_confidence_bps": LOW_CONFIDENCE_BPS, "expiry_warn_days": EXPIRY_WARN_DAYS },
        "components": COMPONENTS,
        "totals": totals_at(&entries, t),
        "totals_trend": trend_json(&overall),
        "rows": page,
        "total_rows": total_rows,
        "offset": offset,
        "limit": limit,
        "limitations": LIMITATIONS,
        "timing_ms": { "load": load_ms, "total": started.elapsed().as_millis() },
    })))
}

#[derive(Deserialize)]
pub struct DetailQuery {
    pub as_of: Option<String>,
    pub points: Option<u32>,
    pub step_days: Option<u32>,
}

#[derive(sqlx::FromRow)]
struct ClaimRef {
    id: Uuid,
    predicate: String,
    subject_key: String,
    value_text: Option<String>,
    value_num: Option<String>,
    value_unit: Option<String>,
    confidence_bps: i32,
    review_status: String,
    superseded_by: Option<Uuid>,
    artifact_id: Option<Uuid>,
    document: Option<String>,
    source_location: Value,
}

/// GET /evidence-debt/decisions/{id}: one decision's components with every item they rest on.
pub async fn decision(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiQuery(q): ApiQuery<DetailQuery>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionRead).await?;
    p.require(&st, Permission::ClaimRead).await?;
    let t = parse_as_of(q.as_of.as_deref(), st.clock.now())?;
    let points = q.points.unwrap_or(12);
    let step_days = q.step_days.unwrap_or(7);
    if !(1..=52).contains(&points) || !(1..=366).contains(&step_days) {
        return Err(ApiError::BadRequest("invalid_trend"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let exists: Option<String> = sqlx::query_scalar("SELECT outcome FROM decisions WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    exists.ok_or(ApiError::NotFound)?;
    let loaded = load(&mut tx, Some(id)).await?;
    let Some(e) = loaded.entries.first() else {
        tx.commit().await?;
        return Ok(Json(
            json!({ "decision": { "id": id }, "accepted": false, "as_of": from_us(t), "components": null }),
        ));
    };
    let claim_ids: Vec<Uuid> = e.facts.deps.iter().map(|c| c.id).collect();
    let refs: Vec<ClaimRef> = sqlx::query_as(
        "SELECT c.id, c.predicate, c.subject_key, c.value_text, c.value_num::text AS value_num, c.value_unit, \
         c.confidence_bps, c.review_status, c.superseded_by, c.artifact_id, a.display_name AS document, c.source_location \
         FROM claims c LEFT JOIN artifacts a ON a.tenant_id = c.tenant_id AND a.id = c.artifact_id WHERE c.id = ANY($1)",
    )
    .bind(&claim_ids)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let refs: HashMap<Uuid, &ClaimRef> = refs.iter().map(|r| (r.id, r)).collect();
    let claim_json = |id: &Uuid| {
        let dep = e.facts.deps.iter().find(|c| c.id == *id);
        refs.get(id).map_or(json!({ "type": "claim", "id": id }), |r| {
            json!({ "type": "claim", "id": r.id, "predicate": r.predicate, "subject_key": r.subject_key,
                "value": r.value_text.clone().or_else(|| r.value_num.clone()), "unit": r.value_unit,
                "confidence_bps": r.confidence_bps, "review_status_now": r.review_status, "superseded_by": r.superseded_by,
                "artifact_id": r.artifact_id, "document": r.document, "location": r.source_location,
                "expiry": dep.and_then(|d| d.expiry).map(from_us) })
        })
    };
    let item_json = |id: &Uuid| {
        loaded.items.get(id).map_or(
            json!({ "type": "unknown", "id": id }),
            |i| json!({ "type": i.kind, "id": id, "label": i.label, "detail": i.detail }),
        )
    };
    let comps: Option<Components<Uuid>> = components_at(&e.facts, t);
    let reqs = requirements(&e.kind);
    let components = comps.as_ref().map(|c| {
        let nearest: Vec<Value> = c
            .expiry_days
            .map(|d| {
                e.facts
                    .deps
                    .iter()
                    .filter(|x| x.expiry.is_some_and(|ex| (ex - t).div_euclid(DAY) == d))
                    .map(|x| claim_json(&x.id))
                    .collect()
            })
            .unwrap_or_default();
        json!({
            "missing_required": c.missing_required.iter().map(|n| {
                let r = reqs.iter().find(|r| r.name == *n);
                json!({ "type": "requirement", "name": n, "any_of": r.map(|r| r.any_of) })
            }).collect::<Vec<_>>(),
            "unconfirmed": c.unconfirmed.iter().map(claim_json).collect::<Vec<_>>(),
            "low_confidence": c.low_confidence.iter().map(claim_json).collect::<Vec<_>>(),
            "rejected": c.rejected.iter().map(claim_json).collect::<Vec<_>>(),
            "superseded": c.superseded.iter().map(claim_json).collect::<Vec<_>>(),
            "re_requested": c.re_requested.iter().map(|i| if refs.contains_key(i) { claim_json(i) } else { item_json(i) })
                .collect::<Vec<_>>(),
            "integrity": c.integrity.iter().map(item_json).collect::<Vec<_>>(),
            "match_findings": c.match_findings.iter().map(item_json).collect::<Vec<_>>(),
            "conflicts": c.conflicts.iter().map(item_json).collect::<Vec<_>>(),
            "capa": c.capa.iter().map(item_json).collect::<Vec<_>>(),
            "expiry": { "days": c.expiry_days, "expiring": c.expiring(), "expired": c.expired(), "claims": nearest },
        })
    });
    let pts = trend_points(t, i64::from(step_days) * DAY, points as usize);
    let series: Vec<Totals> = pts.iter().map(|&x| totals_at(&[e], x)).collect();
    Ok(Json(json!({
        "version": DEBT_VERSION,
        "as_of": from_us(t),
        "decision": { "id": e.id, "title": e.title, "kind": e.kind, "status_now": e.status, "part": e.part,
            "supplier": e.supplier, "decided_at": from_us(e.decided),
            "accepted_until": e.facts.accepted.to.map(from_us) },
        "accepted": comps.is_some(),
        "requirements": reqs.iter().map(|r| json!({ "name": r.name, "any_of": r.any_of })).collect::<Vec<_>>(),
        "components": components,
        "debts": debts_at(&e.facts, t).into_iter().map(|x| json!({ "kind": x.kind, "target": x.target,
            "since": from_us(x.since), "age_days": (t - x.since).div_euclid(DAY) })).collect::<Vec<_>>(),
        "totals": comps.as_ref().map(|_| totals_at(&[e], t)),
        "points": pts.iter().map(|&x| from_us(x)).collect::<Vec<_>>(),
        "trend": trend_json(&series),
        "thresholds": { "low_confidence_bps": LOW_CONFIDENCE_BPS, "expiry_warn_days": EXPIRY_WARN_DAYS },
        "limitations": LIMITATIONS,
    })))
}

// ---------------------------------------------------------------------------------------------------------------
// Items: the ledger turned around, one row per piece of evidence debt with its age and the decisions resting on it.

#[derive(Deserialize)]
pub struct ItemsQuery {
    pub as_of: Option<String>,
    pub kind: Option<String>,
    pub part: Option<String>,
    pub supplier: Option<String>,
    pub sort: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// Debt kinds an item row can have (`expiring`/`expired` are per claim here, not per decision).
const ITEM_KINDS: &[&str] = &[
    "missing_required",
    "unconfirmed",
    "low_confidence",
    "rejected",
    "superseded",
    "re_requested",
    "expiring",
    "expired",
    "integrity",
    "match_findings",
    "conflicts",
    "capa",
];

/// Most decisions listed per row; `blocks` always carries the full count.
const BLOCKS_LISTED: usize = 20;

struct ItemRow<'a> {
    kind: &'static str,
    target: Target<Uuid>,
    since: i64,
    decisions: Vec<&'a Entry>,
}

/// GET /evidence-debt/items: each missing, unconfirmed, re-requested, superseded, rejected or expiring piece of
/// evidence (and each open linked finding) as of a date, with its age and the accepted decisions resting on it.
pub async fn items(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<ItemsQuery>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionRead).await?;
    p.require(&st, Permission::ClaimRead).await?;
    let t = parse_as_of(q.as_of.as_deref(), st.clock.now())?;
    if q.kind.as_deref().is_some_and(|k| !ITEM_KINDS.contains(&k)) {
        return Err(ApiError::BadRequest("invalid_kind"));
    }
    let sort = q.sort.as_deref().unwrap_or("age");
    if !matches!(sort, "age" | "blocks") {
        return Err(ApiError::BadRequest("invalid_sort"));
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 500) as usize;
    let offset = q.offset.unwrap_or(0) as usize;

    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let loaded = load(&mut tx, None).await?;

    // Keyed by (kind, target, part); the part is set only for missing evidence, so the same requirement missing
    // on two parts is two rows.
    let mut rows: BTreeMap<(&'static str, Target<Uuid>, Option<String>), ItemRow> = BTreeMap::new();
    let mut by_kind: BTreeMap<&'static str, u32> = BTreeMap::new();
    for e in loaded
        .entries
        .iter()
        .filter(|e| q.part.as_deref().is_none_or(|p| e.part == p))
        .filter(|e| q.supplier.as_deref().is_none_or(|s| e.supplier == s))
    {
        for d in debts_at(&e.facts, t) {
            if q.kind.as_deref().is_some_and(|k| k != d.kind) {
                continue;
            }
            let part = matches!(d.target, Target::Requirement(_)).then(|| e.part.clone());
            let r = rows.entry((d.kind, d.target, part)).or_insert_with(|| {
                *by_kind.entry(d.kind).or_default() += 1;
                ItemRow {
                    kind: d.kind,
                    target: d.target,
                    since: d.since,
                    decisions: Vec::new(),
                }
            });
            r.since = r.since.min(d.since);
            if !r.decisions.iter().any(|x| x.id == e.id) {
                r.decisions.push(e);
            }
        }
    }
    let mut rows: Vec<ItemRow> = rows.into_values().collect();
    match sort {
        "blocks" => rows.sort_by(|a, b| (b.decisions.len(), a.since).cmp(&(a.decisions.len(), b.since))),
        _ => rows.sort_by(|a, b| (a.since, b.decisions.len()).cmp(&(b.since, a.decisions.len()))),
    }
    let total_rows = rows.len();
    let page: Vec<&ItemRow> = rows.iter().skip(offset).take(limit).collect();

    let claim_ids: Vec<Uuid> = page
        .iter()
        .filter_map(|r| {
            if let Target::Claim(id) = r.target {
                Some(id)
            } else {
                None
            }
        })
        .collect();
    let refs: HashMap<Uuid, Value> = sqlx::query_as::<_, (Uuid, Value)>(
        "SELECT c.id, jsonb_build_object('predicate', c.predicate, 'subject_key', c.subject_key, \
         'value', coalesce(c.value_text, c.value_num::text), 'unit', c.value_unit, 'artifact_id', c.artifact_id, \
         'document', a.display_name) FROM claims c \
         LEFT JOIN artifacts a ON a.tenant_id = c.tenant_id AND a.id = c.artifact_id WHERE c.id = ANY($1)",
    )
    .bind(&claim_ids)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .collect();
    tx.commit().await?;
    let reqs_any_of = |kind: &str, name: &str| requirements(kind).iter().find(|r| r.name == name).map(|r| r.any_of);
    let out: Vec<Value> = page
        .iter()
        .map(|r| {
            let (target, label, detail) = match r.target {
                Target::Claim(id) => {
                    let d = refs.get(&id).cloned().unwrap_or(Value::Null);
                    (
                        json!({ "type": "claim", "id": id }),
                        d["predicate"].as_str().unwrap_or_default().to_owned(),
                        d,
                    )
                }
                Target::Item(id) => loaded.items.get(&id).map_or(
                    (json!({ "type": "unknown", "id": id }), String::new(), Value::Null),
                    |i| (json!({ "type": i.kind, "id": id }), i.label.clone(), i.detail.clone()),
                ),
                Target::Requirement(name) => (
                    json!({ "type": "requirement", "name": name }),
                    name.to_owned(),
                    json!({ "any_of": r.decisions.first().and_then(|e| reqs_any_of(&e.kind, name)) }),
                ),
            };
            let mut parts: Vec<&str> = r.decisions.iter().map(|e| e.part.as_str()).collect();
            parts.sort_unstable();
            parts.dedup();
            let mut suppliers: Vec<&str> = r.decisions.iter().map(|e| e.supplier.as_str()).collect();
            suppliers.sort_unstable();
            suppliers.dedup();
            json!({
                "kind": r.kind,
                "target": target,
                "label": label,
                "detail": detail,
                "since": from_us(r.since),
                "age_days": (t - r.since).div_euclid(DAY),
                "blocks": r.decisions.len(),
                "decisions": r.decisions.iter().take(BLOCKS_LISTED).map(|e| json!({ "id": e.id, "title": e.title,
                    "kind": e.kind, "part": e.part, "supplier": e.supplier })).collect::<Vec<_>>(),
                "parts": parts,
                "suppliers": suppliers,
            })
        })
        .collect();
    Ok(Json(json!({
        "version": DEBT_VERSION,
        "as_of": from_us(t),
        "sort": sort,
        "kinds": ITEM_KINDS,
        "by_kind": by_kind,
        "rows": out,
        "total_rows": total_rows,
        "offset": offset,
        "limit": limit,
        "limitations": LIMITATIONS,
    })))
}

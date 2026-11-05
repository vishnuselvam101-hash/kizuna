//! M4 decision engine: decision records with typed evidence dependencies, selective invalidation on evidence
//! change, reproducible traces ("what did we know when"), human reconfirmation/revocation and verification reports.
//! KIZUNA never re-decides: affected decisions are flagged for human review and earlier results are preserved.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use dependency_engine::{Change, Dep, ENGINE_VERSION, Impact, Materiality, Snapshot, assess, upstream_withdrawn};
use policy_engine::Permission;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::claims::{self, ClaimOut};
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiQuery, ApiResult};

pub fn snapshot_of(c: &ClaimOut) -> Snapshot {
    Snapshot {
        text: c.value_text.clone(),
        num: c.value_num.clone(),
        unit: c.value_unit.clone(),
        revision: c.revision.clone(),
        scope: c
            .scope
            .as_object()
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

pub fn new_evidence(c: &ClaimOut) -> Change {
    Change::NewEvidence {
        new_claim: c.id.to_string(),
        subject: c.subject_key.clone(),
        predicate: c.predicate.clone(),
        new: snapshot_of(c),
    }
}

fn applicability_of(c: &ClaimOut) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    if let Some(r) = &c.revision {
        m.insert("revision".into(), r.clone());
    }
    if let Some(s) = c.scope.get("supplier").and_then(Value::as_str) {
        m.insert("supplier".into(), s.to_string());
    }
    m
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

type DepRow = (
    Uuid,
    Option<Uuid>,
    Option<Uuid>,
    String,
    Option<String>,
    Value,
    Value,
    Value,
);

fn to_dep(r: DepRow) -> Dep {
    let str_map = |v: &Value| -> BTreeMap<String, String> {
        v.as_object()
            .map(|o| {
                o.iter()
                    .filter_map(|(k, x)| x.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    };
    Dep {
        decision: r.0.to_string(),
        claim: r.1.map(|x| x.to_string()),
        on_decision: r.2.map(|x| x.to_string()),
        subject: r.3,
        predicate: r.4,
        applicability: str_map(&r.5),
        materiality: serde_json::from_value(r.6).unwrap_or(Materiality::AnyChange),
        snapshot: serde_json::from_value(r.7).unwrap_or_default(),
    }
}

const DEP_SELECT: &str = "SELECT d.decision_id, d.depends_on_claim, d.depends_on_decision, d.subject_key, d.predicate, \
     d.applicability, d.materiality, d.snapshot FROM decision_dependencies d JOIN decisions x ON x.id = d.decision_id \
     WHERE x.status IN ('valid', 'potentially_affected')";

/// Loads the dependency rows relevant to a change: direct matches plus all decision→decision edges (for propagation).
async fn load_deps(tx: &mut Tx, claim: Option<Uuid>, subject_pred: Option<(&str, &str)>) -> ApiResult<Vec<Dep>> {
    let rows: Vec<DepRow> = sqlx::query_as(&format!(
        "{DEP_SELECT} AND (d.depends_on_decision IS NOT NULL OR d.depends_on_claim = $1 OR (d.subject_key = $2 AND d.predicate = $3))"
    ))
    .bind(claim)
    .bind(subject_pred.map(|s| s.0))
    .bind(subject_pred.map(|s| s.1))
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(to_dep).collect())
}

async fn apply_impacts(
    tx: &mut Tx,
    tenant: Uuid,
    actor: Uuid,
    impacts: Vec<Impact>,
    cause_claim: Option<Uuid>,
    cause_decision: Option<Uuid>,
) -> ApiResult<usize> {
    let mut flagged = 0;
    for i in impacts {
        let did = Uuid::parse_str(&i.decision).map_err(ApiError::internal)?;
        let prev: String = sqlx::query_scalar("SELECT status FROM decisions WHERE id = $1 FOR UPDATE")
            .bind(did)
            .fetch_one(&mut **tx)
            .await?;
        let new = if i.material && prev == "valid" {
            "potentially_affected"
        } else {
            prev.as_str()
        };
        sqlx::query(
            "INSERT INTO invalidation_events (id, tenant_id, decision_id, cause_claim, cause_decision, cause_kind, semantic_change, \
             material, reason, path, previous_status, new_status) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        )
        .bind(Uuid::now_v7())
        .bind(tenant)
        .bind(did)
        .bind(cause_claim)
        .bind(cause_decision)
        .bind(i.cause_kind)
        .bind(&i.semantic_change)
        .bind(i.material)
        .bind(&i.reason)
        .bind(json!(i.path))
        .bind(&prev)
        .bind(new)
        .execute(&mut **tx)
        .await?;
        if new != prev {
            sqlx::query("UPDATE decisions SET status = $2 WHERE id = $1")
                .bind(did)
                .bind(new)
                .execute(&mut **tx)
                .await?;
            audit::append(
                tx,
                tenant,
                Record {
                    actor_user_id: Some(actor),
                    actor_kind: "system",
                    action: "decision.potentially_affected",
                    target_type: "decision",
                    target_id: Some(did.to_string()),
                    details: json!({ "cause": i.cause_kind, "reason": i.reason, "engine": ENGINE_VERSION }),
                },
            )
            .await?;
            flagged += 1;
        }
    }
    Ok(flagged)
}

/// Hook: evidence changed. Runs in the caller's transaction so the flag commits with the change.
/// A correction or withdrawal also reaches decisions that depend on earlier versions of the same claim, and is
/// assessed against the value those decisions actually used (cumulative drift is not hidden by small steps).
pub async fn on_claim_change(tx: &mut Tx, tenant: Uuid, actor: Uuid, change: Change) -> ApiResult<usize> {
    match &change {
        Change::Superseded { old_claim, .. } | Change::Withdrawn { old_claim, .. } => {
            let old = Uuid::parse_str(old_claim).map_err(ApiError::internal)?;
            let mut versions: Vec<Uuid> = sqlx::query_scalar(
                "WITH RECURSIVE anc AS (SELECT id, 0 AS depth FROM claims WHERE superseded_by = $1 \
                   UNION ALL SELECT c.id, anc.depth + 1 FROM claims c JOIN anc ON c.superseded_by = anc.id WHERE anc.depth < 100) \
                 SELECT id FROM anc",
            )
            .bind(old)
            .fetch_all(&mut **tx)
            .await?;
            versions.push(old);
            let mut impacts: BTreeMap<String, Impact> = BTreeMap::new();
            for v in versions {
                let deps = load_deps(tx, Some(v), None).await?;
                let ch = match &change {
                    Change::Superseded { new_claim, new, .. } => Change::Superseded {
                        old_claim: v.to_string(),
                        new_claim: new_claim.clone(),
                        new: new.clone(),
                    },
                    Change::Withdrawn { reason, .. } => Change::Withdrawn {
                        old_claim: v.to_string(),
                        reason: reason.clone(),
                    },
                    Change::NewEvidence { .. } => unreachable!("matched above"),
                };
                for i in assess(&deps, &ch) {
                    match impacts.get(&i.decision) {
                        Some(prev) if prev.material || !i.material => {}
                        _ => {
                            impacts.insert(i.decision.clone(), i);
                        }
                    }
                }
            }
            apply_impacts(tx, tenant, actor, impacts.into_values().collect(), Some(old), None).await
        }
        Change::NewEvidence {
            new_claim,
            subject,
            predicate,
            ..
        } => {
            let deps = load_deps(tx, None, Some((subject, predicate))).await?;
            let cause = Uuid::parse_str(new_claim).ok();
            apply_impacts(tx, tenant, actor, assess(&deps, &change), cause, None).await
        }
    }
}

#[derive(Deserialize)]
pub struct EvidenceRef {
    pub claim_id: Uuid,
    pub role: Option<String>,
    pub materiality: Option<Materiality>,
}

#[derive(Deserialize)]
pub struct UpstreamRef {
    pub decision_id: Uuid,
    pub role: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateReq {
    pub kind: String,
    pub title: String,
    pub subject_type: String,
    pub subject_key: String,
    pub outcome: String,
    pub rationale: String,
    pub effective_from: Option<String>,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
    #[serde(default)]
    pub depends_on: Vec<UpstreamRef>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct DecisionOut {
    pub id: Uuid,
    pub kind: String,
    pub title: String,
    pub subject_type: String,
    pub subject_key: String,
    pub outcome: String,
    pub rationale: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub effective_from: Option<OffsetDateTime>,
    pub decided_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub decided_at: OffsetDateTime,
    pub status: String,
    pub derivation_sha256: String,
    pub supersedes: Option<Uuid>,
}

const DEC_COLS: &str = "id, kind, title, subject_type, subject_key, outcome, rationale, effective_from, decided_by, decided_at, \
                        status, encode(derivation_sha256, 'hex') AS derivation_sha256, supersedes";

pub async fn fetch(tx: &mut Tx, id: Uuid) -> ApiResult<DecisionOut> {
    sqlx::query_as::<_, DecisionOut>(&format!("SELECT {DEC_COLS} FROM decisions WHERE id = $1"))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)
}

fn role(r: &Option<String>) -> ApiResult<String> {
    match r.as_deref().unwrap_or("basis") {
        x @ ("basis" | "supporting" | "condition") => Ok(x.to_string()),
        _ => Err(ApiError::BadRequest("invalid_role")),
    }
}

struct Prepared {
    claim: ClaimOut,
    role: String,
    materiality: Materiality,
    artifact_sha256: Option<String>,
}

/// Inserts a decision with its dependencies and derivation snapshot.
#[allow(clippy::too_many_arguments)]
async fn insert_decision(
    tx: &mut Tx,
    p: &Principal,
    req: &CreateReq,
    effective_from: Option<OffsetDateTime>,
    evidence: Vec<Prepared>,
    upstream: Vec<(DecisionOut, String)>,
    supersedes: Option<Uuid>,
    notes: Vec<String>,
) -> ApiResult<Uuid> {
    let id = Uuid::now_v7();
    let decided_at = crate::audit::now_us();
    let claim_ids: Vec<Uuid> = evidence.iter().map(|e| e.claim.id).collect();
    let open_conflicts: Vec<Value> = sqlx::query_scalar(
        "SELECT json_build_object('id', id, 'kind', kind, 'status', status) FROM conflicts \
         WHERE status IN ('open', 'needs_evidence') AND (claim_a = ANY($1) OR claim_b = ANY($1)) ORDER BY id",
    )
    .bind(&claim_ids)
    .fetch_all(&mut **tx)
    .await?;
    let derivation = json!({
        "engine": ENGINE_VERSION,
        "rules": domain_manufacturing::RULE_VERSION,
        "mapping": domain_manufacturing::MAPPING_VERSION,
        "decided_at": rfc3339(decided_at),
        "outcome": req.outcome,
        "evidence": evidence.iter().map(|e| json!({
            "claim_id": e.claim.id, "role": e.role, "subject_key": e.claim.subject_key, "predicate": e.claim.predicate,
            "snapshot": snapshot_of(&e.claim), "artifact_id": e.claim.artifact_id, "artifact_sha256": e.artifact_sha256,
            "source_location": e.claim.source_location, "extraction_method": e.claim.extraction_method,
            "confidence_bps": e.claim.confidence_bps, "review_status": e.claim.review_status,
            "recorded_at": rfc3339(e.claim.recorded_at),
        })).collect::<Vec<_>>(),
        "upstream_decisions": upstream.iter().map(|(d, r)| json!({ "id": d.id, "role": r, "status": d.status, "derivation_sha256": d.derivation_sha256 })).collect::<Vec<_>>(),
        "open_conflicts_at_decision": open_conflicts,
        "notes": notes,
    });
    let canonical = audit_engine::canonical_json(&derivation).map_err(ApiError::internal)?;
    let hash = crate::crypto::sha256(canonical.as_bytes());
    sqlx::query(
        "INSERT INTO decisions (id, tenant_id, kind, title, subject_type, subject_key, outcome, rationale, effective_from, \
         decided_by, decided_at, derivation, derivation_sha256, supersedes) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)",
    )
    .bind(id)
    .bind(p.tenant_id)
    .bind(&req.kind)
    .bind(req.title.trim())
    .bind(&req.subject_type)
    .bind(&req.subject_key)
    .bind(&req.outcome)
    .bind(req.rationale.trim())
    .bind(effective_from)
    .bind(p.user_id)
    .bind(decided_at)
    .bind(&derivation)
    .bind(hash.to_vec())
    .bind(supersedes)
    .execute(&mut **tx)
    .await?;
    for e in &evidence {
        sqlx::query(
            "INSERT INTO decision_dependencies (id, tenant_id, decision_id, depends_on_claim, role, subject_key, predicate, applicability, materiality, snapshot) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(Uuid::now_v7())
        .bind(p.tenant_id)
        .bind(id)
        .bind(e.claim.id)
        .bind(&e.role)
        .bind(&e.claim.subject_key)
        .bind(&e.claim.predicate)
        .bind(json!(applicability_of(&e.claim)))
        .bind(json!(e.materiality))
        .bind(json!(snapshot_of(&e.claim)))
        .execute(&mut **tx)
        .await?;
    }
    for (d, r) in &upstream {
        sqlx::query(
            "INSERT INTO decision_dependencies (id, tenant_id, decision_id, depends_on_decision, role, subject_key, materiality, snapshot) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(Uuid::now_v7())
        .bind(p.tenant_id)
        .bind(id)
        .bind(d.id)
        .bind(r)
        .bind(&d.subject_key)
        .bind(json!(Materiality::AnyChange))
        .bind(json!({ "status": d.status, "derivation_sha256": d.derivation_sha256 }))
        .execute(&mut **tx)
        .await?;
    }
    audit::append(tx, p.tenant_id, Record::user(p.user_id, "decision.recorded", "decision", id,
        json!({ "kind": req.kind, "outcome": req.outcome, "derivation_sha256": hex::encode(hash), "supersedes": supersedes }))).await?;
    Ok(id)
}

async fn artifact_sha(tx: &mut Tx, c: &ClaimOut) -> ApiResult<Option<String>> {
    match c.artifact_id {
        Some(a) => Ok(
            sqlx::query_scalar("SELECT encode(sha256, 'hex') FROM artifacts WHERE id = $1")
                .bind(a)
                .fetch_optional(&mut **tx)
                .await?,
        ),
        None => Ok(None),
    }
}

fn validate_header(req: &CreateReq) -> ApiResult<()> {
    let ident = |s: &str, max: usize| {
        !s.is_empty()
            && s.len() <= max
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    };
    if !ident(&req.kind, 60) || !req.kind.as_bytes()[0].is_ascii_lowercase() || req.kind.len() < 3 {
        return Err(ApiError::BadRequest("invalid_kind"));
    }
    if !ident(&req.subject_type, 40) || req.subject_key.trim().is_empty() || req.subject_key.len() > 200 {
        return Err(ApiError::BadRequest("invalid_subject"));
    }
    if !matches!(req.outcome.as_str(), "approved" | "rejected" | "conditional") {
        return Err(ApiError::BadRequest("invalid_outcome"));
    }
    if req.title.trim().is_empty() || req.title.chars().count() > 200 {
        return Err(ApiError::BadRequest("invalid_title"));
    }
    if req.rationale.trim().is_empty() || req.rationale.chars().count() > 4000 {
        return Err(ApiError::BadRequest("rationale_required"));
    }
    if req.evidence.is_empty() && req.depends_on.is_empty() {
        return Err(ApiError::BadRequest("evidence_required"));
    }
    if req.evidence.len() + req.depends_on.len() > 200 {
        return Err(ApiError::BadRequest("too_many_dependencies"));
    }
    for e in &req.evidence {
        if let Some(Materiality::NumericAbs { threshold }) = &e.materiality
            && evidence_core::claim::parse_decimal(threshold).is_err()
        {
            return Err(ApiError::BadRequest("invalid_threshold"));
        }
    }
    Ok(())
}

pub async fn create(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<CreateReq>,
) -> ApiResult<Json<DecisionOut>> {
    p.require(&st, Permission::DecisionWrite).await?;
    validate_header(&req)?;
    let effective_from = req
        .effective_from
        .as_deref()
        .map(|v| OffsetDateTime::parse(v, &Rfc3339).map_err(|_| ApiError::BadRequest("invalid_timestamp")))
        .transpose()?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let mut evidence = Vec::new();
    for e in &req.evidence {
        let c = claims::fetch(&mut tx, e.claim_id)
            .await
            .map_err(|_| ApiError::BadRequest("unknown_claim"))?;
        if c.superseded_at.is_some() {
            return Err(ApiError::Conflict("claim_superseded"));
        }
        if c.review_status == "rejected" {
            return Err(ApiError::Conflict("claim_rejected"));
        }
        let artifact_sha256 = artifact_sha(&mut tx, &c).await?;
        evidence.push(Prepared {
            role: role(&e.role)?,
            materiality: e.materiality.clone().unwrap_or(Materiality::AnyChange),
            claim: c,
            artifact_sha256,
        });
    }
    let mut upstream = Vec::new();
    for u in &req.depends_on {
        let d = fetch(&mut tx, u.decision_id)
            .await
            .map_err(|_| ApiError::BadRequest("unknown_decision"))?;
        if d.status != "valid" {
            return Err(ApiError::Conflict("upstream_decision_not_valid"));
        }
        upstream.push((d, role(&u.role)?));
    }
    let id = insert_decision(&mut tx, &p, &req, effective_from, evidence, upstream, None, vec![]).await?;
    let out = fetch(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub status: Option<String>,
    pub subject_key: Option<String>,
}

pub async fn list(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> ApiResult<Json<Vec<DecisionOut>>> {
    p.require(&st, Permission::DecisionRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, DecisionOut>(&format!(
        "SELECT {DEC_COLS} FROM decisions WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR subject_key = $2) ORDER BY decided_at DESC LIMIT 500"
    ))
    .bind(q.status)
    .bind(q.subject_key)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct InvalidationOut {
    pub cause_kind: String,
    pub cause_claim: Option<Uuid>,
    pub cause_decision: Option<Uuid>,
    pub semantic_change: Value,
    pub material: bool,
    pub reason: String,
    pub path: Value,
    pub previous_status: String,
    pub new_status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub recorded_at: OffsetDateTime,
}

async fn invalidations(tx: &mut Tx, id: Uuid) -> ApiResult<Vec<InvalidationOut>> {
    Ok(sqlx::query_as::<_, InvalidationOut>(
        "SELECT cause_kind, cause_claim, cause_decision, semantic_change, material, reason, path, previous_status, new_status, recorded_at \
         FROM invalidation_events WHERE decision_id = $1 ORDER BY recorded_at",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?)
}

/// Follows `superseded_by` to the current version of a claim.
async fn current_version(tx: &mut Tx, id: Uuid) -> ApiResult<ClaimOut> {
    let mut c = claims::fetch(tx, id).await?;
    for _ in 0..100 {
        match c.superseded_by {
            Some(n) => c = claims::fetch(tx, n).await?,
            None => break,
        }
    }
    Ok(c)
}

/// Reproducible decision trace: evidence used (snapshot), what KIZUNA knew at decision time, what changed since,
/// invalidation history and a derivation hash check.
pub async fn trace(State(st): State<AppState>, p: Principal, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::DecisionRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    Ok(Json(build_trace(&mut tx, id).await?))
}

pub async fn build_trace(tx: &mut Tx, id: Uuid) -> ApiResult<Value> {
    let d = fetch(tx, id).await?;
    let (derivation, stored): (Value, Vec<u8>) =
        sqlx::query_as("SELECT derivation, derivation_sha256 FROM decisions WHERE id = $1")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
    let recomputed = audit_engine::canonical_json(&derivation)
        .map(|c| crate::crypto::sha256(c.as_bytes()).to_vec())
        .unwrap_or_default();
    type TraceDep = (Option<Uuid>, Option<Uuid>, String, Option<String>, Value, Value);
    let deps: Vec<TraceDep> = sqlx::query_as(
        "SELECT depends_on_claim, depends_on_decision, subject_key, predicate, snapshot, materiality FROM decision_dependencies WHERE decision_id = $1",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    let mut evidence = Vec::new();
    let mut known = Vec::new();
    let mut changed = Vec::new();
    for (claim, upstream, subject, predicate, snapshot, materiality) in deps {
        match (claim, upstream) {
            (Some(cid), _) => {
                let cur = current_version(tx, cid).await?;
                evidence.push(json!({
                    "claim_id": cid, "predicate": predicate, "snapshot_at_decision": snapshot, "materiality": materiality,
                    "current_claim_id": cur.id, "current_value": snapshot_of(&cur), "current_review_status": cur.review_status,
                    "changed": cur.id != cid || cur.review_status == "rejected",
                }));
                let k: Vec<ClaimOut> = sqlx::query_as(&format!(
                    "SELECT {} FROM claims WHERE subject_key = $1 AND predicate = $2 AND recorded_at <= $3 AND (superseded_at IS NULL OR superseded_at > $3) ORDER BY recorded_at",
                    claims::CLAIM_COLS
                ))
                .bind(&subject)
                .bind(&predicate)
                .bind(d.decided_at)
                .fetch_all(&mut **tx)
                .await?;
                known.extend(k.into_iter().map(|c| json!({ "claim_id": c.id, "predicate": c.predicate, "value": snapshot_of(&c), "recorded_at": rfc3339(c.recorded_at) })));
                let ch: Vec<ClaimOut> = sqlx::query_as(&format!(
                    "SELECT {} FROM claims WHERE subject_key = $1 AND predicate = $2 AND (recorded_at > $3 OR superseded_at > $3) ORDER BY recorded_at",
                    claims::CLAIM_COLS
                ))
                .bind(&subject)
                .bind(&predicate)
                .bind(d.decided_at)
                .fetch_all(&mut **tx)
                .await?;
                changed.extend(ch.into_iter().map(|c| json!({
                    "claim_id": c.id, "predicate": c.predicate, "value": snapshot_of(&c), "recorded_at": rfc3339(c.recorded_at),
                    "superseded_at": c.superseded_at.map(rfc3339), "event": if c.recorded_at > d.decided_at { "recorded_after_decision" } else { "superseded_after_decision" },
                })));
            }
            (None, Some(up)) => {
                let u = fetch(tx, up).await?;
                evidence.push(json!({ "upstream_decision": up, "snapshot_at_decision": snapshot, "current_status": u.status, "changed": u.status != "valid" }));
            }
            _ => {}
        }
    }
    known.dedup();
    changed.dedup();
    Ok(json!({
        "decision": d,
        "derivation": derivation,
        "derivation_reproducible": recomputed == stored,
        "evidence": evidence,
        "known_at_decision_time": known,
        "changed_since_decision": changed,
        "invalidations": invalidations(tx, id).await?,
    }))
}

pub async fn get(State(st): State<AppState>, p: Principal, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    trace(State(st), p, Path(id)).await
}

#[derive(Deserialize)]
pub struct CommentReq {
    pub comment: String,
}

fn comment_ok(c: &str) -> ApiResult<String> {
    let c = c.trim();
    if c.is_empty() || c.chars().count() > 2000 {
        return Err(ApiError::BadRequest("comment_required"));
    }
    Ok(c.to_string())
}

/// Human reconfirmation of a potentially affected decision: a new decision version is recorded against the current
/// evidence (following corrections); the old one is kept as `superseded`.
pub async fn reconfirm(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(req): ApiJson<CommentReq>,
) -> ApiResult<Json<DecisionOut>> {
    p.require(&st, Permission::ReviewDecide).await?;
    let comment = comment_ok(&req.comment)?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let old = fetch(&mut tx, id).await?;
    let locked: String = sqlx::query_scalar("SELECT status FROM decisions WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if locked != "potentially_affected" {
        return Err(ApiError::Conflict("decision_not_affected"));
    }
    let deps: Vec<(Option<Uuid>, Option<Uuid>, String, Value)> =
        sqlx::query_as("SELECT depends_on_claim, depends_on_decision, role, materiality FROM decision_dependencies WHERE decision_id = $1")
            .bind(id)
            .fetch_all(&mut *tx)
            .await?;
    let mut evidence = Vec::new();
    let mut upstream = Vec::new();
    let mut notes = vec![format!("reconfirmation of {id}: {comment}")];
    for (claim, up, role, materiality) in deps {
        if let Some(cid) = claim {
            let cur = current_version(&mut tx, cid).await?;
            if cur.review_status == "rejected" {
                notes.push(format!(
                    "evidence {cid} dropped: current version {} was rejected",
                    cur.id
                ));
                continue;
            }
            let artifact_sha256 = artifact_sha(&mut tx, &cur).await?;
            evidence.push(Prepared {
                claim: cur,
                role,
                materiality: serde_json::from_value(materiality).unwrap_or(Materiality::AnyChange),
                artifact_sha256,
            });
        } else if let Some(u) = up {
            let d = fetch(&mut tx, u).await?;
            if d.status != "valid" {
                notes.push(format!("upstream decision {u} dropped: status {}", d.status));
                continue;
            }
            upstream.push((d, role));
        }
    }
    if evidence.is_empty() && upstream.is_empty() {
        return Err(ApiError::Conflict("no_current_evidence"));
    }
    let header = CreateReq {
        kind: old.kind.clone(),
        title: old.title.clone(),
        subject_type: old.subject_type.clone(),
        subject_key: old.subject_key.clone(),
        outcome: old.outcome.clone(),
        rationale: format!("{} [reconfirmed: {comment}]", old.rationale)
            .chars()
            .take(4000)
            .collect(),
        effective_from: None,
        evidence: vec![],
        depends_on: vec![],
    };
    let new_id = insert_decision(
        &mut tx,
        &p,
        &header,
        old.effective_from,
        evidence,
        upstream,
        Some(id),
        notes,
    )
    .await?;
    sqlx::query("UPDATE decisions SET status = 'superseded' WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO review_items (id, tenant_id, target_type, target_id, decision, comment, reviewer) VALUES ($1,$2,'decision',$3,'reconfirm',$4,$5)")
        .bind(Uuid::now_v7())
        .bind(p.tenant_id)
        .bind(id)
        .bind(&comment)
        .bind(p.user_id)
        .execute(&mut *tx)
        .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "decision.reconfirmed",
            "decision",
            id,
            json!({ "new_version": new_id }),
        ),
    )
    .await?;
    let out = fetch(&mut tx, new_id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

pub async fn revoke(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(req): ApiJson<CommentReq>,
) -> ApiResult<Json<DecisionOut>> {
    p.require(&st, Permission::ReviewDecide).await?;
    let comment = comment_ok(&req.comment)?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM decisions WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    if !matches!(
        status.ok_or(ApiError::NotFound)?.as_str(),
        "valid" | "potentially_affected"
    ) {
        return Err(ApiError::Conflict("decision_final"));
    }
    sqlx::query("UPDATE decisions SET status = 'revoked' WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO review_items (id, tenant_id, target_type, target_id, decision, comment, reviewer) VALUES ($1,$2,'decision',$3,'revoke',$4,$5)")
        .bind(Uuid::now_v7())
        .bind(p.tenant_id)
        .bind(id)
        .bind(&comment)
        .bind(p.user_id)
        .execute(&mut *tx)
        .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(p.user_id, "decision.revoked", "decision", id, json!({})),
    )
    .await?;
    let deps = load_deps(&mut tx, None, None).await?;
    apply_impacts(
        &mut tx,
        p.tenant_id,
        p.user_id,
        upstream_withdrawn(&deps, &id.to_string(), "was revoked"),
        None,
        Some(id),
    )
    .await?;
    let out = fetch(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Decisions whose current dependencies include any of these claims (for finding detail views).
pub async fn affected_by_claims(tx: &mut Tx, claims: &[Uuid]) -> ApiResult<Vec<Value>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT json_build_object('id', x.id, 'title', x.title, 'status', x.status)::jsonb FROM decision_dependencies d \
         JOIN decisions x ON x.id = d.decision_id WHERE d.depends_on_claim = ANY($1) AND x.status IN ('valid', 'potentially_affected')",
    )
    .bind(claims)
    .fetch_all(&mut **tx)
    .await?)
}

#[derive(Deserialize)]
pub struct ReportQuery {
    pub format: Option<String>,
}

/// Traceable verification report for one decision (JSON or Markdown), with a SHA-256 over its canonical JSON.
pub async fn report(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiQuery(q): ApiQuery<ReportQuery>,
) -> ApiResult<Response> {
    p.require(&st, Permission::DecisionRead).await?;
    let mut tx = db::tenant_snapshot_tx(&st.pool, p.tenant_id).await?;
    let trace = build_trace(&mut tx, id).await?;
    let claim_ids: Vec<Uuid> = trace["evidence"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["claim_id"].as_str())
        .filter_map(|s| Uuid::parse_str(s).ok())
        .collect();
    let findings: Vec<Value> = sqlx::query_scalar(
        "SELECT json_build_object('id', id, 'kind', kind, 'status', status, 'why', explanation->>'why')::jsonb FROM conflicts \
         WHERE claim_a = ANY($1) OR claim_b = ANY($1) ORDER BY detected_at",
    )
    .bind(&claim_ids)
    .fetch_all(&mut *tx)
    .await?;
    let artifacts: Vec<Value> = sqlx::query_scalar(
        "SELECT DISTINCT json_build_object('id', a.id, 'name', a.display_name, 'sha256', encode(a.sha256, 'hex'), 'status', a.status, \
           'last_integrity_check', (SELECT json_build_object('ok', ok, 'at', checked_at) FROM integrity_checks i WHERE i.artifact_id = a.id ORDER BY checked_at DESC LIMIT 1))::jsonb \
         FROM artifacts a JOIN claims c ON c.artifact_id = a.id WHERE c.id = ANY($1)",
    )
    .bind(&claim_ids)
    .fetch_all(&mut *tx)
    .await?;
    let reviews: Vec<Value> = sqlx::query_scalar(
        "SELECT json_build_object('target_type', target_type, 'target_id', target_id, 'decision', decision, 'comment', comment, 'reviewer', reviewer, 'at', decided_at)::jsonb \
         FROM review_items WHERE target_id = $1 OR target_id = ANY($2) OR target_id IN (SELECT id FROM conflicts WHERE claim_a = ANY($2) OR claim_b = ANY($2)) ORDER BY 1",
    )
    .bind(id)
    .bind(&claim_ids)
    .fetch_all(&mut *tx)
    .await?;
    let chain = crate::audit::verify_tenant(&mut tx, p.tenant_id).await?;
    tx.commit().await?;
    crate::security_events::record(
        &st.pool,
        Some(p.tenant_id),
        crate::security_events::Kind::ExportReport,
        Some(p.user_id),
        Some(&p.ip),
        json!({ "decision_id": id, "format": q.format.as_deref().unwrap_or("json") }),
    )
    .await;
    let mut body = json!({
        "report_version": "kizuna-verification-report@1",
        "generated_at": rfc3339(OffsetDateTime::now_utc()),
        "generated_by": p.user_id,
        "decision": trace["decision"],
        "derivation_reproducible": trace["derivation_reproducible"],
        "evidence": trace["evidence"],
        "changed_since_decision": trace["changed_since_decision"],
        "invalidations": trace["invalidations"],
        "findings": findings,
        "source_documents": artifacts,
        "reviews": reviews,
        "audit_chain": { "intact": chain.intact, "events": chain.events_checked, "head_sha256": chain.head_hash },
        "statement": "Hashes show that stored documents and the audit log are unchanged since capture. They do not show that document contents are true or authentic. Extracted values are machine-read and carry their review status.",
    });
    let canonical = audit_engine::canonical_json(&body).map_err(ApiError::internal)?;
    body["report_sha256"] = json!(hex::encode(crate::crypto::sha256(canonical.as_bytes())));
    if q.format.as_deref() == Some("markdown") {
        let mut res = render_markdown(&body).into_response();
        res.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/markdown; charset=utf-8"),
        );
        return Ok(res);
    }
    Ok(Json(body).into_response())
}

fn md_escape(v: &Value) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Null => "—".into(),
        other => other.to_string(),
    };
    s.replace('|', "\\|").replace(['\n', '\r'], " ").replace('<', "&lt;")
}

pub fn render_markdown(r: &Value) -> String {
    let d = &r["decision"];
    let mut out = format!(
        "# Verification report: {}\n\nDecision `{}` · kind `{}` · outcome **{}** · status **{}**\n\nDecided {} by {} · derivation SHA-256 `{}` (reproducible: {})\n\nRationale: {}\n\n## Evidence\n\n| Claim | Predicate | Value at decision | Current | Changed |\n|---|---|---|---|---|\n",
        md_escape(&d["title"]),
        md_escape(&d["id"]),
        md_escape(&d["kind"]),
        md_escape(&d["outcome"]),
        md_escape(&d["status"]),
        md_escape(&d["decided_at"]),
        md_escape(&d["decided_by"]),
        md_escape(&d["derivation_sha256"]),
        r["derivation_reproducible"],
        md_escape(&d["rationale"]),
    );
    for e in r["evidence"].as_array().into_iter().flatten() {
        let val = |s: &Value| {
            md_escape(&json!(
                format!(
                    "{} {}",
                    s["num"].as_str().or(s["text"].as_str()).unwrap_or(""),
                    s["unit"].as_str().unwrap_or("")
                )
                .trim()
                .to_string()
            ))
        };
        out += &format!(
            "| {} | {} | {} | {} | {} |\n",
            md_escape(&e["claim_id"]),
            md_escape(&e["predicate"]),
            val(&e["snapshot_at_decision"]),
            val(&e["current_value"]),
            e["changed"]
        );
    }
    out += "\n## Source documents\n\n";
    for a in r["source_documents"].as_array().into_iter().flatten() {
        out += &format!(
            "- {} — SHA-256 `{}` ({})\n",
            md_escape(&a["name"]),
            md_escape(&a["sha256"]),
            md_escape(&a["status"])
        );
    }
    out += "\n## Findings\n\n";
    for f in r["findings"].as_array().into_iter().flatten() {
        out += &format!(
            "- {} ({}): {}\n",
            md_escape(&f["kind"]),
            md_escape(&f["status"]),
            md_escape(&f["why"])
        );
    }
    out += "\n## Changes since decision\n\n";
    for i in r["invalidations"].as_array().into_iter().flatten() {
        out += &format!(
            "- {} — material: {} — {}\n",
            md_escape(&i["recorded_at"]),
            i["material"],
            md_escape(&i["reason"])
        );
    }
    out += &format!(
        "\n## Integrity\n\nAudit chain intact: {} ({} events, head `{}`).\n\n{}\n\nReport SHA-256: `{}` · generated {}\n",
        r["audit_chain"]["intact"],
        r["audit_chain"]["events"],
        md_escape(&r["audit_chain"]["head_sha256"]),
        md_escape(&r["statement"]),
        md_escape(&r["report_sha256"]),
        md_escape(&r["generated_at"]),
    );
    out
}

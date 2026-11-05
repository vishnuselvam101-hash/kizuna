//! Claims (model v1): append-only, bitemporal assertions linked to their source artifact and location.
//! Corrections supersede; nothing is overwritten (docs/data-model/claims.md).

use axum::Json;
use axum::extract::{Path, State};
use evidence_core::claim::{self, ClaimInput, ValidClaim};
use policy_engine::Permission;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiQuery, ApiResult};

#[derive(Serialize, sqlx::FromRow, Clone)]
pub struct ClaimOut {
    pub id: Uuid,
    pub model_version: i16,
    pub subject_type: String,
    pub subject_key: String,
    pub predicate: String,
    pub value_text: Option<String>,
    pub value_num: Option<String>,
    pub value_unit: Option<String>,
    pub scope: Value,
    #[serde(with = "time::serde::rfc3339::option")]
    pub valid_from: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub valid_to: Option<OffsetDateTime>,
    pub revision: Option<String>,
    pub artifact_id: Option<Uuid>,
    pub source_location: Value,
    pub extraction_method: String,
    pub confidence_bps: i32,
    pub review_status: String,
    pub provenance: Value,
    pub recorded_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub recorded_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub superseded_at: Option<OffsetDateTime>,
    pub superseded_by: Option<Uuid>,
    pub artifact_display_name: Option<String>,
}

pub const CLAIM_COLS: &str = "id, model_version, subject_type, subject_key, predicate, value_text, value_num::text AS value_num, \
     value_unit, scope, valid_from, valid_to, revision, artifact_id, source_location, extraction_method, confidence_bps, \
     review_status, provenance, recorded_by, recorded_at, superseded_at, superseded_by, \
     (SELECT a.display_name FROM artifacts a WHERE a.id = claims.artifact_id) AS artifact_display_name";

#[derive(Deserialize)]
pub struct CreateReq {
    #[serde(flatten)]
    pub claim: ClaimInput,
    pub artifact_id: Option<Uuid>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
}

fn parse_time(s: &Option<String>) -> ApiResult<Option<OffsetDateTime>> {
    s.as_deref()
        .map(|v| OffsetDateTime::parse(v, &Rfc3339).map_err(|_| ApiError::BadRequest("invalid_timestamp")))
        .transpose()
}

/// Everything needed to insert one claim; shared by the API, supersession and the extraction pipeline.
pub struct NewClaim<'a> {
    pub claim: &'a ValidClaim,
    pub artifact_id: Option<Uuid>,
    pub valid_from: Option<OffsetDateTime>,
    pub valid_to: Option<OffsetDateTime>,
    pub extraction_method: &'a str,
    pub provenance: Value,
    pub recorded_by: Option<Uuid>,
    /// Created by AI-assisted extraction: stored as `ai_suggested` until a human reviews it.
    pub ai_suggested: bool,
}

pub async fn insert_claim(tx: &mut Tx, tenant: Uuid, n: NewClaim<'_>) -> ApiResult<Uuid> {
    if let (Some(f), Some(t)) = (n.valid_from, n.valid_to)
        && t <= f
    {
        return Err(ApiError::BadRequest("invalid_valid_time"));
    }
    if let Some(a) = n.artifact_id {
        let status: Option<String> = sqlx::query_scalar("SELECT status FROM artifacts WHERE id = $1")
            .bind(a)
            .fetch_optional(&mut **tx)
            .await?;
        // Claims may only cite released documents (review finding M4).
        if status.ok_or(ApiError::BadRequest("unknown_artifact"))? != "accepted" {
            return Err(ApiError::Conflict("artifact_not_released"));
        }
    }
    let id = Uuid::now_v7();
    let c = n.claim;
    sqlx::query(
        "INSERT INTO claims (id, tenant_id, model_version, subject_type, subject_key, predicate, value_text, value_num, value_unit, \
         scope, valid_from, valid_to, revision, artifact_id, source_location, extraction_method, confidence_bps, provenance, recorded_by, \
         review_status) VALUES ($1,$2,$3,$4,$5,$6,$7,$8::numeric,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19, \
         CASE WHEN $20 THEN 'ai_suggested' ELSE 'unreviewed' END)",
    )
    .bind(id)
    .bind(tenant)
    .bind(claim::MODEL_VERSION)
    .bind(&c.subject_type)
    .bind(&c.subject_key)
    .bind(&c.predicate)
    .bind(&c.value_text)
    .bind(&c.value_num)
    .bind(&c.value_unit)
    .bind(&c.scope)
    .bind(n.valid_from)
    .bind(n.valid_to)
    .bind(&c.revision)
    .bind(n.artifact_id)
    .bind(&c.source_location)
    .bind(n.extraction_method)
    .bind(c.confidence_bps)
    .bind(&n.provenance)
    .bind(n.recorded_by)
    .bind(n.ai_suggested)
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

pub async fn fetch(tx: &mut Tx, id: Uuid) -> ApiResult<ClaimOut> {
    sqlx::query_as::<_, ClaimOut>(&format!("SELECT {CLAIM_COLS} FROM claims WHERE id = $1"))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)
}

pub async fn create(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<CreateReq>,
) -> ApiResult<Json<ClaimOut>> {
    p.require(&st, Permission::ClaimWrite).await?;
    let mut v = claim::validate(&req.claim).map_err(|e| ApiError::BadRequest(e.code()))?;
    v.confidence_bps = 10_000; // manual: a human assertion; client-supplied confidence is ignored (review M4)
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let id = insert_claim(
        &mut tx,
        p.tenant_id,
        NewClaim {
            claim: &v,
            artifact_id: req.artifact_id,
            valid_from: parse_time(&req.valid_from)?,
            valid_to: parse_time(&req.valid_to)?,
            extraction_method: "manual",
            provenance: json!({ "entered_by": p.user_id, "via": "api" }),
            recorded_by: Some(p.user_id),
            ai_suggested: false,
        },
    )
    .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "claim.recorded",
            "claim",
            id,
            json!({ "subject": format!("{}:{}", v.subject_type, v.subject_key), "predicate": v.predicate }),
        ),
    )
    .await?;
    let out = fetch(&mut tx, id).await?;
    crate::decisions::on_claim_change(&mut tx, p.tenant_id, p.user_id, crate::decisions::new_evidence(&out)).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct SupersedeReq {
    #[serde(flatten)]
    pub create: CreateReq,
    pub reason: String,
}

/// Records a correction: the new claim supersedes the old one, which stays queryable forever.
pub async fn supersede(
    State(st): State<AppState>,
    p: Principal,
    Path(old): Path<Uuid>,
    ApiJson(req): ApiJson<SupersedeReq>,
) -> ApiResult<Json<ClaimOut>> {
    p.require(&st, Permission::ClaimWrite).await?;
    let reason = req.reason.trim();
    if reason.is_empty() || reason.chars().count() > 500 {
        return Err(ApiError::BadRequest("reason_required"));
    }
    let v = claim::validate(&req.create.claim).map_err(|e| ApiError::BadRequest(e.code()))?;
    let mut v = v;
    v.confidence_bps = 10_000; // manual: a human assertion; client-supplied confidence is ignored (review M4)
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let prev: Option<Option<OffsetDateTime>> =
        sqlx::query_scalar("SELECT superseded_at FROM claims WHERE id = $1 FOR UPDATE")
            .bind(old)
            .fetch_optional(&mut *tx)
            .await?;
    if prev.ok_or(ApiError::NotFound)?.is_some() {
        return Err(ApiError::Conflict("already_superseded"));
    }
    let id = insert_claim(
        &mut tx,
        p.tenant_id,
        NewClaim {
            claim: &v,
            artifact_id: req.create.artifact_id,
            valid_from: parse_time(&req.create.valid_from)?,
            valid_to: parse_time(&req.create.valid_to)?,
            extraction_method: "manual",
            provenance: json!({ "entered_by": p.user_id, "via": "api", "supersedes": old, "reason": reason }),
            recorded_by: Some(p.user_id),
            ai_suggested: false,
        },
    )
    .await?;
    sqlx::query("UPDATE claims SET superseded_at = now(), superseded_by = $2 WHERE id = $1")
        .bind(old)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "claim.superseded",
            "claim",
            old,
            json!({ "by": id, "reason": reason }),
        ),
    )
    .await?;
    let out = fetch(&mut tx, id).await?;
    crate::decisions::on_claim_change(
        &mut tx,
        p.tenant_id,
        p.user_id,
        dependency_engine::Change::Superseded {
            old_claim: old.to_string(),
            new_claim: id.to_string(),
            new: crate::decisions::snapshot_of(&out),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub subject_type: Option<String>,
    pub subject_key: Option<String>,
    pub predicate: Option<String>,
    pub artifact_id: Option<Uuid>,
    /// Transaction-time view: what KIZUNA knew at this instant (RFC 3339).
    pub known_at: Option<String>,
    /// Valid-time filter: claims whose validity covers this instant (unknown bounds count as open).
    pub valid_at: Option<String>,
    #[serde(default)]
    pub include_superseded: bool,
    pub limit: Option<i64>,
    /// Keyset paging: return claims with id greater than this (ids are time-ordered UUIDv7).
    pub after: Option<Uuid>,
}

pub async fn list(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> ApiResult<Json<Vec<ClaimOut>>> {
    p.require(&st, Permission::ClaimRead).await?;
    let known_at = parse_time(&q.known_at)?;
    let valid_at = parse_time(&q.valid_at)?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, ClaimOut>(&format!(
        "SELECT {CLAIM_COLS} FROM claims WHERE \
           ($1::text IS NULL OR subject_type = $1) AND ($2::text IS NULL OR subject_key = $2) \
           AND ($3::text IS NULL OR predicate = $3) AND ($4::uuid IS NULL OR artifact_id = $4) \
           AND ($5::timestamptz IS NULL OR (recorded_at <= $5 AND (superseded_at IS NULL OR superseded_at > $5))) \
           AND ($5::timestamptz IS NOT NULL OR $6 OR superseded_at IS NULL) \
           AND ($7::timestamptz IS NULL OR ((valid_from IS NULL OR valid_from <= $7) AND (valid_to IS NULL OR valid_to > $7))) \
           AND ($9::uuid IS NULL OR id > $9) \
         ORDER BY id LIMIT $8"
    ))
    .bind(q.subject_type)
    .bind(q.subject_key)
    .bind(q.predicate)
    .bind(q.artifact_id)
    .bind(known_at)
    .bind(q.include_superseded)
    .bind(valid_at)
    .bind(q.limit.unwrap_or(200).clamp(1, 1000))
    .bind(q.after)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Serialize)]
pub struct ClaimDetail {
    pub claim: ClaimOut,
    /// Older versions this claim replaced (most recent first).
    pub history: Vec<ClaimOut>,
    pub superseded_by: Option<ClaimOut>,
    pub artifact: Option<crate::artifacts::ArtifactOut>,
}

pub async fn get(State(st): State<AppState>, p: Principal, Path(id): Path<Uuid>) -> ApiResult<Json<ClaimDetail>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let c = fetch(&mut tx, id).await?;
    let history = sqlx::query_as::<_, ClaimOut>(&format!(
        "WITH RECURSIVE h AS (SELECT id, 0 AS depth FROM claims WHERE superseded_by = $1 \
           UNION ALL SELECT c.id, h.depth + 1 FROM claims c JOIN h ON c.superseded_by = h.id WHERE h.depth < 100) \
         SELECT {CLAIM_COLS} FROM claims WHERE id IN (SELECT id FROM h) ORDER BY recorded_at DESC"
    ))
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    let superseded_by = match c.superseded_by {
        Some(n) => Some(fetch(&mut tx, n).await?),
        None => None,
    };
    let artifact = match c.artifact_id {
        Some(a) => Some(crate::artifacts::fetch(&mut tx, a).await?),
        None => None,
    };
    tx.commit().await?;
    Ok(Json(ClaimDetail {
        claim: c,
        history,
        superseded_by,
        artifact,
    }))
}

#[derive(Serialize)]
pub struct PredicateEvidence {
    pub predicate: String,
    pub current: Vec<ClaimOut>,
    pub superseded_count: i64,
    /// True when current claims for this predicate disagree (shown, never auto-resolved).
    pub disagreement: bool,
}

/// Evidence explorer: all current claims about a subject grouped by predicate, with their sources.
pub async fn subject(
    State(st): State<AppState>,
    p: Principal,
    Path((stype, skey)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, ClaimOut>(&format!(
        "SELECT {CLAIM_COLS} FROM claims WHERE subject_type = $1 AND subject_key = $2 ORDER BY predicate, recorded_at"
    ))
    .bind(&stype)
    .bind(&skey)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut groups: Vec<PredicateEvidence> = Vec::new();
    for c in rows {
        let idx = match groups.iter().position(|g| g.predicate == c.predicate) {
            Some(i) => i,
            None => {
                groups.push(PredicateEvidence {
                    predicate: c.predicate.clone(),
                    current: vec![],
                    superseded_count: 0,
                    disagreement: false,
                });
                groups.len() - 1
            }
        };
        let g = &mut groups[idx];
        if c.superseded_at.is_some() {
            g.superseded_count += 1;
        } else {
            g.current.push(c);
        }
    }
    for g in &mut groups {
        let mut vals: Vec<(Option<&str>, Option<&str>, Option<&str>)> = g
            .current
            .iter()
            .map(|c| (c.value_text.as_deref(), c.value_num.as_deref(), c.value_unit.as_deref()))
            .collect();
        vals.sort();
        vals.dedup();
        g.disagreement = vals.len() > 1;
    }
    Ok(Json(
        json!({ "subject_type": stype, "subject_key": skey, "predicates": groups }),
    ))
}

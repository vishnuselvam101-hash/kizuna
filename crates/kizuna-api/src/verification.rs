//! M3 verification workflow: extraction of claims from documents, entity resolution decisions, conflict detection,
//! revision comparison and human review. Rules live in `domain-manufacturing`; this module does I/O and audit.

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

use axum::Json;
use axum::extract::{Path, State};
use conflict_engine::{Ctx, Fact, Rule};
use domain_manufacturing::{MAPPING_VERSION, RULE_VERSION, all_rules, compare_revisions, map_records};
use entity_resolution::{Outcome, candidates, safe_normalize};
use policy_engine::{AttributeRule, Permission, Resource, Subject};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::app::AppState;
use crate::artifacts;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::claims::{self, ClaimOut, NewClaim};
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiQuery, ApiResult};
use crate::extraction;
use crate::security_events::{self as sec, Kind};

/// ABAC: nobody reviews a claim they entered by hand (four-eyes principle).
pub struct NoSelfReview;

impl AttributeRule for NoSelfReview {
    fn deny(&self, s: &Subject, p: Permission, r: &Resource<'_>) -> Option<&'static str> {
        (p == Permission::ReviewDecide && r.owner_user_id == Some(s.user_id)).then_some("no_self_review")
    }
}

fn integerize(v: &mut Value) {
    match v {
        Value::Number(n) if n.as_i64().is_none() && n.as_u64().is_none() => {
            *v = json!(n.as_f64().map(|f| f.round().clamp(0.0, 999_999.0) as i64).unwrap_or(0));
        }
        Value::Array(a) => a.iter_mut().for_each(integerize),
        Value::Object(o) => o.values_mut().for_each(integerize),
        _ => {}
    }
}

/// Canonical identity of a part key: safe normalisation, then confirmed alias.
pub async fn canonical_key(tx: &mut Tx, key: &str) -> ApiResult<String> {
    let n = safe_normalize(key);
    let c: Option<String> =
        sqlx::query_scalar("SELECT canonical_key FROM entity_aliases WHERE subject_type = 'part' AND alias_key = $1")
            .bind(&n)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(c.unwrap_or(n))
}

/// Everyone who put evidence into these claims: who recorded them and who uploaded their source documents.
/// They may not review that evidence (four-eyes, review finding H1).
pub async fn claim_owners(tx: &mut Tx, claims: &[Uuid]) -> ApiResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT recorded_by FROM claims WHERE id = ANY($1) AND recorded_by IS NOT NULL \
         UNION SELECT a.created_by FROM claims c JOIN artifacts a ON a.id = c.artifact_id WHERE c.id = ANY($1) \
         UNION SELECT k.uploaded_by FROM claims c JOIN artifact_captures k ON k.artifact_id = c.artifact_id WHERE c.id = ANY($1)",
    )
    .bind(claims)
    .fetch_all(&mut **tx)
    .await?)
}

fn confidence_for(extractor: &str) -> i32 {
    match extractor.split('@').next().unwrap_or("") {
        "csv" | "xlsx" => 9000,
        "pdf" | "dxf" => 8000,
        _ => 5000,
    }
}

#[derive(Serialize)]
pub struct ExtractResp {
    pub run_id: Uuid,
    pub extractor: String,
    pub records: usize,
    pub claims_created: usize,
    pub unmapped_labels: Vec<String>,
    pub warnings: Vec<String>,
}

async fn record_failed_run(
    st: &AppState,
    p: &Principal,
    artifact: Uuid,
    started: OffsetDateTime,
    code: &str,
) -> ApiResult<()> {
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let run = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO extraction_runs (id, tenant_id, artifact_id, extractor, mapping_version, status, error, started_by, started_at) \
         VALUES ($1,$2,$3,'unknown',$4,'failed',$5,$6,$7)",
    )
    .bind(run)
    .bind(p.tenant_id)
    .bind(artifact)
    .bind(MAPPING_VERSION)
    .bind(code)
    .bind(p.user_id)
    .bind(started)
    .execute(&mut *tx)
    .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "extraction.failed",
            "artifact",
            artifact,
            json!({ "run_id": run, "error": code }),
        ),
    )
    .await?;
    tx.commit().await?;
    sec::record(
        &st.pool,
        Some(p.tenant_id),
        Kind::ExtractionFailed,
        Some(p.user_id),
        Some(&p.ip),
        json!({ "artifact_id": artifact, "error": code }),
    )
    .await;
    Ok(())
}

pub async fn extract(State(st): State<AppState>, p: Principal, Path(id): Path<Uuid>) -> ApiResult<Json<ExtractResp>> {
    p.require(&st, Permission::ClaimWrite).await?;
    let (intact, bytes, art) = artifacts::verify_integrity(&st, &p, id).await?;
    if art.status != "accepted" {
        return Err(ApiError::Conflict("artifact_not_released"));
    }
    if !intact {
        return Err(ApiError::Conflict("integrity_check_failed"));
    }
    // I2: the worker accepts at most 64 MiB of base64 JSON (~47 MiB raw).
    if bytes.len() > 40 * 1024 * 1024 {
        return Err(ApiError::BadRequest("document_too_large_for_extraction"));
    }
    // I1: one extraction per artifact at a time (session-level lock, released when this request ends).
    let mut lock_conn = st.pool.acquire().await?;
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1, 19))")
        .bind(format!("{}:{id}", p.tenant_id))
        .fetch_one(&mut *lock_conn)
        .await?;
    if !got {
        return Err(ApiError::Conflict("extraction_in_progress"));
    }
    let result = extract_locked(&st, &p, id, bytes, art).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 19))")
        .bind(format!("{}:{id}", p.tenant_id))
        .execute(&mut *lock_conn)
        .await;
    result
}

async fn extract_locked(
    st: &AppState,
    p: &Principal,
    id: Uuid,
    bytes: Vec<u8>,
    art: artifacts::ArtifactOut,
) -> ApiResult<Json<ExtractResp>> {
    {
        let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
        let done: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM extraction_runs WHERE artifact_id = $1 AND mapping_version = $2 AND status = 'succeeded'",
        )
        .bind(id)
        .bind(MAPPING_VERSION)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        if done.is_some() {
            return Err(ApiError::Conflict("already_extracted"));
        }
    }
    {
        let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
        let suspended = crate::incidents::extraction_suspended(&mut tx).await?;
        tx.commit().await?;
        if suspended {
            return Err(ApiError::Conflict("extraction_suspended"));
        }
    }
    let _permit = st.worker_permits.acquire().await.map_err(ApiError::internal)?;
    let started = OffsetDateTime::now_utc();
    let (out, out_hash) = match extraction::run_worker(&st.cfg, &art.media_type, &bytes).await {
        Ok(r) => r,
        Err(e) => {
            record_failed_run(st, p, id, started, e.code()).await?;
            return Err(ApiError::BadRequest("extraction_failed"));
        }
    };
    if !out.ok {
        let code = out.error.unwrap_or_else(|| "unknown".into());
        record_failed_run(st, p, id, started, &code.chars().take(60).collect::<String>()).await?;
        return Err(ApiError::BadRequest("extraction_failed"));
    }
    let extractor: String = out
        .extractor
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "@-_.".contains(*c))
        .take(40)
        .collect();
    // Locations become integers only (e.g. bbox in whole PDF points) so derivations stay canonical (no floats).
    let mut records = out.records.clone();
    for r in &mut records {
        integerize(&mut r.location);
    }
    let mapped = map_records(&art.kind, &records, confidence_for(&extractor));
    let method = format!("worker:{extractor}+{MAPPING_VERSION}");
    let run = Uuid::now_v7();
    let mut warnings: Vec<String> = out
        .warnings
        .iter()
        .map(|w| w.chars().take(120).collect())
        .take(50)
        .collect();
    warnings.extend(mapped.warnings.iter().take(50).cloned());

    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let mut created = 0usize;
    let mut claim_ids = Vec::new();
    for c in &mapped.claims {
        let v = match evidence_core::claim::validate(c) {
            Ok(v) => v,
            Err(e) => {
                warnings.push(format!("claim skipped: {}", e.code()));
                continue;
            }
        };
        let cid = claims::insert_claim(
            &mut tx,
            p.tenant_id,
            NewClaim {
                claim: &v,
                artifact_id: Some(id),
                valid_from: None,
                valid_to: None,
                extraction_method: &method,
                provenance: json!({
                    "extraction_run": run, "artifact_sha256": art.sha256, "worker_version": out.worker_version,
                    "extractor": extractor, "mapping": MAPPING_VERSION,
                }),
                recorded_by: Some(p.user_id),
            },
        )
        .await?;
        let fresh = claims::fetch(&mut tx, cid).await?;
        crate::decisions::on_claim_change(&mut tx, p.tenant_id, p.user_id, crate::decisions::new_evidence(&fresh))
            .await?;
        claim_ids.push(cid);
        created += 1;
    }
    sqlx::query(
        "INSERT INTO extraction_runs (id, tenant_id, artifact_id, extractor, mapping_version, status, records, claims_created, \
         output_sha256, warnings, started_by, started_at) VALUES ($1,$2,$3,$4,$5,'succeeded',$6,$7,$8,$9,$10,$11)",
    )
    .bind(run)
    .bind(p.tenant_id)
    .bind(id)
    .bind(&extractor)
    .bind(MAPPING_VERSION)
    .bind(out.records.len() as i32)
    .bind(created as i32)
    .bind(out_hash.to_vec())
    .bind(json!({ "warnings": warnings, "unmapped_labels": mapped.unmapped_labels.iter().take(50).collect::<Vec<_>>() }))
    .bind(p.user_id)
    .bind(started)
    .execute(&mut *tx)
    .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "extraction.completed",
            "artifact",
            id,
            json!({ "run_id": run, "extractor": extractor, "claims": created, "output_sha256": hex::encode(out_hash) }),
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(ExtractResp {
        run_id: run,
        extractor,
        records: out.records.len(),
        claims_created: created,
        unmapped_labels: mapped.unmapped_labels,
        warnings,
    }))
}

type FactRow = (
    Uuid,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Value,
    String,
);

/// Loads current, non-rejected part claims as facts, with subjects canonicalised by safe normalisation and
/// human-confirmed aliases.
pub async fn load_facts(tx: &mut Tx, only_subject: Option<&str>) -> ApiResult<Vec<Fact>> {
    load_facts_of(tx, "part", only_subject).await
}

pub async fn load_facts_of(tx: &mut Tx, subject_type: &str, only_subject: Option<&str>) -> ApiResult<Vec<Fact>> {
    let aliases: HashMap<String, String> = sqlx::query_as::<_, (String, String)>(
        "SELECT alias_key, canonical_key FROM entity_aliases WHERE subject_type = $1",
    )
    .bind(subject_type)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .collect();
    let canon = |k: &str| {
        let n = safe_normalize(k);
        aliases.get(&n).cloned().unwrap_or(n)
    };
    let rows: Vec<FactRow> = sqlx::query_as(
        "SELECT c.id, c.subject_key, c.predicate, c.value_text, c.value_num::text, c.value_unit, c.revision, c.scope, \
                CASE WHEN c.extraction_method = 'manual' THEN 'manual' ELSE coalesce(a.kind, 'manual') END FROM claims c LEFT JOIN artifacts a ON a.id = c.artifact_id \
         WHERE c.subject_type = $1 AND c.superseded_at IS NULL AND c.review_status <> 'rejected'",
    )
    .bind(subject_type)
    .fetch_all(&mut **tx)
    .await?;
    let want = only_subject.map(canon);
    Ok(rows
        .into_iter()
        .map(|r| Fact {
            id: r.0.to_string(),
            subject: canon(&r.1),
            predicate: r.2,
            text: r.3,
            num: r.4.as_deref().and_then(|n| Decimal::from_str(n).ok()),
            unit: r.5,
            revision: r.6,
            scope: r
                .7
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default(),
            source_kind: r.8,
        })
        .filter(|f| want.as_ref().is_none_or(|w| &f.subject == w))
        .collect())
}

#[derive(Serialize)]
pub struct RunResp {
    pub rule_version: &'static str,
    pub findings_total: usize,
    pub findings_new: usize,
    pub marked_obsolete: u64,
}

pub async fn run_detection(State(st): State<AppState>, p: Principal) -> ApiResult<Json<RunResp>> {
    p.require(&st, Permission::ClaimWrite).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let resp = detect_in_tx(&mut tx, p.tenant_id).await?;
    audit::append(&mut tx, p.tenant_id, Record::user(p.user_id, "verification.run", "tenant", p.tenant_id,
        json!({ "rule_version": RULE_VERSION, "new": resp.findings_new, "total": resp.findings_total, "obsolete": resp.marked_obsolete }))).await?;
    tx.commit().await?;
    Ok(Json(resp))
}

/// Detects conflicts for the tenant in `tx`. New findings are inserted; findings whose claims are no longer current
/// become `obsolete` (kept, never deleted).
pub async fn detect_in_tx(tx: &mut Tx, tenant: Uuid) -> ApiResult<RunResp> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 13))")
        .bind(tenant.to_string())
        .execute(&mut **tx)
        .await?;
    let facts = load_facts(tx, None).await?;
    let rules = all_rules();
    let refs: Vec<&dyn Rule> = rules.iter().map(|r| r.as_ref()).collect();
    let today = OffsetDateTime::now_utc().date().to_string();
    let findings = conflict_engine::run(&refs, &facts, &Ctx { today });
    let mut new = 0;
    for f in &findings {
        let a = Uuid::parse_str(&f.claim_a).map_err(ApiError::internal)?;
        let b = f
            .claim_b
            .as_deref()
            .map(Uuid::parse_str)
            .transpose()
            .map_err(ApiError::internal)?;
        let r: Option<(Uuid, bool)> = sqlx::query_as(
            "INSERT INTO conflicts (id, tenant_id, kind, subject_type, subject_key, claim_a, claim_b, explanation, rule_version) \
             VALUES ($1,$2,$3,'part',$4,$5,$6,$7,$8) ON CONFLICT (tenant_id, kind, claim_a, coalesce(claim_b, claim_a)) \
             DO UPDATE SET status = 'open', explanation = EXCLUDED.explanation WHERE conflicts.status = 'obsolete' \
             RETURNING id, (xmax = 0) AS inserted",
        )
        .bind(Uuid::now_v7())
        .bind(tenant)
        .bind(f.kind)
        .bind(&f.subject)
        .bind(a)
        .bind(b)
        .bind(&f.explanation)
        .bind(RULE_VERSION)
        .fetch_optional(&mut **tx)
        .await?;
        match r {
            Some((_, true)) => new += 1,
            Some((cid, false)) => {
                // A finding that had become obsolete fires again: reopen it (review finding H2).
                audit::append(
                    tx,
                    tenant,
                    Record {
                        actor_user_id: None,
                        actor_kind: "system",
                        action: "conflict.reopened",
                        target_type: "conflict",
                        target_id: Some(cid.to_string()),
                        details: json!({ "kind": f.kind }),
                    },
                )
                .await?;
                new += 1;
            }
            None => {}
        }
    }
    // Findings that no longer fire under current evidence become obsolete (kept for history, never deleted).
    let current: std::collections::HashSet<(String, String, Option<String>)> = findings
        .iter()
        .map(|f| (f.kind.to_string(), f.claim_a.clone(), f.claim_b.clone()))
        .collect();
    let open: Vec<(Uuid, String, Uuid, Option<Uuid>)> =
        sqlx::query_as("SELECT id, kind, claim_a, claim_b FROM conflicts WHERE status IN ('open', 'needs_evidence')")
            .fetch_all(&mut **tx)
            .await?;
    let stale: Vec<Uuid> = open
        .into_iter()
        .filter(|(_, k, a, b)| !current.contains(&(k.clone(), a.to_string(), b.map(|x| x.to_string()))))
        .map(|(id, ..)| id)
        .collect();
    let obsolete = sqlx::query("UPDATE conflicts SET status = 'obsolete' WHERE id = ANY($1)")
        .bind(&stale)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    Ok(RunResp {
        rule_version: RULE_VERSION,
        findings_total: findings.len(),
        findings_new: new,
        marked_obsolete: obsolete,
    })
}

#[derive(Serialize, sqlx::FromRow)]
pub struct ConflictOut {
    pub id: Uuid,
    pub kind: String,
    pub subject_type: String,
    pub subject_key: String,
    pub claim_a: Uuid,
    pub claim_b: Option<Uuid>,
    pub explanation: Value,
    pub rule_version: String,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub detected_at: OffsetDateTime,
}

const CONFLICT_COLS: &str =
    "id, kind, subject_type, subject_key, claim_a, claim_b, explanation, rule_version, status, detected_at";

#[derive(Deserialize)]
pub struct ConflictQuery {
    pub status: Option<String>,
    pub kind: Option<String>,
    pub subject_key: Option<String>,
}

pub async fn list_conflicts(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<ConflictQuery>,
) -> ApiResult<Json<Vec<ConflictOut>>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, ConflictOut>(&format!(
        "SELECT {CONFLICT_COLS} FROM conflicts WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR kind = $2) \
         AND ($3::text IS NULL OR subject_key = $3) ORDER BY detected_at DESC LIMIT 500"
    ))
    .bind(q.status)
    .bind(q.kind)
    .bind(q.subject_key)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct ReviewOut {
    pub decision: String,
    pub comment: Option<String>,
    pub reviewer: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub decided_at: OffsetDateTime,
}

pub async fn fetch_conflict(tx: &mut Tx, id: Uuid) -> ApiResult<ConflictOut> {
    sqlx::query_as::<_, ConflictOut>(&format!("SELECT {CONFLICT_COLS} FROM conflicts WHERE id = $1"))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)
}

/// Finding detail: what was detected, why, the supporting and contradicting sources, review history and
/// (from M4) decisions that may be affected.
pub async fn get_conflict(State(st): State<AppState>, p: Principal, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let c = fetch_conflict(&mut tx, id).await?;
    let a = claims::fetch(&mut tx, c.claim_a).await?;
    let b = match c.claim_b {
        Some(b) => Some(claims::fetch(&mut tx, b).await?),
        None => None,
    };
    let mut sources = Vec::new();
    for cl in std::iter::once(&a).chain(b.as_ref()) {
        if let Some(aid) = cl.artifact_id {
            let art = artifacts::fetch(&mut tx, aid).await?;
            sources.push(json!({ "claim_id": cl.id, "artifact_id": aid, "display_name": art.display_name, "sha256": art.sha256, "location": cl.source_location }));
        }
    }
    let reviews = sqlx::query_as::<_, ReviewOut>(
        "SELECT decision, comment, reviewer, decided_at FROM review_items WHERE target_type = 'conflict' AND target_id = $1 ORDER BY decided_at",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    let affected = crate::decisions::affected_by_claims(
        &mut tx,
        &std::iter::once(c.claim_a).chain(c.claim_b).collect::<Vec<_>>(),
    )
    .await?;
    tx.commit().await?;
    let min_conf = std::iter::once(&a)
        .chain(b.as_ref())
        .map(|x| x.confidence_bps)
        .min()
        .unwrap_or(0);
    let needs_review = c.status == "open" || c.status == "needs_evidence";
    Ok(Json(json!({
        "conflict": c,
        "claim_a": a,
        "claim_b": b,
        "sources": sources,
        "reviews": reviews,
        "requires_human_review": needs_review,
        "potentially_affected_decisions": affected,
        "limitations": format!("Deterministic rule {}; extracted values may contain extraction errors (lowest source confidence {} bps); a matching hash proves only that a document is unchanged since capture.", RULE_VERSION, min_conf),
    })))
}

#[derive(Deserialize)]
pub struct ReviewReq {
    pub decision: String,
    pub comment: Option<String>,
    /// For confirming (rejecting) a finding: which of its claims are wrong.
    pub faulty_claims: Option<Vec<Uuid>>,
}

fn check_review(req: &ReviewReq) -> ApiResult<(&'static str, Option<String>)> {
    let decision = match req.decision.as_str() {
        "approve" => "approve",
        "reject" => "reject",
        "request_evidence" => "request_evidence",
        _ => return Err(ApiError::BadRequest("invalid_decision")),
    };
    let comment = req
        .comment
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_owned);
    if comment.as_ref().is_some_and(|c| c.chars().count() > 2000) {
        return Err(ApiError::BadRequest("comment_too_long"));
    }
    if decision != "request_evidence" && comment.is_none() {
        return Err(ApiError::BadRequest("comment_required"));
    }
    Ok((decision, comment))
}

async fn insert_review(
    tx: &mut Tx,
    p: &Principal,
    target_type: &str,
    target: Uuid,
    decision: &str,
    comment: &Option<String>,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO review_items (id, tenant_id, target_type, target_id, decision, comment, reviewer) VALUES ($1,$2,$3,$4,$5,$6,$7)")
        .bind(Uuid::now_v7())
        .bind(p.tenant_id)
        .bind(target_type)
        .bind(target)
        .bind(decision)
        .bind(comment)
        .bind(p.user_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// approve = reviewer accepts proceeding despite the finding (documented waiver);
/// reject = the finding is confirmed and the evidence/item is rejected; request_evidence = more evidence needed.
pub async fn review_conflict(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(req): ApiJson<ReviewReq>,
) -> ApiResult<Json<ConflictOut>> {
    p.require(&st, Permission::ReviewDecide).await?;
    let (decision, comment) = check_review(&req)?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let row: Option<(String, Uuid, Option<Uuid>)> =
        sqlx::query_as("SELECT status, claim_a, claim_b FROM conflicts WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let (status, ca, cb) = row.ok_or(ApiError::NotFound)?;
    let involved: Vec<Uuid> = std::iter::once(ca).chain(cb).collect();
    let owners = claim_owners(&mut tx, &involved).await?;
    p.require_on(
        &st,
        Permission::ReviewDecide,
        &Resource {
            tenant_id: p.tenant_id,
            kind: "conflict",
            owner_user_id: owners.contains(&p.user_id).then_some(p.user_id),
        },
    )
    .await?;
    if !matches!(status.as_str(), "open" | "needs_evidence") {
        return Err(ApiError::Conflict("conflict_not_open"));
    }
    // Confirming a finding must name the faulty evidence, which is then rejected and withdrawn from decisions (M7).
    let faulty: Vec<Uuid> = req.faulty_claims.clone().unwrap_or_default();
    if decision == "reject" && (faulty.is_empty() || !faulty.iter().all(|f| involved.contains(f))) {
        return Err(ApiError::BadRequest("faulty_claims_required"));
    }
    let new_status = match decision {
        "approve" => "resolved_approved",
        "reject" => "resolved_rejected",
        _ => "needs_evidence",
    };
    insert_review(&mut tx, &p, "conflict", id, decision, &comment).await?;
    for f in &faulty {
        let cur: String = sqlx::query_scalar("SELECT review_status FROM claims WHERE id = $1 FOR UPDATE")
            .bind(f)
            .fetch_one(&mut *tx)
            .await?;
        if cur != "rejected" {
            insert_review(&mut tx, &p, "claim", *f, "reject", &comment).await?;
            sqlx::query("UPDATE claims SET review_status = 'rejected' WHERE id = $1")
                .bind(f)
                .execute(&mut *tx)
                .await?;
            crate::decisions::on_claim_change(
                &mut tx,
                p.tenant_id,
                p.user_id,
                dependency_engine::Change::Withdrawn {
                    old_claim: f.to_string(),
                    reason: format!("rejected when finding {id} was confirmed"),
                },
            )
            .await?;
        }
    }
    sqlx::query("UPDATE conflicts SET status = $2 WHERE id = $1")
        .bind(id)
        .bind(new_status)
        .execute(&mut *tx)
        .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "review.conflict",
            "conflict",
            id,
            json!({ "decision": decision, "from": status, "to": new_status }),
        ),
    )
    .await?;
    let out = fetch_conflict(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

pub async fn review_claim(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(req): ApiJson<ReviewReq>,
) -> ApiResult<Json<ClaimOut>> {
    let (decision, comment) = check_review(&req)?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let row: Option<(Option<OffsetDateTime>, String)> =
        sqlx::query_as("SELECT superseded_at, review_status FROM claims WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let (superseded, current) = row.ok_or(ApiError::NotFound)?;
    let owners = claim_owners(&mut tx, &[id]).await?;
    p.require_on(
        &st,
        Permission::ReviewDecide,
        &Resource {
            tenant_id: p.tenant_id,
            kind: "claim",
            owner_user_id: owners.contains(&p.user_id).then_some(p.user_id),
        },
    )
    .await?;
    if superseded.is_some() {
        return Err(ApiError::Conflict("claim_superseded"));
    }
    // A rejected claim is final; corrections go through supersession (review finding H2).
    if current == "rejected" {
        return Err(ApiError::Conflict("claim_rejected_supersede_instead"));
    }
    let status = match decision {
        "approve" => "accepted",
        "reject" => "rejected",
        _ => "needs_evidence",
    };
    insert_review(&mut tx, &p, "claim", id, decision, &comment).await?;
    sqlx::query("UPDATE claims SET review_status = $2 WHERE id = $1")
        .bind(id)
        .bind(status)
        .execute(&mut *tx)
        .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "review.claim",
            "claim",
            id,
            json!({ "decision": decision, "review_status": status }),
        ),
    )
    .await?;
    let out = claims::fetch(&mut tx, id).await?;
    if status == "rejected" {
        crate::decisions::on_claim_change(
            &mut tx,
            p.tenant_id,
            p.user_id,
            dependency_engine::Change::Withdrawn {
                old_claim: id.to_string(),
                reason: "rejected in review".into(),
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct EntityQuery {
    pub subject_type: Option<String>,
}

pub async fn entity_candidates(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<EntityQuery>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimRead).await?;
    let stype = q.subject_type.unwrap_or_else(|| "part".into());
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT subject_key FROM claims WHERE subject_type = $1 AND superseded_at IS NULL LIMIT 5000",
    )
    .bind(&stype)
    .fetch_all(&mut *tx)
    .await?;
    let decided: Vec<(String, String)> =
        sqlx::query_as("SELECT left_key, right_key FROM entity_decisions WHERE subject_type = $1")
            .bind(&stype)
            .fetch_all(&mut *tx)
            .await?;
    tx.commit().await?;
    let pending: Vec<_> = candidates(&keys)
        .into_iter()
        .filter(|c| c.outcome != Outcome::Match)
        .filter(|c| {
            !decided
                .iter()
                .any(|(l, r)| (l == &c.left && r == &c.right) || (l == &c.right && r == &c.left))
        })
        .collect();
    Ok(Json(
        json!({ "subject_type": stype, "candidates": pending, "note": "Possible matches are never merged automatically." }),
    ))
}

#[derive(Deserialize)]
pub struct EntityDecisionReq {
    pub subject_type: String,
    pub left_key: String,
    pub right_key: String,
    pub outcome: String,
}

pub async fn entity_decision(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<EntityDecisionReq>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ReviewDecide).await?;
    if !matches!(req.outcome.as_str(), "same" | "different") {
        return Err(ApiError::BadRequest("invalid_outcome"));
    }
    let (l, r) = (safe_normalize(&req.left_key), safe_normalize(&req.right_key));
    if l.is_empty()
        || r.is_empty()
        || l == r
        || l.len() > 200
        || r.len() > 200
        || !matches!(req.subject_type.as_str(), "part" | "supplier")
    {
        return Err(ApiError::BadRequest("invalid_keys"));
    }
    let (_, reasons) = entity_resolution::compare(&l, &r);
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO entity_decisions (id, tenant_id, subject_type, left_key, right_key, outcome, reasons, decided_by) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(id)
        .bind(p.tenant_id)
        .bind(&req.subject_type)
        .bind(&l)
        .bind(&r)
        .bind(&req.outcome)
        .bind(json!(reasons))
        .bind(p.user_id)
        .execute(&mut *tx)
        .await?;
    if req.outcome == "same" {
        let canonical: String =
            sqlx::query_scalar("SELECT canonical_key FROM entity_aliases WHERE subject_type = $1 AND alias_key = $2")
                .bind(&req.subject_type)
                .bind(&l)
                .fetch_optional(&mut *tx)
                .await?
                .unwrap_or(l.clone());
        if canonical == r {
            return Err(ApiError::Conflict("alias_cycle"));
        }
        // Keys that pointed at `r` now point at the merged identity, so chains never split entities (L2).
        sqlx::query("UPDATE entity_aliases SET canonical_key = $3 WHERE subject_type = $1 AND canonical_key = $2")
            .bind(&req.subject_type)
            .bind(&r)
            .bind(&canonical)
            .execute(&mut *tx)
            .await?;
        let ins = sqlx::query(
            "INSERT INTO entity_aliases (tenant_id, subject_type, alias_key, canonical_key, decision_id) VALUES ($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING",
        )
        .bind(p.tenant_id)
        .bind(&req.subject_type)
        .bind(&r)
        .bind(&canonical)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if ins.rows_affected() == 0 {
            return Err(ApiError::Conflict("alias_already_mapped"));
        }
    }
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "entity.decision",
            "entity",
            &r,
            json!({ "left": l, "right": r, "outcome": req.outcome }),
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "id": id, "outcome": req.outcome })))
}

#[derive(Deserialize)]
pub struct CompareQuery {
    pub from: String,
    pub to: String,
}

pub async fn compare(
    State(st): State<AppState>,
    p: Principal,
    Path((stype, key)): Path<(String, String)>,
    ApiQuery(q): ApiQuery<CompareQuery>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimRead).await?;
    if stype.is_empty() || stype.len() > 40 || !stype.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
        return Err(ApiError::BadRequest("invalid_subject_type"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let facts = load_facts_of(&mut tx, &stype, Some(&key)).await?;
    tx.commit().await?;
    let diff = compare_revisions(&facts, &q.from, &q.to);
    let by_rev: BTreeMap<String, usize> = facts.iter().fold(BTreeMap::new(), |mut m, f| {
        *m.entry(f.revision.clone().unwrap_or_else(|| "(none)".into()))
            .or_default() += 1;
        m
    });
    Ok(Json(
        json!({ "subject_key": safe_normalize(&key), "from": q.from, "to": q.to, "differences": diff, "claims_per_revision": by_rev }),
    ))
}

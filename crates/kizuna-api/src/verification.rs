//! M3 verification workflow: extraction of claims from documents, entity resolution decisions, conflict detection,
//! revision comparison and human review. Rules live in `domain-manufacturing`; this module does I/O and audit.

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

use axum::Json;
use axum::extract::{Path, State};
use conflict_engine::{Ctx, Fact, Rule};
use domain_manufacturing::{MAPPING_VERSION, RULE_VERSION, all_rules, compare_revisions, map_records_with};
use entity_resolution::v2::{PartContext, TypoReview};
use entity_resolution::{ErVersion, Outcome, candidates, safe_normalize};
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
use crate::simulations::Overlay;

/// ABAC: nobody reviews a claim they entered by hand (four-eyes principle).
pub struct NoSelfReview;

impl AttributeRule for NoSelfReview {
    fn deny(&self, s: &Subject, p: Permission, r: &Resource<'_>) -> Option<&'static str> {
        (matches!(p, Permission::ReviewDecide | Permission::IncidentManage) && r.owner_user_id == Some(s.user_id))
            .then_some("no_self_review")
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
    canonical_key_of(tx, "part", key).await
}

/// Canonical identity of a key of any subject type.
pub async fn canonical_key_of(tx: &mut Tx, subject_type: &str, key: &str) -> ApiResult<String> {
    let n = safe_normalize(key);
    let c: Option<String> =
        sqlx::query_scalar("SELECT canonical_key FROM entity_aliases WHERE subject_type = $1 AND alias_key = $2")
            .bind(subject_type)
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

/// Identity of an extracted claim across runs: subject, predicate, scope and origin (rules vs AI).
fn identity(subject_type: &str, subject_key: &str, predicate: &str, scope: &Value, ai: bool) -> String {
    json!([subject_type, subject_key, predicate, scope, ai]).to_string()
}

fn same_value(o: &ClaimOut, v: &evidence_core::claim::ValidClaim) -> bool {
    let num = |s: &Option<String>| {
        s.as_deref()
            .and_then(|x| Decimal::from_str(x).ok())
            .map(|d| d.normalize())
    };
    o.value_text == v.value_text
        && num(&o.value_num) == num(&v.value_num)
        && o.value_unit == v.value_unit
        && o.revision == v.revision
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
    /// Of `claims_created`, stored as `ai_suggested` (0 unless the tenant opted in to AI-assisted extraction).
    pub ai_claims_suggested: usize,
    pub ai_model: Option<String>,
    /// Version of the tenant's import settings used (0 = defaults).
    pub import_settings_version: i32,
    /// Re-extraction only: the run this one supersedes and what happened to the old claims.
    pub supersedes_run: Option<Uuid>,
    pub claims_superseded: usize,
    pub claims_unchanged: usize,
    pub claims_not_reconfirmed: usize,
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
    extract_common(st, p, id, None).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReextractReq {
    pub reason: String,
}

/// Explicit re-extraction (e.g. after an import-settings change): a new run that supersedes the current one. New
/// claims supersede changed old claims (old versions stay queryable); unchanged claims are kept; old claims the new
/// run no longer finds become `needs_evidence`. Dependent decisions go through selective invalidation. Audited.
pub async fn reextract(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(req): ApiJson<ReextractReq>,
) -> ApiResult<Json<ExtractResp>> {
    let reason = req.reason.trim().to_string();
    if reason.is_empty() || reason.chars().count() > 500 {
        return Err(ApiError::BadRequest("reason_required"));
    }
    extract_common(st, p, id, Some(reason)).await
}

async fn extract_common(st: AppState, p: Principal, id: Uuid, re: Option<String>) -> ApiResult<Json<ExtractResp>> {
    p.require(&st, Permission::ClaimWrite).await?;
    let (intact, bytes, art) = artifacts::verify_integrity(&st, &p, id).await?;
    if art.status != "accepted" {
        return Err(ApiError::Conflict("artifact_not_released"));
    }
    if !intact {
        return Err(ApiError::Conflict("integrity_check_failed"));
    }
    // A document that keeps failing is not retried endlessly (final review M2).
    {
        let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
        let fails: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM extraction_runs WHERE artifact_id = $1 AND status = 'failed' AND finished_at > now() - interval '1 hour'",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        if fails >= 3 {
            return Err(ApiError::TooManyRequests);
        }
    }
    // I2: the worker accepts at most 64 MiB of base64 JSON (~47 MiB raw).
    if bytes.len() > 40 * 1024 * 1024 {
        return Err(ApiError::BadRequest("document_too_large_for_extraction"));
    }
    // I1: one extraction per artifact at a time. A lease row (not a session lock) so no pooled connection is held
    // while the worker and the AI step run; it expires by itself if the request is dropped.
    let holder = Uuid::now_v7();
    let got: Option<i32> = {
        let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
        let r = sqlx::query_scalar(
            "INSERT INTO extraction_leases (tenant_id, artifact_id, holder, expires_at) \
             VALUES ($1, $2, $3, now() + interval '120 seconds') \
             ON CONFLICT (tenant_id, artifact_id) DO UPDATE SET holder = EXCLUDED.holder, expires_at = EXCLUDED.expires_at \
             WHERE extraction_leases.expires_at < now() RETURNING 1",
        )
        .bind(p.tenant_id)
        .bind(id)
        .bind(holder)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        r
    };
    if got.is_none() {
        return Err(ApiError::Conflict("extraction_in_progress"));
    }
    let mut lease = Lease {
        pool: st.pool.clone(),
        tenant: p.tenant_id,
        artifact: id,
        holder,
        released: false,
    };
    let result = extract_locked(&st, &p, id, bytes, art, re).await;
    lease.release().await;
    result
}

/// Releases the extraction lease when the request finishes (or is dropped).
struct Lease {
    pool: sqlx::PgPool,
    tenant: Uuid,
    artifact: Uuid,
    holder: Uuid,
    released: bool,
}

async fn release_lease(pool: sqlx::PgPool, t: Uuid, a: Uuid, h: Uuid) {
    if let Ok(mut tx) = db::tenant_tx(&pool, t).await {
        let _ = sqlx::query("DELETE FROM extraction_leases WHERE artifact_id = $1 AND holder = $2")
            .bind(a)
            .bind(h)
            .execute(&mut *tx)
            .await;
        let _ = tx.commit().await;
    }
}

impl Lease {
    async fn release(&mut self) {
        self.released = true;
        release_lease(self.pool.clone(), self.tenant, self.artifact, self.holder).await;
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // Request dropped mid-way (timeout/cancel): release in the background; the expiry is the last resort.
        if !self.released {
            tokio::spawn(release_lease(
                self.pool.clone(),
                self.tenant,
                self.artifact,
                self.holder,
            ));
        }
    }
}

async fn extract_locked(
    st: &AppState,
    p: &Principal,
    id: Uuid,
    bytes: Vec<u8>,
    art: artifacts::ArtifactOut,
    re: Option<String>,
) -> ApiResult<Json<ExtractResp>> {
    // The current run: succeeded and not superseded by a later re-extraction.
    let prev_run = {
        let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
        let done: Option<Uuid> = sqlx::query_scalar(
            "SELECT r.id FROM extraction_runs r WHERE r.artifact_id = $1 AND r.mapping_version = $2 AND r.status = 'succeeded' \
             AND NOT EXISTS (SELECT 1 FROM extraction_runs s WHERE s.supersedes_run = r.id)",
        )
        .bind(id)
        .bind(MAPPING_VERSION)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        match (done, &re) {
            (Some(_), None) => return Err(ApiError::Conflict("already_extracted")),
            (None, Some(_)) => return Err(ApiError::Conflict("not_extracted")),
            (d, _) => d,
        }
    };
    {
        let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
        let suspended = crate::incidents::extraction_suspended(&mut tx).await?;
        tx.commit().await?;
        if suspended {
            return Err(ApiError::Conflict("extraction_suspended"));
        }
    }
    let permit = st.worker_permits.acquire().await.map_err(ApiError::internal)?;
    let started = OffsetDateTime::now_utc();
    // The tenant's saved import settings (latest version) drive the worker and the long-table mapping.
    // The tenant's learnt label synonyms (accepted fallback mappings) apply to this tenant only.
    let (import, synonyms) = {
        let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
        let row: Option<(i32, Value)> =
            sqlx::query_as("SELECT version, settings FROM import_settings ORDER BY version DESC LIMIT 1")
                .fetch_optional(&mut *tx)
                .await?;
        let synonyms = crate::label_synonyms::load(&mut tx).await?;
        tx.commit().await?;
        let (v, s) = row.unwrap_or((0, json!({})));
        (extraction::import_options(v, &s, &art.media_type), synonyms)
    };
    let (out, out_hash) = match extraction::run_worker(&st.cfg, &art.media_type, &bytes, &import).await {
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
    // The sandbox slot is released before the (slow, external) AI step (review M5-M7 L6).
    drop(permit);
    if let Some(m) = &import.mapping {
        records = domain_manufacturing::apply_column_mapping(&records, m);
    }
    let mapped = map_records_with(&art.kind, &records, confidence_for(&extractor), &synonyms);
    let method = format!("worker:{extractor}+{MAPPING_VERSION}");
    let run = Uuid::now_v7();
    let mut warnings: Vec<String> = out
        .warnings
        .iter()
        .map(|w| w.chars().take(120).collect())
        .take(50)
        .collect();
    warnings.extend(mapped.warnings.iter().take(50).cloned());
    warnings.extend(import.ignored.iter().map(|k| format!("import_setting_ignored:{k}")));
    // Optional AI label mapping on the already-extracted records (never on raw document bytes). Any failure keeps
    // the deterministic result only.
    let outcome = crate::ai::suggest(&st.pool, st.cfg.ai.as_ref(), p.tenant_id, id, &art.kind, &records).await?;
    if !outcome.refused.is_empty() {
        // Instruction-like document text aimed at the AI: refused, never sent, raised as a security event. Only
        // hashes of the labels are logged (no document text in the security log).
        warnings.push(format!("ai_label_refused:{}", outcome.refused.len()));
        let hashes: Vec<String> = outcome
            .refused
            .iter()
            .map(|l| hex::encode(&crate::crypto::sha256(l.as_bytes())[..8]))
            .collect();
        sec::record(
            &st.pool,
            Some(p.tenant_id),
            Kind::PromptInjectionSuspected,
            Some(p.user_id),
            Some(&p.ip),
            json!({ "artifact_id": id, "labels": outcome.refused.len(), "label_sha256_prefixes": hashes }),
        )
        .await;
    }
    let ai = match outcome.result {
        Ok(s) => s,
        Err(code) => {
            warnings.push(format!("ai_fallback:{code}"));
            let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
            let model = st.cfg.ai.as_ref().map(|a| a.model.clone());
            audit::append(
                &mut tx,
                p.tenant_id,
                Record::user(
                    p.user_id,
                    "extraction.ai_call",
                    "artifact",
                    id,
                    json!({ "status": code, "model": model, "fallback": MAPPING_VERSION }),
                ),
            )
            .await?;
            tx.commit().await?;
            None
        }
    };
    if let Some(a) = &ai {
        warnings.extend(a.dropped.iter().take(50).map(|d| format!("ai_mapping_dropped:{d}")));
    }
    let ai_method = ai.as_ref().map(|a| crate::ai::method(&a.model)).unwrap_or_default();
    let mut all: Vec<(&evidence_core::claim::ClaimInput, bool)> = mapped.claims.iter().map(|c| (c, false)).collect();
    if let Some(a) = &ai {
        all.extend(a.claims.iter().map(|c| (c, true)));
    }

    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let mut created = 0usize;
    let mut ai_created = 0usize;
    let mut claim_ids = Vec::new();
    let (mut superseded, mut unchanged) = (0usize, 0usize);
    // Re-extraction: current extracted (not manual) claims of this artifact, by identity.
    let mut old: BTreeMap<String, Vec<ClaimOut>> = BTreeMap::new();
    if prev_run.is_some() {
        let rows = sqlx::query_as::<_, ClaimOut>(&format!(
            "SELECT {} FROM claims WHERE artifact_id = $1 AND superseded_at IS NULL AND extraction_method <> 'manual' \
             ORDER BY recorded_at FOR UPDATE",
            claims::CLAIM_COLS
        ))
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        for c in rows {
            let k = identity(
                &c.subject_type,
                &c.subject_key,
                &c.predicate,
                &c.scope,
                c.extraction_method.starts_with("ai:"),
            );
            old.entry(k).or_default().push(c);
        }
    }
    let reason = re.clone().unwrap_or_default();
    for (c, is_ai) in all {
        let v = match evidence_core::claim::validate(c) {
            Ok(v) => v,
            Err(e) => {
                warnings.push(format!("claim skipped: {}", e.code()));
                continue;
            }
        };
        let prior = old
            .get_mut(&identity(
                &v.subject_type,
                &v.subject_key,
                &v.predicate,
                &v.scope,
                is_ai,
            ))
            .and_then(|l| (!l.is_empty()).then(|| l.remove(0)));
        if let Some(o) = &prior
            && same_value(o, &v)
        {
            unchanged += 1;
            continue;
        }
        let cid = claims::insert_claim(
            &mut tx,
            p.tenant_id,
            NewClaim {
                claim: &v,
                artifact_id: Some(id),
                valid_from: None,
                valid_to: None,
                extraction_method: if is_ai { &ai_method } else { &method },
                provenance: match (&ai, is_ai) {
                    (Some(a), true) => json!({
                        "extraction_run": run, "artifact_sha256": art.sha256, "worker_version": out.worker_version,
                        "extractor": extractor, "mapping": MAPPING_VERSION, "import_settings_version": import.version,
                        "ai": { "model": a.model, "mapper": domain_manufacturing::ai::AI_MAPPER_VERSION,
                                "prompt": domain_manufacturing::ai::SYSTEM_PROMPT_VERSION,
                                "prompt_sha256": hex::encode(a.prompt_sha256) },
                    }),
                    _ => json!({
                        "extraction_run": run, "artifact_sha256": art.sha256, "worker_version": out.worker_version,
                        "extractor": extractor, "mapping": MAPPING_VERSION, "import_settings_version": import.version,
                    }),
                },
                recorded_by: Some(p.user_id),
                ai_suggested: is_ai,
            },
        )
        .await?;
        if let Some(o) = &prior {
            sqlx::query("UPDATE claims SET superseded_at = now(), superseded_by = $2 WHERE id = $1")
                .bind(o.id)
                .bind(cid)
                .execute(&mut *tx)
                .await?;
            audit::append(
                &mut tx,
                p.tenant_id,
                Record::user(
                    p.user_id,
                    "claim.superseded",
                    "claim",
                    o.id,
                    json!({ "by": cid, "reason": reason, "extraction_run": run, "via": "reextract" }),
                ),
            )
            .await?;
            superseded += 1;
            let fresh = claims::fetch(&mut tx, cid).await?;
            let change = if !is_ai {
                Some(dependency_engine::Change::Superseded {
                    old_claim: o.id.to_string(),
                    new_claim: cid.to_string(),
                    new: crate::decisions::snapshot_of(&fresh),
                })
            } else if o.review_status == "accepted" || !o.extraction_method.starts_with("ai:") {
                // A reviewed claim replaced by an unreviewed suggestion is no longer evidence.
                Some(dependency_engine::Change::Withdrawn {
                    old_claim: o.id.to_string(),
                    reason: "superseded by an unreviewed AI suggestion on re-extraction".into(),
                })
            } else {
                None
            };
            if let Some(ch) = change {
                crate::decisions::on_claim_change(&mut tx, p.tenant_id, p.user_id, ch).await?;
            }
        } else if !is_ai {
            // AI suggestions do not touch decisions until a reviewer accepts them.
            let fresh = claims::fetch(&mut tx, cid).await?;
            crate::decisions::on_claim_change(&mut tx, p.tenant_id, p.user_id, crate::decisions::new_evidence(&fresh))
                .await?;
        }
        claim_ids.push(cid);
        created += 1;
        ai_created += usize::from(is_ai);
    }
    // Old claims the new run no longer produces: kept, but flagged for review and withdrawn as evidence.
    let mut not_reconfirmed = 0usize;
    for o in old.into_values().flatten() {
        // Already rejected or already flagged (e.g. by an earlier re-extraction): nothing new to record.
        if matches!(o.review_status.as_str(), "rejected" | "needs_evidence") {
            continue;
        }
        not_reconfirmed += 1;
        if o.review_status == "accepted" || !o.extraction_method.starts_with("ai:") {
            sqlx::query("UPDATE claims SET review_status = 'needs_evidence' WHERE id = $1")
                .bind(o.id)
                .execute(&mut *tx)
                .await?;
            crate::decisions::on_claim_change(
                &mut tx,
                p.tenant_id,
                p.user_id,
                dependency_engine::Change::Withdrawn {
                    old_claim: o.id.to_string(),
                    reason: "not found on re-extraction".into(),
                },
            )
            .await?;
        }
        audit::append(
            &mut tx,
            p.tenant_id,
            Record::user(
                p.user_id,
                "claim.not_reconfirmed",
                "claim",
                o.id,
                json!({ "reason": reason, "extraction_run": run, "previous_status": o.review_status }),
            ),
        )
        .await?;
    }
    if let Some(a) = &ai {
        audit::append(
            &mut tx,
            p.tenant_id,
            Record::user(
                p.user_id,
                "extraction.ai_call",
                "artifact",
                id,
                json!({
                    "status": "ok", "model": a.model, "prompt_sha256": hex::encode(a.prompt_sha256),
                    "labels_sent": a.labels_sent, "mappings": a.mappings, "claims_suggested": ai_created,
                    "tokens": a.tokens, "duration_ms": a.duration_ms as u64,
                }),
            ),
        )
        .await?;
    }
    sqlx::query(
        "INSERT INTO extraction_runs (id, tenant_id, artifact_id, extractor, mapping_version, status, records, claims_created, \
         output_sha256, warnings, started_by, started_at, supersedes_run, import_settings_version) \
         VALUES ($1,$2,$3,$4,$5,'succeeded',$6,$7,$8,$9,$10,$11,$12,$13)",
    )
    .bind(run)
    .bind(p.tenant_id)
    .bind(id)
    .bind(&extractor)
    .bind(MAPPING_VERSION)
    .bind(out.records.len() as i32)
    .bind(created as i32)
    .bind(out_hash.to_vec())
    .bind(json!({ "warnings": warnings, "import_settings_version": import.version, "unmapped_labels": mapped.unmapped_labels.iter().take(50).collect::<Vec<_>>() }))
    .bind(p.user_id)
    .bind(started)
    .bind(prev_run)
    .bind(import.version)
    .execute(&mut *tx)
    .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            if prev_run.is_some() {
                "extraction.reextracted"
            } else {
                "extraction.completed"
            },
            "artifact",
            id,
            json!({
                "run_id": run, "extractor": extractor, "claims": created, "output_sha256": hex::encode(out_hash),
                "supersedes_run": prev_run, "reason": re, "import_settings_version": import.version,
                "superseded": superseded, "unchanged": unchanged, "not_reconfirmed": not_reconfirmed,
            }),
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(ExtractResp {
        run_id: run,
        extractor,
        records: out.records.len(),
        claims_created: created,
        ai_claims_suggested: ai_created,
        import_settings_version: import.version,
        supersedes_run: prev_run,
        claims_superseded: superseded,
        claims_unchanged: unchanged,
        claims_not_reconfirmed: not_reconfirmed,
        ai_model: ai.as_ref().map(|a| a.model.clone()),
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
    load_facts_with(tx, subject_type, only_subject, &Overlay::default()).await
}

fn fact_scope(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// [`load_facts_of`] with a simulated change applied (`ov`: claims that leave the evidence, a claim that joins it).
pub(crate) async fn load_facts_with(
    tx: &mut Tx,
    subject_type: &str,
    only_subject: Option<&str>,
    ov: &Overlay,
) -> ApiResult<Vec<Fact>> {
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
         WHERE c.subject_type = $1 AND c.superseded_at IS NULL AND c.review_status <> 'rejected' \
         AND (c.extraction_method NOT LIKE 'ai:%' OR c.review_status = 'accepted')",
    )
    .bind(subject_type)
    .fetch_all(&mut **tx)
    .await?;
    let want = only_subject.map(canon);
    let mut facts: Vec<Fact> = rows
        .into_iter()
        .filter(|r| ov.keeps(&r.0))
        .map(|r| Fact {
            id: r.0.to_string(),
            subject: canon(&r.1),
            predicate: r.2,
            text: r.3,
            num: r.4.as_deref().and_then(|n| Decimal::from_str(n).ok()),
            unit: r.5,
            revision: r.6,
            scope: fact_scope(&r.7),
            source_kind: r.8,
        })
        .collect();
    // A simulated correction is a manual claim (supersession always records one), current and unreviewed.
    if let Some(s) = &ov.added
        && s.claim.subject_type == subject_type
    {
        let c = &s.claim;
        facts.push(Fact {
            id: c.id.to_string(),
            subject: canon(&c.subject_key),
            predicate: c.predicate.clone(),
            text: c.value_text.clone(),
            num: c.value_num.as_deref().and_then(|n| Decimal::from_str(n).ok()),
            unit: c.value_unit.clone(),
            revision: c.revision.clone(),
            scope: fact_scope(&c.scope),
            source_kind: "manual".into(),
        });
    }
    facts.retain(|f| want.as_ref().is_none_or(|w| &f.subject == w));
    Ok(facts)
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
    let resp = detect_in_tx(&mut tx, p.tenant_id, st.clock.now()).await?;
    audit::append(&mut tx, p.tenant_id, Record::user(p.user_id, "verification.run", "tenant", p.tenant_id,
        json!({ "rule_version": RULE_VERSION, "new": resp.findings_new, "total": resp.findings_total, "obsolete": resp.marked_obsolete }))).await?;
    tx.commit().await?;
    Ok(Json(resp))
}

/// Detects conflicts for the tenant in `tx`. New findings are inserted; findings whose claims are no longer current
/// become `obsolete` (kept, never deleted).
pub async fn detect_in_tx(tx: &mut Tx, tenant: Uuid, now: OffsetDateTime) -> ApiResult<RunResp> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 13))")
        .bind(tenant.to_string())
        .execute(&mut **tx)
        .await?;
    let facts = load_facts(tx, None).await?;
    let rules = all_rules();
    let refs: Vec<&dyn Rule> = rules.iter().map(|r| r.as_ref()).collect();
    // Expiry is judged on the tenant's calendar day at the injected clock's "now", not the server's UTC date.
    let today = db::tenant_today(tx, tenant, now).await?.to_string();
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
    let row: Option<(Option<OffsetDateTime>, String, String)> =
        sqlx::query_as("SELECT superseded_at, review_status, extraction_method FROM claims WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let (superseded, current, method) = row.ok_or(ApiError::NotFound)?;
    let ai_origin = method.starts_with("ai:");
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
        // AI provenance is sticky: only approve or reject moves a suggestion out of `ai_suggested` (M5-M7 M1).
        _ if current == "ai_suggested" => "ai_suggested",
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
    // Accepting a fallback-mapped worker claim teaches this tenant the label (migration 0022).
    if status == "accepted" && current != "accepted" {
        crate::label_synonyms::learn(&mut tx, &p, id, &method, &out.source_location).await?;
    }
    // AI-origin claims are evidence only while accepted: entering `accepted` notifies dependents as new evidence,
    // leaving it (request evidence) withdraws it.
    if ai_origin && status == "accepted" && current != "accepted" {
        crate::decisions::on_claim_change(&mut tx, p.tenant_id, p.user_id, crate::decisions::new_evidence(&out))
            .await?;
    }
    if ai_origin && current == "accepted" && status == "needs_evidence" {
        crate::decisions::on_claim_change(
            &mut tx,
            p.tenant_id,
            p.user_id,
            dependency_engine::Change::Withdrawn {
                old_claim: id.to_string(),
                reason: "AI-origin claim no longer accepted".into(),
            },
        )
        .await?;
    }
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
    /// "v1" or "v2"; defaults to the server's KIZUNA_ER_VERSION.
    pub version: Option<String>,
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
    let version = match q.version.as_deref() {
        None => st.cfg.er_version,
        Some(v) => ErVersion::parse(v).ok_or(ApiError::BadRequest("invalid_version"))?,
    };
    let typo_review = crate::tenant_settings::typo_review(&mut tx).await?;
    let context = if version == ErVersion::V2 && stype == "part" && typo_review == TypoReview::Strict {
        part_context(&mut tx).await?
    } else {
        HashMap::new()
    };
    tx.commit().await?;
    let undecided = |l: &str, r: &str| !decided.iter().any(|(a, b)| (a == l && b == r) || (a == r && b == l));
    let (pending, stats) = match version {
        // v1: rule outcomes; automatic `Match` pairs (safe normalisation) are not listed for review.
        ErVersion::V1 => {
            let c: Vec<Value> = candidates(&keys)
                .into_iter()
                .filter(|c| c.outcome != Outcome::Match && undecided(&c.left, &c.right))
                .map(|c| json!(c))
                .collect();
            (c, Value::Null)
        }
        // v2: every non-safe pair is listed, including FS `match` (strong evidence still needs a human decision).
        ErVersion::V2 => {
            let res = entity_resolution::v2::resolve_with_context(
                entity_resolution::v2::Kind::for_subject_type(&stype),
                &keys,
                &context,
                &entity_resolution::v2::Config {
                    typo_review,
                    ..entity_resolution::v2::Config::default()
                },
            );
            let c: Vec<Value> = res
                .candidates
                .into_iter()
                .filter(|c| !c.safe && undecided(&c.left, &c.right))
                .map(|c| json!(c))
                .collect();
            let s = &res.stats;
            (
                c,
                json!({ "pairs_compared": s.pairs_compared, "em": s.em, "t_match": s.t_match, "t_possible": s.t_possible,
                        "typo_review": s.typo_review, "typo_reviews": s.typo_reviews, "typo_left_to_fs": s.typo_left_to_fs }),
            )
        }
    };
    Ok(Json(json!({
        "subject_type": stype,
        "version": version,
        "candidates": pending,
        "stats": stats,
        "note": "Possible matches are never merged automatically.",
    })))
}

/// Context of part keys for the strict typo rule: part name, supplier (fact or scope), drawing number and material
/// from current claims. Rejected and AI-suggested claims are left out (context can keep a pair out of review).
async fn part_context(tx: &mut db::Tx) -> ApiResult<HashMap<String, PartContext>> {
    let rows: Vec<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT subject_key, predicate, value_text, scope->>'supplier' FROM claims \
         WHERE subject_type = 'part' AND superseded_at IS NULL AND review_status NOT IN ('rejected', 'ai_suggested') \
           AND (predicate ~ '\\.(part_name|supplier|drawing_number|material)$' OR scope ? 'supplier') \
         LIMIT 100000",
    )
    .fetch_all(&mut **tx)
    .await?;
    let mut out: HashMap<String, PartContext> = HashMap::new();
    for (key, predicate, value, scope_supplier) in rows {
        let c = out.entry(key).or_default();
        if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
            let field = match predicate.rsplit('.').next() {
                Some("part_name") => Some(&mut c.descriptions),
                Some("supplier") => Some(&mut c.suppliers),
                Some("drawing_number") => Some(&mut c.drawings),
                Some("material") => Some(&mut c.materials),
                _ => None,
            };
            if let Some(f) = field.filter(|f| f.len() < 16 && !f.contains(&v)) {
                f.push(v);
            }
        }
        if let Some(s) =
            scope_supplier.filter(|s| !s.trim().is_empty() && c.suppliers.len() < 16 && !c.suppliers.contains(s))
        {
            c.suppliers.push(s);
        }
    }
    Ok(out)
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

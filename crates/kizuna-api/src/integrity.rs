//! Certificate integrity signals and revision drift radar (`integrity-signals` engine) over the tenant's current
//! claims. Findings are review items only: no claim is rejected, no decision changes until a reviewer confirms a
//! drift (which then runs selective invalidation for the changed claims). Who viewed a signal is audited, because
//! a signal about a supplier is sensitive even though it is not an accusation.

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

use axum::Json;
use axum::extract::{Path, State};
use entity_resolution::safe_normalize;
use integrity_signals::cert::{Certificate, DateField, Limit, Limits, Reading, Signal};
use integrity_signals::drift::{DocVersion, Drift, FieldValue};
use integrity_signals::{ENGINE_VERSION, Settings};
use policy_engine::{Permission, Resource};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiQuery, ApiResult};
use crate::simulations::Overlay;

// ---- Settings -------------------------------------------------------------------------------------------------

async fn settings_latest(tx: &mut Tx) -> ApiResult<(i32, Value, Option<(Uuid, OffsetDateTime)>)> {
    let row: Option<(i32, Value, Uuid, OffsetDateTime)> = sqlx::query_as(
        "SELECT version, settings, updated_by, updated_at FROM integrity_signal_settings ORDER BY version DESC LIMIT 1",
    )
    .fetch_optional(&mut **tx)
    .await?;
    Ok(match row {
        Some((v, s, by, at)) => (v, s, Some((by, at))),
        None => (0, json!({}), None),
    })
}

/// Effective settings (defaults where unset; a stored value that no longer validates falls back to defaults).
async fn effective(tx: &mut Tx) -> ApiResult<(i32, Settings)> {
    let (v, s, _) = settings_latest(tx).await?;
    let s: Settings = serde_json::from_value(s).unwrap_or_default();
    Ok((v, if s.validate().is_ok() { s } else { Settings::default() }))
}

pub async fn get_settings(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (v, s, by) = settings_latest(&mut tx).await?;
    let (_, eff) = effective(&mut tx).await?;
    tx.commit().await?;
    Ok(Json(json!({
        "version": v, "settings": s, "effective": eff, "defaults": Settings::default(), "bounds": Settings::bounds(),
        "updated_by": by.map(|b| b.0), "updated_at": by.map(|b| b.1.unix_timestamp()),
        "engine_version": ENGINE_VERSION,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutSettings {
    pub expected_version: i32,
    pub settings: Settings,
}

pub async fn put_settings(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<PutSettings>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::UserManage).await?;
    req.settings
        .validate()
        .map_err(|_| ApiError::BadRequest("invalid_thresholds"))?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 31))")
        .bind(format!("{}:integrity_settings", p.tenant_id))
        .execute(&mut *tx)
        .await?;
    let (cur, previous, _) = settings_latest(&mut tx).await?;
    if cur != req.expected_version {
        return Err(ApiError::Conflict("settings_version_conflict"));
    }
    let settings = json!(req.settings);
    sqlx::query(
        "INSERT INTO integrity_signal_settings (tenant_id, version, settings, updated_by) VALUES ($1,$2,$3,$4)",
    )
    .bind(p.tenant_id)
    .bind(cur + 1)
    .bind(&settings)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "settings.integrity_signals_updated",
            "tenant",
            p.tenant_id,
            json!({ "version": cur + 1, "previous": previous, "settings": settings }),
        ),
    )
    .await?;
    tx.commit().await?;
    let old: Settings = serde_json::from_value(previous).unwrap_or_default();
    crate::tenant_settings::alert_change(&st, &p, "integrity_signals", cur + 1, req.settings.weaker_than(&old)).await;
    Ok(Json(json!({ "version": cur + 1, "settings": settings })))
}

// ---- Loading claims -------------------------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct Row {
    claim: Uuid,
    subject_key: String,
    predicate: String,
    value_text: Option<String>,
    value_num: Option<String>,
    value_unit: Option<String>,
    revision: Option<String>,
    scope: Value,
    source_location: Value,
    artifact: Uuid,
    display_name: String,
    sha256: String,
    created_at: i64,
    created_ts: OffsetDateTime,
}

/// Current, usable claims of accepted documents (same evidence rules as conflict detection: no superseded,
/// rejected or unreviewed-AI claims), with the artifact they came from. `ov` applies a simulated change (what-if).
async fn load_rows(tx: &mut Tx, kinds: &[&str], ov: &Overlay) -> ApiResult<Vec<Row>> {
    let mut rows = sqlx::query_as::<_, Row>(
        "SELECT c.id AS claim, c.subject_key, c.predicate, c.value_text, c.value_num::text AS value_num, c.value_unit, \
                c.revision, c.scope, c.source_location, a.id AS artifact, a.display_name, \
                encode(a.sha256, 'hex') AS sha256, extract(epoch FROM a.created_at)::bigint AS created_at, \
                a.created_at AS created_ts \
         FROM claims c JOIN artifacts a ON a.id = c.artifact_id \
         WHERE c.subject_type = 'part' AND c.superseded_at IS NULL AND c.review_status NOT IN ('rejected', 'needs_evidence') \
           AND (c.extraction_method NOT LIKE 'ai:%' OR c.review_status = 'accepted') \
           AND a.status NOT IN ('quarantined', 'rejected') AND a.kind = ANY($1) \
         ORDER BY a.created_at, a.id, c.recorded_at, c.id",
    )
    .bind(kinds)
    .fetch_all(&mut **tx)
    .await?;
    if ov.is_empty() {
        return Ok(rows);
    }
    rows.retain(|r| ov.keeps(&r.claim));
    if let Some(s) = &ov.added
        && let Some(a) = &s.artifact
        && s.claim.subject_type == "part"
        && !matches!(a.status.as_str(), "quarantined" | "rejected")
        && kinds.contains(&a.kind.as_str())
    {
        let c = &s.claim;
        let row = Row {
            claim: c.id,
            subject_key: c.subject_key.clone(),
            predicate: c.predicate.clone(),
            value_text: c.value_text.clone(),
            value_num: c.value_num.clone(),
            value_unit: c.value_unit.clone(),
            revision: c.revision.clone(),
            scope: c.scope.clone(),
            source_location: c.source_location.clone(),
            artifact: a.id,
            display_name: a.display_name.clone(),
            sha256: a.sha256.clone(),
            created_at: a.created_ts.unix_timestamp(),
            created_ts: a.created_ts,
        };
        // The new claim is the most recently recorded one of its document: last within that document.
        let at = rows
            .iter()
            .rposition(|r| (r.created_ts, r.artifact) <= (a.created_ts, a.id))
            .map_or(0, |i| i + 1);
        rows.insert(at, row);
    }
    Ok(rows)
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

fn num(r: &Row) -> Option<Decimal> {
    r.value_num.as_deref().and_then(|n| Decimal::from_str(n).ok())
}

fn scope_str(r: &Row, k: &str) -> Option<String> {
    r.scope.get(k).and_then(Value::as_str).map(str::to_string)
}

/// Certificates (inspection reports) and spec limits (from drawings / spec documents, latest document wins).
async fn load_certificates(tx: &mut Tx, ov: &Overlay) -> ApiResult<(Vec<Certificate>, Limits)> {
    let parts = aliases(tx, "part").await?;
    let suppliers = aliases(tx, "supplier").await?;
    // One certificate per artifact, part and lot/heat (a CSV report can hold one row per lot).
    let mut certs: BTreeMap<(Uuid, String, Option<String>, Option<String>), Certificate> = BTreeMap::new();
    for r in load_rows(tx, &["inspection_report"], ov).await? {
        let part = canon(&parts, &r.subject_key);
        let (lot, heat) = (scope_str(&r, "lot"), scope_str(&r, "heat"));
        let id = format!(
            "{}/{}/{}/{}",
            r.artifact,
            part,
            lot.as_deref().unwrap_or(""),
            heat.as_deref().unwrap_or("")
        );
        let c = certs
            .entry((r.artifact, part.clone(), lot, heat))
            .or_insert_with(|| Certificate {
                id,
                artifact: r.artifact.to_string(),
                document: r.display_name.clone(),
                supplier: String::new(),
                part,
                lot: None,
                heat: None,
                issued: None,
                shipped: None,
                inspected: None,
                readings: Vec::new(),
            });
        if c.supplier.is_empty() {
            let s = scope_str(&r, "supplier")
                .or_else(|| (r.predicate == "qa.supplier").then(|| r.value_text.clone()).flatten());
            if let Some(s) = s {
                c.supplier = canon(&suppliers, &s);
            }
        }
        c.lot = c.lot.take().or_else(|| scope_str(&r, "lot"));
        c.heat = c.heat.take().or_else(|| scope_str(&r, "heat"));
        let date = || {
            r.value_text.clone().map(|d| DateField {
                date: d,
                claim: r.claim.to_string(),
                location: r.source_location.clone(),
            })
        };
        match r.predicate.as_str() {
            "qa.certificate_date" => c.issued = c.issued.take().or_else(date),
            "qa.ship_date" => c.shipped = c.shipped.take().or_else(date),
            "qa.inspection_date" => c.inspected = c.inspected.take().or_else(date),
            p if p.starts_with("qa.measured.") || p.starts_with("qa.property.") || p.starts_with("qa.chem.") => {
                if let Some(v) = num(&r) {
                    c.readings.push(Reading {
                        characteristic: p.to_string(),
                        value: v,
                        unit: r.value_unit.clone(),
                        claim: r.claim.to_string(),
                        location: r.source_location.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    // Spec limits: nominal ± tolerance per part and dimension, from the most recently captured document.
    type Spec = (
        Option<Decimal>,
        Option<String>,
        Option<Decimal>,
        Option<Decimal>,
        Vec<String>,
    );
    let mut specs: BTreeMap<(String, String), Spec> = BTreeMap::new();
    for r in load_rows(tx, &["drawing", "spec_document"], ov).await? {
        let part = canon(&parts, &r.subject_key);
        let (dim, slot) = if let Some(d) = r.predicate.strip_prefix("mfg.dimension.") {
            (d.to_string(), 0)
        } else if let Some(d) = r
            .predicate
            .strip_prefix("mfg.tolerance.")
            .and_then(|x| x.strip_suffix(".plus"))
        {
            (d.to_string(), 1)
        } else if let Some(d) = r
            .predicate
            .strip_prefix("mfg.tolerance.")
            .and_then(|x| x.strip_suffix(".minus"))
        {
            (d.to_string(), 2)
        } else {
            continue;
        };
        let Some(v) = num(&r) else { continue };
        // Rows come in capture order: later documents overwrite earlier ones.
        let e = specs.entry((part, dim)).or_default();
        match slot {
            0 => {
                e.0 = Some(v);
                e.1 = r.value_unit.clone();
            }
            1 => e.2 = Some(v),
            _ => e.3 = Some(v),
        }
        e.4.push(r.claim.to_string());
    }
    let mut limits = Limits::new();
    for ((part, dim), (nominal, unit, plus, minus, claims)) in specs {
        let (Some(n), Some(pl), Some(mi)) = (nominal, plus, minus) else {
            continue;
        };
        limits.insert(
            (part, format!("qa.measured.{dim}")),
            Limit {
                lower: Some(n - mi.abs()),
                upper: Some(n + pl.abs()),
                unit,
                claims: claims.into_iter().rev().take(6).collect(),
            },
        );
    }
    Ok((certs.into_values().filter(|c| !c.supplier.is_empty()).collect(), limits))
}

/// Versions of drawings / specs: one per (artifact, part); key = drawing number (図番) when extracted, else part.
async fn load_versions(tx: &mut Tx, ov: &Overlay) -> ApiResult<Vec<DocVersion>> {
    let parts = aliases(tx, "part").await?;
    let mut vs: BTreeMap<(Uuid, String), DocVersion> = BTreeMap::new();
    for r in load_rows(tx, &["drawing", "spec_document"], ov).await? {
        let part = canon(&parts, &r.subject_key);
        let v = vs.entry((r.artifact, part.clone())).or_insert_with(|| DocVersion {
            id: r.artifact.to_string(),
            document: r.display_name.clone(),
            key: part.clone(),
            part,
            revision: None,
            sha256: r.sha256.clone(),
            captured_at: r.created_at,
            fields: Vec::new(),
        });
        if v.revision.is_none() {
            v.revision = r.revision.clone();
        }
        match r.predicate.as_str() {
            "mfg.revision" => v.revision = r.value_text.clone().or(v.revision.take()),
            "mfg.drawing_number" => {
                if let Some(d) = &r.value_text {
                    v.key = safe_normalize(d);
                }
            }
            _ => {}
        }
        if r.predicate.starts_with("mfg.") || r.predicate.starts_with("drawing.") {
            v.fields.push(FieldValue {
                predicate: r.predicate.clone(),
                text: r.value_text.clone(),
                num: num(&r),
                unit: r.value_unit.clone(),
                claim: r.claim.to_string(),
                location: r.source_location.clone(),
            });
        }
    }
    Ok(vs.into_values().collect())
}

/// Signals and drift findings the engines produce over the current claims with `ov` applied (no writes).
pub(crate) async fn live(tx: &mut Tx, ov: &Overlay) -> ApiResult<(i32, Vec<Signal>, Vec<Drift>)> {
    let (sv, s) = effective(tx).await?;
    let (certs, limits) = load_certificates(tx, ov).await?;
    let signals = integrity_signals::cert::analyze(&certs, &limits, &s);
    let versions = load_versions(tx, ov).await?;
    let drifts = integrity_signals::drift::detect(&versions, &s);
    Ok((sv, signals, drifts))
}

// ---- Run ------------------------------------------------------------------------------------------------------

#[derive(Serialize)]
pub struct RunResp {
    pub engine_version: &'static str,
    pub settings_version: i32,
    pub signals_total: usize,
    pub signals_new: usize,
    pub drift_total: usize,
    pub drift_new: usize,
    pub marked_obsolete: u64,
}

fn artifacts_of(evidence: &[integrity_signals::EvidenceRef]) -> Vec<Uuid> {
    let set: std::collections::BTreeSet<Uuid> = evidence
        .iter()
        .filter_map(|e| Uuid::parse_str(&e.artifact).ok())
        .collect();
    set.into_iter().collect()
}

pub async fn run(State(st): State<AppState>, p: Principal) -> ApiResult<Json<RunResp>> {
    p.require(&st, Permission::ClaimWrite).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let resp = run_in_tx(&mut tx, p.tenant_id).await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "integrity.run",
            "tenant",
            p.tenant_id,
            json!({ "engine": ENGINE_VERSION, "settings_version": resp.settings_version, "signals_new": resp.signals_new,
                    "drift_new": resp.drift_new, "obsolete": resp.marked_obsolete }),
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(resp))
}

/// Recomputes all findings. New ones are inserted; ones that fire again after becoming obsolete reopen; reviewed
/// ones keep their status and text; open ones that no longer fire become obsolete (kept, never deleted).
pub async fn run_in_tx(tx: &mut Tx, tenant: Uuid) -> ApiResult<RunResp> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 37))")
        .bind(tenant.to_string())
        .execute(&mut **tx)
        .await?;
    let (sv, signals, drifts) = live(tx, &Overlay::default()).await?;
    let mut signals_new = 0;
    for g in &signals {
        let inserted: Option<bool> = sqlx::query_scalar(
            "INSERT INTO integrity_signals (id, tenant_id, kind, supplier_key, part_key, characteristic, strength, explanation, \
             statistic, evidence, artifact_ids, fingerprint, engine_version, settings_version) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) \
             ON CONFLICT (tenant_id, fingerprint) DO UPDATE SET strength = EXCLUDED.strength, explanation = EXCLUDED.explanation, \
               statistic = EXCLUDED.statistic, evidence = EXCLUDED.evidence, artifact_ids = EXCLUDED.artifact_ids, \
               engine_version = EXCLUDED.engine_version, settings_version = EXCLUDED.settings_version, \
               status = CASE WHEN integrity_signals.status = 'obsolete' THEN 'open' ELSE integrity_signals.status END \
             WHERE integrity_signals.status IN ('open', 'needs_evidence', 'obsolete') \
             RETURNING (xmax = 0)",
        )
        .bind(Uuid::now_v7())
        .bind(tenant)
        .bind(g.kind.as_str())
        .bind(&g.supplier)
        .bind(&g.part)
        .bind(&g.characteristic)
        .bind(g.strength.as_str())
        .bind(json!({ "why": g.why, "why_ja": g.why_ja }))
        .bind(&g.statistic)
        .bind(json!(g.evidence))
        .bind(artifacts_of(&g.evidence))
        .bind(&g.fingerprint)
        .bind(ENGINE_VERSION)
        .bind(sv)
        .fetch_optional(&mut **tx)
        .await?;
        signals_new += usize::from(inserted == Some(true));
    }
    let mut drift_new = 0;
    for d in &drifts {
        let (Ok(a), Ok(b)) = (Uuid::parse_str(&d.earlier), Uuid::parse_str(&d.later)) else {
            continue;
        };
        let inserted: Option<bool> = sqlx::query_scalar(
            "INSERT INTO revision_drift_findings (id, tenant_id, kind, drawing_key, part_key, earlier_artifact, later_artifact, \
             earlier_revision, later_revision, changes, strength, explanation, fingerprint, engine_version, settings_version) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) \
             ON CONFLICT (tenant_id, fingerprint) DO UPDATE SET changes = EXCLUDED.changes, strength = EXCLUDED.strength, \
               explanation = EXCLUDED.explanation, engine_version = EXCLUDED.engine_version, settings_version = EXCLUDED.settings_version, \
               status = CASE WHEN revision_drift_findings.status = 'obsolete' THEN 'open' ELSE revision_drift_findings.status END \
             WHERE revision_drift_findings.status IN ('open', 'needs_evidence', 'obsolete') \
             RETURNING (xmax = 0)",
        )
        .bind(Uuid::now_v7())
        .bind(tenant)
        .bind(d.kind.as_str())
        .bind(&d.key)
        .bind(&d.part)
        .bind(a)
        .bind(b)
        .bind(&d.earlier_revision)
        .bind(&d.later_revision)
        .bind(json!(d.changes))
        .bind(d.strength.as_str())
        .bind(json!({ "why": d.why, "why_ja": d.why_ja, "file_changed": d.file_changed }))
        .bind(&d.fingerprint)
        .bind(ENGINE_VERSION)
        .bind(sv)
        .fetch_optional(&mut **tx)
        .await?;
        drift_new += usize::from(inserted == Some(true));
    }
    let live: Vec<String> = signals.iter().map(|g| g.fingerprint.clone()).collect();
    let mut obsolete = sqlx::query(
        "UPDATE integrity_signals SET status = 'obsolete' WHERE status IN ('open', 'needs_evidence') AND NOT (fingerprint = ANY($1))",
    )
    .bind(&live)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    let live: Vec<String> = drifts.iter().map(|d| d.fingerprint.clone()).collect();
    obsolete += sqlx::query(
        "UPDATE revision_drift_findings SET status = 'obsolete' WHERE status IN ('open', 'needs_evidence') AND NOT (fingerprint = ANY($1))",
    )
    .bind(&live)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    Ok(RunResp {
        engine_version: ENGINE_VERSION,
        settings_version: sv,
        signals_total: signals.len(),
        signals_new,
        drift_total: drifts.len(),
        drift_new,
        marked_obsolete: obsolete,
    })
}

// ---- Signals: list, detail, review ----------------------------------------------------------------------------

#[derive(Serialize, sqlx::FromRow)]
pub struct SignalOut {
    pub id: Uuid,
    pub kind: String,
    pub supplier_key: String,
    pub part_key: Option<String>,
    pub characteristic: Option<String>,
    pub strength: String,
    pub explanation: Value,
    pub statistic: Value,
    pub evidence: Value,
    pub artifact_ids: Vec<Uuid>,
    pub engine_version: String,
    pub settings_version: i32,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub detected_at: OffsetDateTime,
}

const SIGNAL_COLS: &str = "id, kind, supplier_key, part_key, characteristic, strength, explanation, statistic, evidence, \
    artifact_ids, engine_version, settings_version, status, detected_at";

#[derive(Deserialize)]
pub struct ListQuery {
    pub status: Option<String>,
    pub kind: Option<String>,
    pub supplier: Option<String>,
}

pub async fn list_signals(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> ApiResult<Json<Vec<SignalOut>>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, SignalOut>(&format!(
        "SELECT {SIGNAL_COLS} FROM integrity_signals WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR kind = $2) \
         AND ($3::text IS NULL OR supplier_key = $3) \
         ORDER BY CASE strength WHEN 'strong' THEN 0 WHEN 'moderate' THEN 1 ELSE 2 END, detected_at DESC LIMIT 500"
    ))
    .bind(q.status)
    .bind(q.kind)
    .bind(q.supplier)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct IntegrityReviewOut {
    pub decision: String,
    pub comment: Option<String>,
    pub reviewer: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub decided_at: OffsetDateTime,
}

async fn reviews(tx: &mut Tx, target_type: &str, id: Uuid) -> ApiResult<Vec<IntegrityReviewOut>> {
    Ok(sqlx::query_as::<_, IntegrityReviewOut>(
        "SELECT decision, comment, reviewer, decided_at FROM integrity_reviews WHERE target_type = $1 AND target_id = $2 ORDER BY decided_at",
    )
    .bind(target_type)
    .bind(id)
    .fetch_all(&mut **tx)
    .await?)
}

/// Documents behind a finding with their hashes (a hash proves only that a document is unchanged since capture).
async fn documents(tx: &mut Tx, ids: &[Uuid]) -> ApiResult<Vec<Value>> {
    Ok(sqlx::query_scalar(
        "SELECT json_build_object('artifact_id', id, 'display_name', display_name, 'sha256', encode(sha256, 'hex'), 'kind', kind)::jsonb \
         FROM artifacts WHERE id = ANY($1) ORDER BY created_at, id",
    )
    .bind(ids)
    .fetch_all(&mut **tx)
    .await?)
}

fn claims_in(evidence: &Value) -> Vec<Uuid> {
    evidence
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| e["claim"].as_str().and_then(|c| Uuid::parse_str(c).ok()))
                .collect()
        })
        .unwrap_or_default()
}

pub async fn get_signal(State(st): State<AppState>, p: Principal, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let g = sqlx::query_as::<_, SignalOut>(&format!("SELECT {SIGNAL_COLS} FROM integrity_signals WHERE id = $1"))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    let docs = documents(&mut tx, &g.artifact_ids).await?;
    let revs = reviews(&mut tx, "integrity_signal", id).await?;
    // Who looked at a signal about a supplier is part of the record.
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "integrity_signal.viewed",
            "integrity_signal",
            id,
            json!({ "kind": g.kind }),
        ),
    )
    .await?;
    tx.commit().await?;
    let open = matches!(g.status.as_str(), "open" | "needs_evidence");
    Ok(Json(json!({
        "signal": g,
        "documents": docs,
        "reviews": revs,
        "requires_human_review": open,
        "never_auto_reject": true,
        "limitations": format!("Statistical/rule signal {ENGINE_VERSION} on extracted values. Strength describes how pronounced the pattern is; it is not a probability of wrongdoing. Extraction errors, ordinary process behaviour or re-issued documents can produce the same pattern. No evidence is rejected automatically."),
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewReq {
    pub decision: String,
    pub comment: Option<String>,
}

fn comment_of(req: &ReviewReq, required: bool) -> ApiResult<Option<String>> {
    let c = req
        .comment
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_owned);
    if c.as_ref().is_some_and(|c| c.chars().count() > 2000) {
        return Err(ApiError::BadRequest("comment_too_long"));
    }
    if required && c.is_none() {
        return Err(ApiError::BadRequest("comment_required"));
    }
    Ok(c)
}

async fn insert_review(
    tx: &mut Tx,
    p: &Principal,
    target_type: &str,
    id: Uuid,
    decision: &str,
    comment: &Option<String>,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO integrity_reviews (id, tenant_id, target_type, target_id, decision, comment, reviewer) VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(Uuid::now_v7())
    .bind(p.tenant_id)
    .bind(target_type)
    .bind(id)
    .bind(decision)
    .bind(comment)
    .bind(p.user_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Four-eyes: whoever put the evidence in may not review a finding about it.
async fn require_reviewer(
    st: &AppState,
    p: &Principal,
    tx: &mut Tx,
    kind: &'static str,
    claims: &[Uuid],
) -> ApiResult<()> {
    let owners = crate::verification::claim_owners(tx, claims).await?;
    p.require_on(
        st,
        Permission::ReviewDecide,
        &Resource {
            tenant_id: p.tenant_id,
            kind,
            owner_user_id: owners.contains(&p.user_id).then_some(p.user_id),
        },
    )
    .await
}

/// explained = an ordinary cause was found (documented); follow_up = the reviewer decided to ask the supplier;
/// request_evidence = more documents are needed. None of these changes any claim.
pub async fn review_signal(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(req): ApiJson<ReviewReq>,
) -> ApiResult<Json<SignalOut>> {
    p.require(&st, Permission::ReviewDecide).await?;
    let (decision, status) = match req.decision.as_str() {
        "explained" => ("explained", "explained"),
        "follow_up" => ("follow_up", "follow_up"),
        "request_evidence" => ("request_evidence", "needs_evidence"),
        _ => return Err(ApiError::BadRequest("invalid_decision")),
    };
    let comment = comment_of(&req, decision != "request_evidence")?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let row: Option<(String, Value)> =
        sqlx::query_as("SELECT status, evidence FROM integrity_signals WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let (cur, evidence) = row.ok_or(ApiError::NotFound)?;
    require_reviewer(&st, &p, &mut tx, "integrity_signal", &claims_in(&evidence)).await?;
    if !matches!(cur.as_str(), "open" | "needs_evidence") {
        return Err(ApiError::Conflict("signal_not_open"));
    }
    insert_review(&mut tx, &p, "integrity_signal", id, decision, &comment).await?;
    sqlx::query("UPDATE integrity_signals SET status = $2 WHERE id = $1")
        .bind(id)
        .bind(status)
        .execute(&mut *tx)
        .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "integrity_signal.reviewed",
            "integrity_signal",
            id,
            json!({ "decision": decision, "status": status }),
        ),
    )
    .await?;
    let out = sqlx::query_as::<_, SignalOut>(&format!("SELECT {SIGNAL_COLS} FROM integrity_signals WHERE id = $1"))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---- Revision drift: list, detail, review ---------------------------------------------------------------------

#[derive(Serialize, sqlx::FromRow)]
pub struct DriftOut {
    pub id: Uuid,
    pub kind: String,
    pub drawing_key: String,
    pub part_key: String,
    pub earlier_artifact: Uuid,
    pub later_artifact: Uuid,
    pub earlier_revision: Option<String>,
    pub later_revision: Option<String>,
    pub changes: Value,
    pub strength: String,
    pub explanation: Value,
    pub engine_version: String,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub detected_at: OffsetDateTime,
}

const DRIFT_COLS: &str = "id, kind, drawing_key, part_key, earlier_artifact, later_artifact, earlier_revision, later_revision, \
    changes, strength, explanation, engine_version, status, detected_at";

#[derive(Deserialize)]
pub struct DriftQuery {
    pub status: Option<String>,
    pub kind: Option<String>,
    pub drawing: Option<String>,
}

pub async fn list_drift(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<DriftQuery>,
) -> ApiResult<Json<Vec<DriftOut>>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, DriftOut>(&format!(
        "SELECT {DRIFT_COLS} FROM revision_drift_findings WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR kind = $2) \
         AND ($3::text IS NULL OR drawing_key = $3) \
         ORDER BY CASE strength WHEN 'strong' THEN 0 WHEN 'moderate' THEN 1 ELSE 2 END, detected_at DESC LIMIT 500"
    ))
    .bind(q.status)
    .bind(q.kind)
    .bind(q.drawing)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

/// Claim ids on one side ("before" / "after") of the stored field changes.
fn change_claims(changes: &Value, side: &str) -> Vec<Uuid> {
    changes
        .as_array()
        .map(|a| {
            a.iter()
                .flat_map(|c| c[side].as_array().cloned().unwrap_or_default())
                .filter_map(|s| s["claim"].as_str().and_then(|c| Uuid::parse_str(c).ok()))
                .collect()
        })
        .unwrap_or_default()
}

pub async fn get_drift(State(st): State<AppState>, p: Principal, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let d = sqlx::query_as::<_, DriftOut>(&format!(
        "SELECT {DRIFT_COLS} FROM revision_drift_findings WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let docs = documents(&mut tx, &[d.earlier_artifact, d.later_artifact]).await?;
    let revs = reviews(&mut tx, "revision_drift", id).await?;
    let mut involved = change_claims(&d.changes, "before");
    involved.extend(change_claims(&d.changes, "after"));
    let affected = crate::decisions::affected_by_claims(&mut tx, &involved).await?;
    tx.commit().await?;
    let open = matches!(d.status.as_str(), "open" | "needs_evidence");
    Ok(Json(json!({
        "drift": d,
        "documents": docs,
        "reviews": revs,
        "requires_human_review": open,
        "potentially_affected_decisions": affected,
        "limitations": format!("Compares extracted fields ({ENGINE_VERSION}); content that is not extracted (views, notes not read by OCR) is not compared, and a field missing in one version may be an extraction gap. Confirming the drift runs selective invalidation for the changed claims."),
    })))
}

/// accept_change = the difference is legitimate (documented); confirm_drift = the drift is real: the later
/// version's changed claims are fed to selective invalidation (decisions depending on that drawing/revision are
/// assessed for materiality and flagged, never re-decided); request_evidence = ask for the controlled copy.
pub async fn review_drift(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(req): ApiJson<ReviewReq>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ReviewDecide).await?;
    let (decision, status) = match req.decision.as_str() {
        "accept_change" => ("accept_change", "accepted_change"),
        "confirm_drift" => ("confirm_drift", "confirmed"),
        "request_evidence" => ("request_evidence", "needs_evidence"),
        _ => return Err(ApiError::BadRequest("invalid_decision")),
    };
    let comment = comment_of(&req, decision != "request_evidence")?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let row: Option<(String, Value)> =
        sqlx::query_as("SELECT status, changes FROM revision_drift_findings WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let (cur, changes) = row.ok_or(ApiError::NotFound)?;
    let after = change_claims(&changes, "after");
    let mut involved = change_claims(&changes, "before");
    involved.extend(&after);
    require_reviewer(&st, &p, &mut tx, "revision_drift", &involved).await?;
    if !matches!(cur.as_str(), "open" | "needs_evidence") {
        return Err(ApiError::Conflict("drift_not_open"));
    }
    insert_review(&mut tx, &p, "revision_drift", id, decision, &comment).await?;
    sqlx::query("UPDATE revision_drift_findings SET status = $2 WHERE id = $1")
        .bind(id)
        .bind(status)
        .execute(&mut *tx)
        .await?;
    let mut flagged = 0;
    if decision == "confirm_drift" {
        for c in &after {
            let claim = crate::claims::fetch(&mut tx, *c).await?;
            if claim.superseded_at.is_some() || claim.review_status == "rejected" {
                continue;
            }
            flagged += crate::decisions::on_claim_change(
                &mut tx,
                p.tenant_id,
                p.user_id,
                crate::decisions::new_evidence(&claim),
            )
            .await?;
        }
    }
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "revision_drift.reviewed",
            "revision_drift",
            id,
            json!({ "decision": decision, "status": status, "decisions_flagged": flagged }),
        ),
    )
    .await?;
    let out = sqlx::query_as::<_, DriftOut>(&format!(
        "SELECT {DRIFT_COLS} FROM revision_drift_findings WHERE id = $1"
    ))
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "drift": out, "decisions_flagged": flagged })))
}

//! Versioned per-tenant settings (append-only `tenant_settings`): AI-assisted extraction opt-in and detection
//! thresholds. Changes need a tenant admin with MFA (`UserManage`), use optimistic concurrency and are audited.

use axum::Json;
use axum::extract::State;
use policy_engine::Permission;
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiResult};

/// Latest version and settings of one kind (version 0 and `{}` when never set).
pub async fn latest(tx: &mut Tx, kind: &str) -> ApiResult<(i32, Value, Option<(Uuid, OffsetDateTime)>)> {
    let row: Option<(i32, Value, Uuid, OffsetDateTime)> = sqlx::query_as(
        "SELECT version, settings, updated_by, updated_at FROM tenant_settings WHERE kind = $1 ORDER BY version DESC LIMIT 1",
    )
    .bind(kind)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(match row {
        Some((v, s, by, at)) => (v, s, Some((by, at))),
        None => (0, json!({}), None),
    })
}

async fn save(tx: &mut Tx, p: &Principal, kind: &'static str, expected: i32, settings: &Value) -> ApiResult<i32> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 29))")
        .bind(format!("{}:{kind}", p.tenant_id))
        .execute(&mut **tx)
        .await?;
    let cur: i32 = sqlx::query_scalar("SELECT coalesce(max(version), 0) FROM tenant_settings WHERE kind = $1")
        .bind(kind)
        .fetch_one(&mut **tx)
        .await?;
    if cur != expected {
        return Err(ApiError::Conflict("settings_version_conflict"));
    }
    sqlx::query("INSERT INTO tenant_settings (tenant_id, kind, version, settings, updated_by) VALUES ($1,$2,$3,$4,$5)")
        .bind(p.tenant_id)
        .bind(kind)
        .bind(cur + 1)
        .bind(settings)
        .bind(p.user_id)
        .execute(&mut **tx)
        .await?;
    audit::append(
        tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            &format!("settings.{kind}_updated"),
            "tenant",
            p.tenant_id,
            json!({ "version": cur + 1, "settings": settings }),
        ),
    )
    .await?;
    Ok(cur + 1)
}

fn meta(v: i32, s: Value, by: Option<(Uuid, OffsetDateTime)>) -> Value {
    json!({
        "version": v, "settings": s,
        "updated_by": by.map(|b| b.0), "updated_at": by.map(|b| b.1.unix_timestamp()),
    })
}

// ---- AI-assisted extraction opt-in ----------------------------------------------------------------------------

pub const AI_NOTICE_VERSION: &str = "ai-notice@1";
pub const AI_NOTICE: &str = "AI-assisted extraction sends text extracted from your documents to an external AI \
provider chosen by the KIZUNA operator. Only free AI services are used. Free providers may log, keep and use the \
data you send to train their models, and KIZUNA has no data-processing agreement with them. KIZUNA sends only \
column labels that its rules could not map, with up to two example values each; e-mail addresses, phone numbers, \
URLs and values of person-related fields are removed first, but other document content in those labels and values \
(for example part numbers, materials or customer names inside a label) can reach the provider. AI results are \
suggestions only: they are stored as 'ai_suggested' and must be reviewed by a person. Do not enable this for \
confidential, export-controlled or personal data.";

/// Whether this tenant has opted in under the current notice.
pub async fn ai_opted_in(tx: &mut Tx) -> ApiResult<bool> {
    let (_, s, _) = latest(tx, "ai_extraction").await?;
    Ok(s["enabled"] == json!(true) && s["notice_version"] == json!(AI_NOTICE_VERSION))
}

pub async fn get_ai(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::UserRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (v, s, by) = latest(&mut tx, "ai_extraction").await?;
    let opted = ai_opted_in(&mut tx).await?;
    let used: (i64, i64) = sqlx::query_as(
        "SELECT count(*), coalesce(sum(prompt_chars), 0)::bigint FROM ai_calls WHERE occurred_at >= date_trunc('day', now())",
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut out = meta(v, s, by);
    out["active"] = json!(opted && st.cfg.ai.is_some());
    out["notice"] = json!({ "version": AI_NOTICE_VERSION, "text": AI_NOTICE });
    out["provider"] = match &st.cfg.ai {
        None => json!({ "configured": false }),
        Some(a) => json!({
            "configured": true, "model": a.model,
            "host": a.base_url.split("://").nth(1).and_then(|r| r.split('/').next()),
            "daily_calls": a.daily_calls, "daily_chars": a.daily_chars, "calls_per_min": a.calls_per_min,
        }),
    };
    out["usage_today"] = json!({ "calls": used.0, "prompt_chars": used.1 });
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutAi {
    pub expected_version: i32,
    pub enabled: bool,
    /// Must equal the current notice version to enable (explicit acknowledgement).
    #[serde(default)]
    pub accept_notice: Option<String>,
}

pub async fn put_ai(State(st): State<AppState>, p: Principal, ApiJson(req): ApiJson<PutAi>) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::UserManage).await?;
    if req.enabled && req.accept_notice.as_deref() != Some(AI_NOTICE_VERSION) {
        return Err(ApiError::BadRequest("notice_not_accepted"));
    }
    let settings = if req.enabled {
        json!({ "enabled": true, "notice_version": AI_NOTICE_VERSION, "accepted_by": p.user_id })
    } else {
        json!({ "enabled": false })
    };
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let v = save(&mut tx, &p, "ai_extraction", req.expected_version, &settings).await?;
    tx.commit().await?;
    Ok(Json(json!({ "version": v, "settings": settings })))
}

// ---- Detection thresholds -------------------------------------------------------------------------------------

/// Effective detection settings of the tenant (defaults where unset).
pub async fn detection(tx: &mut Tx) -> ApiResult<security_detection::DetectionSettings> {
    let (_, s, _) = latest(tx, "detection").await?;
    Ok(serde_json::from_value(s).unwrap_or_default())
}

pub async fn get_detection(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::SecurityEventRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (v, s, by) = latest(&mut tx, "detection").await?;
    let eff = detection(&mut tx).await?;
    let active = crate::incidents::thresholds_for(&mut tx, p.tenant_id).await?;
    tx.commit().await?;
    let mut out = meta(v, s, by);
    out["effective"] = json!(eff);
    out["active_thresholds"] = json!(active);
    out["defaults"] = json!(security_detection::DetectionSettings::default());
    out["bounds"] = json!(security_detection::DetectionSettings::bounds());
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutDetection {
    pub expected_version: i32,
    pub settings: security_detection::DetectionSettings,
}

pub async fn put_detection(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<PutDetection>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::UserManage).await?;
    req.settings
        .validate()
        .map_err(|_| ApiError::BadRequest("invalid_thresholds"))?;
    let settings = json!(req.settings);
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let v = save(&mut tx, &p, "detection", req.expected_version, &settings).await?;
    tx.commit().await?;
    crate::incidents::forget_baseline(p.tenant_id);
    Ok(Json(json!({ "version": v, "settings": settings })))
}

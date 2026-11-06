//! Read models and settings requested by the web UI (web/API_GAPS.md): dashboard summary, subject listing,
//! organisation-level audit report, versioned import settings, a user's sessions.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use policy_engine::Permission;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::Principal;
use crate::db;
use crate::error::{ApiError, ApiJson, ApiQuery, ApiResult};

async fn counts(tx: &mut db::Tx, sql: &str) -> ApiResult<Value> {
    let rows: Vec<(String, i64)> = sqlx::query_as(sql).fetch_all(&mut **tx).await?;
    Ok(Value::Object(rows.into_iter().map(|(k, n)| (k, json!(n))).collect()))
}

/// Counts by status for the overview (gap #2).
pub async fn dashboard(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_snapshot_tx(&st.pool, p.tenant_id).await?;
    let out = json!({
        "artifacts": counts(&mut tx, "SELECT status, count(*) FROM artifacts GROUP BY 1").await?,
        "claims": counts(&mut tx, "SELECT review_status, count(*) FROM claims WHERE superseded_at IS NULL GROUP BY 1").await?,
        "conflicts": counts(&mut tx, "SELECT status, count(*) FROM conflicts GROUP BY 1").await?,
        "conflict_kinds_open": counts(&mut tx, "SELECT kind, count(*) FROM conflicts WHERE status IN ('open','needs_evidence') GROUP BY 1").await?,
        "decisions": counts(&mut tx, "SELECT status, count(*) FROM decisions GROUP BY 1").await?,
        "subjects": counts(&mut tx, "SELECT subject_type, count(DISTINCT subject_key) FROM claims WHERE superseded_at IS NULL GROUP BY 1").await?,
    });
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct SubjectsQuery {
    #[serde(rename = "type")]
    pub subject_type: Option<String>,
    pub after: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct SubjectOut {
    pub subject_type: String,
    pub subject_key: String,
    pub claims: i64,
    pub predicates: i64,
}

/// Distinct records with counts, keyset-paged by key (gap #3).
pub async fn subjects(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<SubjectsQuery>,
) -> ApiResult<Json<Vec<SubjectOut>>> {
    p.require(&st, Permission::ClaimRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, SubjectOut>(
        "SELECT subject_type, subject_key, count(*) AS claims, count(DISTINCT predicate) AS predicates FROM claims \
         WHERE superseded_at IS NULL AND ($1::text IS NULL OR subject_type = $1) AND ($2::text IS NULL OR subject_key > $2) \
         GROUP BY 1, 2 ORDER BY 2, 1 LIMIT $3",
    )
    .bind(q.subject_type)
    .bind(q.after)
    .bind(q.limit.unwrap_or(200).clamp(1, 1000))
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct OrgReportReq {
    /// `open_findings` (default) or `all`.
    pub scope: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    /// Calendar dates (`YYYY-MM-DD`, both inclusive) in the tenant's timezone; preferred over `from`/`to`.
    pub from_date: Option<String>,
    pub to_date: Option<String>,
}

/// Organisation-level audit report (gap #8): findings, decisions and audit-chain status, hashed and signed.
pub async fn org_report(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<OrgReportReq>,
) -> ApiResult<Response> {
    p.require(&st, Permission::AuditRead).await?;
    audit::heavy_limit(&st, &p)?;
    let parse = |s: &Option<String>| {
        s.as_deref()
            .map(|v| {
                OffsetDateTime::parse(v, &time::format_description::well_known::Rfc3339)
                    .map_err(|_| ApiError::BadRequest("invalid_timestamp"))
            })
            .transpose()
    };
    let (mut from, mut to) = (parse(&req.from)?, parse(&req.to)?);
    // Dates are tenant-local days in the tenant's IANA timezone (bughunt: a JST user's "5 Nov" is 4 Nov 15:00Z –
    // 5 Nov 15:00Z).
    let parse_day = |v: &Option<String>| -> ApiResult<Option<time::Date>> {
        v.as_deref()
            .map(|v| {
                time::Date::parse(v, &time::macros::format_description!("[year]-[month]-[day]"))
                    .map_err(|_| ApiError::BadRequest("invalid_date"))
            })
            .transpose()
    };
    let (fd, td) = (parse_day(&req.from_date)?, parse_day(&req.to_date)?);
    let td = td
        .map(|d| d.next_day().ok_or(ApiError::BadRequest("invalid_date")))
        .transpose()?;
    let tz = {
        let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
        let tz = db::tenant_timezone(&mut tx).await?;
        if let Some(d) = fd {
            from = Some(db::tenant_midnight(&mut tx, d).await?);
        }
        if let Some(d) = td {
            to = Some(db::tenant_midnight(&mut tx, d).await?);
        }
        tx.commit().await?;
        tz
    };
    let all = match req.scope.as_deref() {
        None | Some("open_findings") => false,
        Some("all") => true,
        Some(_) => return Err(ApiError::BadRequest("invalid_scope")),
    };
    let mut tx = db::tenant_snapshot_tx(&st.pool, p.tenant_id).await?;
    let findings: Vec<Value> = sqlx::query_scalar(
        "SELECT json_build_object('id', id, 'kind', kind, 'subject', subject_key, 'status', status, 'why', explanation->>'why', 'msg', explanation->'msg', \
           'detected_at', detected_at)::jsonb FROM conflicts WHERE ($1 OR status IN ('open', 'needs_evidence')) \
           AND ($2::timestamptz IS NULL OR detected_at >= $2) AND ($3::timestamptz IS NULL OR detected_at < $3) ORDER BY detected_at LIMIT 10001",
    )
    .bind(all)
    .bind(from)
    .bind(to)
    .fetch_all(&mut *tx)
    .await?;
    let decisions: Vec<Value> = sqlx::query_scalar(
        "SELECT json_build_object('id', id, 'title', title, 'subject', subject_key, 'outcome', outcome, 'status', status, \
           'derivation_sha256', encode(derivation_sha256, 'hex'))::jsonb FROM decisions \
         WHERE ($1 OR status = 'potentially_affected') AND ($2::timestamptz IS NULL OR decided_at >= $2) \
           AND ($3::timestamptz IS NULL OR decided_at < $3) ORDER BY decided_at LIMIT 10001",
    )
    .bind(all)
    .bind(from)
    .bind(to)
    .fetch_all(&mut *tx)
    .await?;
    let chain = audit::verify_tenant(&mut tx, p.tenant_id).await?;
    tx.commit().await?;
    // Bounded output: narrow the date range when a report would exceed 10 000 rows per section.
    let (mut findings, mut decisions) = (findings, decisions);
    let truncated = findings.len() > 10_000 || decisions.len() > 10_000;
    findings.truncate(10_000);
    decisions.truncate(10_000);
    let mut body = json!({
        "report_version": "kizuna-org-report@1",
        "generated_at": OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        "generated_by": p.user_id,
        "scope": if all { "all" } else { "open_findings" },
        "range": { "from": from.map(|f| f.unix_timestamp()), "to_exclusive": to.map(|t| t.unix_timestamp()),
                   "timezone": tz },
        "findings": findings,
        "decisions": decisions,
        "audit_chain": { "intact": chain.intact, "events": chain.events_checked, "head_sha256": chain.head_hash },
        "statement": kizuna_messages::Msg::new("statement.org_report").en(),
        "statement_msg": kizuna_messages::Msg::new("statement.org_report"),
        "truncated": truncated,
    });
    let sha = crate::crypto::sha256(
        audit_engine::canonical_json(&body)
            .map_err(ApiError::internal)?
            .as_bytes(),
    );
    body["report_sha256"] = json!(hex::encode(sha));
    if let Some(k) = &st.cfg.audit_signing_key {
        let msg = [b"KZRP1".as_slice(), &sha].concat();
        body["signature"] = json!({ "algorithm": "ed25519", "public_key": hex::encode(audit_engine::checkpoint::public_key(k.expose())),
            "signature": hex::encode(audit_engine::checkpoint::sign(k.expose(), &msg)) });
    }
    // Exports are part of the tamper-evident audit chain too (review M5-M7 I1).
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        audit::Record::user(
            p.user_id,
            "export.report",
            "report",
            hex::encode(sha),
            json!({ "report": "org", "scope": if all { "all" } else { "open_findings" }, "truncated": truncated }),
        ),
    )
    .await?;
    tx.commit().await?;
    crate::security_events::record(
        &st.pool,
        Some(p.tenant_id),
        crate::security_events::Kind::ExportReport,
        Some(p.user_id),
        Some(&p.ip),
        json!({ "report": "org", "report_sha256": hex::encode(sha) }),
    )
    .await;
    let mut res = Json(body).into_response();
    res.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"kizuna-org-report.json\""),
    );
    Ok(res)
}

const SETTINGS_KEYS: [&str; 6] = ["default_kind", "max_upload_mb", "csv", "xlsx", "pdf", "mapping"];

fn scalar_ok(v: &Value) -> bool {
    v.is_boolean() || v.as_u64().is_some_and(|n| n <= 1_000_000) || v.as_str().is_some_and(|s| s.chars().count() <= 100)
}

/// Current import settings (gap #1).
pub async fn get_settings(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::UserRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let row: Option<(i32, Value, Uuid, OffsetDateTime)> = sqlx::query_as(
        "SELECT version, settings, updated_by, updated_at FROM import_settings ORDER BY version DESC LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(match row {
        Some((v, s, by, at)) => {
            json!({ "version": v, "settings": s, "updated_by": by, "updated_at": at.unix_timestamp() })
        }
        None => json!({ "version": 0, "settings": {} }),
    }))
}

#[derive(Deserialize)]
pub struct PutSettings {
    pub expected_version: i32,
    pub settings: Value,
}

/// Saves a new version (privileged: tenant admin with MFA; audited; optimistic concurrency).
pub async fn put_settings(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<PutSettings>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::UserManage).await?;
    let Some(obj) = req.settings.as_object() else {
        return Err(ApiError::BadRequest("invalid_settings"));
    };
    if req.settings.to_string().len() > 8192 {
        return Err(ApiError::BadRequest("invalid_settings"));
    }
    for (k, v) in obj {
        let ok = SETTINGS_KEYS.contains(&k.as_str())
            && (scalar_ok(v)
                || v.as_object()
                    .is_some_and(|o| o.len() <= 20 && o.iter().all(|(k2, x)| k2.len() <= 40 && scalar_ok(x))));
        if !ok {
            return Err(ApiError::BadRequest("invalid_settings"));
        }
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 23))")
        .bind(p.tenant_id.to_string())
        .execute(&mut *tx)
        .await?;
    let previous: Value = sqlx::query_scalar("SELECT settings FROM import_settings ORDER BY version DESC LIMIT 1")
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or_else(|| json!({}));
    let cur: i32 = sqlx::query_scalar("SELECT coalesce(max(version), 0) FROM import_settings")
        .fetch_one(&mut *tx)
        .await?;
    if cur != req.expected_version {
        return Err(ApiError::Conflict("settings_version_conflict"));
    }
    sqlx::query("INSERT INTO import_settings (tenant_id, version, settings, updated_by) VALUES ($1,$2,$3,$4)")
        .bind(p.tenant_id)
        .bind(cur + 1)
        .bind(&req.settings)
        .bind(p.user_id)
        .execute(&mut *tx)
        .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "settings.import_updated",
            "tenant",
            p.tenant_id,
            json!({ "version": cur + 1, "previous": previous, "settings": req.settings }),
        ),
    )
    .await?;
    tx.commit().await?;
    // The long-table mapping and limits change how every future document becomes claims (review L3).
    crate::tenant_settings::alert_change(&st, &p, "import", cur + 1, false).await;
    Ok(Json(json!({ "version": cur + 1 })))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct SessionOut {
    pub id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub mfa_verified: bool,
    pub client_ip: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
    pub revoked_reason: Option<String>,
}

/// A user's sessions (gap #9). No token material is ever returned.
pub async fn user_sessions(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<SessionOut>>> {
    p.require(&st, Permission::UserRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, SessionOut>(
        "SELECT id, created_at, expires_at, mfa_verified, client_ip, revoked_at, revoked_reason FROM sessions \
         WHERE user_id = $1 ORDER BY created_at DESC LIMIT 100",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

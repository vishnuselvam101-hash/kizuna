//! M5 automated incident response (defensive only). Detection rules come from `security-detection`; this module
//! records incidents, executes pre-authorised containment, queues high-impact actions for MFA-verified approval,
//! and supports reverting actions and emergency recovery. Never acts on anything outside KIZUNA.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, State};
use policy_engine::Permission;
use security_detection::{ActionKind, Detection, Event, Policy, Thresholds, detect};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::{Principal, revoke_user_sessions};
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiResult};
use crate::security_events::{self as sec, Kind};

pub const BLOCK_SECS: i64 = 3600;
pub const UPLOAD_RESTRICT_SECS: i64 = 3600;
pub const EXTRACTION_SUSPEND_SECS: i64 = 1800;

type EvRow = (Uuid, String, Option<Uuid>, Option<String>, OffsetDateTime, Value);

type BaselineCache = std::sync::Mutex<std::collections::HashMap<Uuid, (i64, Thresholds)>>;
static BASELINES: std::sync::OnceLock<BaselineCache> = std::sync::OnceLock::new();
const BASELINE_TTL_SECS: i64 = 600;

/// Drops the cached adaptive thresholds of a tenant (after its settings change).
pub fn forget_baseline(tenant: Uuid) {
    if let Ok(mut m) = BASELINES.get_or_init(Default::default).lock() {
        m.remove(&tenant);
    }
}

/// The tenant's thresholds: configured values (defaults if unset); in adaptive mode raised from the tenant's own
/// history, recomputed at most every 10 minutes. Events that opened an incident (unless closed as false positive)
/// never count toward the baseline (review M5-M7 L2).
pub async fn thresholds_for(tx: &mut Tx, tenant: Uuid) -> ApiResult<Thresholds> {
    let settings = crate::tenant_settings::detection(tx).await?;
    if settings.mode == security_detection::Mode::Static {
        return Ok(settings.thresholds);
    }
    let now = OffsetDateTime::now_utc().unix_timestamp();
    if let Ok(m) = BASELINES.get_or_init(Default::default).lock()
        && let Some((at, t)) = m.get(&tenant)
        && now - at < BASELINE_TTL_SECS
    {
        return Ok(t.clone());
    }
    let rows: Vec<(String, Option<Uuid>, OffsetDateTime)> = sqlx::query_as(
        "SELECT e.kind, e.user_id, e.occurred_at FROM security_events e WHERE e.tenant_id = $1 \
         AND e.occurred_at > now() - make_interval(days => $2) AND e.occurred_at <= now() - interval '1 hour' \
         AND e.kind IN ('authz.denied', 'admin.role_changed', 'admin.user_disabled', 'auth.mfa_reset', \
         'auth.sessions_revoked', 'admin.settings_changed', 'ingest.upload_rejected', 'ingest.artifact_quarantined', \
         'ingest.extraction_failed', 'export.report') \
         AND NOT EXISTS (SELECT 1 FROM incidents i WHERE i.status <> 'false_positive' AND i.event_ids ? e.id::text) \
         ORDER BY e.occurred_at DESC LIMIT 100000",
    )
    .bind(tenant)
    .bind(settings.baseline_days as i32)
    .fetch_all(&mut **tx)
    .await?;
    let history: Vec<Event> = rows
        .into_iter()
        .enumerate()
        .map(|(i, (kind, user, at))| Event {
            id: i.to_string(),
            kind,
            user: user.map(|u| u.to_string()),
            ip: None,
            at: at.unix_timestamp(),
            details: Default::default(),
        })
        .collect();
    let t = security_detection::effective_thresholds(&settings, &history, now);
    if let Ok(mut m) = BASELINES.get_or_init(Default::default).lock() {
        if m.len() > 10_000 {
            m.clear();
        }
        m.insert(tenant, (now, t.clone()));
    }
    Ok(t)
}

/// Runs all detection rules over the tenant's last hour of security events.
pub async fn run_detection(pool: &PgPool, tenant: Uuid) -> ApiResult<usize> {
    let mut tx = db::tenant_tx(pool, tenant).await?;
    // Non-blocking: if a detection run for this tenant is in progress, skip (the 60 s sweep and the next event
    // catch up). Requests never queue on this lock (final review H1).
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 17))")
        .bind(tenant.to_string())
        .fetch_one(&mut *tx)
        .await?;
    if !got {
        return Ok(0);
    }
    // Newest first, so a flood of old events can never hide recent ones.
    let rows: Vec<EvRow> = sqlx::query_as(
        "SELECT id, kind, user_id, client_ip, occurred_at, details FROM security_events \
         WHERE tenant_id = $1 AND occurred_at > now() - interval '1 hour' ORDER BY occurred_at DESC LIMIT 20000",
    )
    .bind(tenant)
    .fetch_all(&mut *tx)
    .await?;
    let events: Vec<Event> = rows
        .into_iter()
        .map(|r| Event {
            id: r.0.to_string(),
            kind: r.1,
            user: r.2.map(|u| u.to_string()),
            ip: r.3,
            at: r.4.unix_timestamp(),
            details: r
                .5
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect();
    let thresholds = thresholds_for(&mut tx, tenant).await?;
    let detections = detect(&events, OffsetDateTime::now_utc().unix_timestamp(), &thresholds);
    let mut touched = 0;
    let mut alerts = Vec::new();
    for d in detections {
        // Events already judged in a resolved / false-positive incident do not re-trigger it (final review M1).
        let covered: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM incidents WHERE rule = $1 AND subject = $2 AND status IN ('resolved', 'false_positive') \
             AND event_ids @> $3::jsonb)",
        )
        .bind(d.rule)
        .bind(&d.subject)
        .bind(json!(d.event_ids))
        .fetch_one(&mut *tx)
        .await?;
        if covered {
            continue;
        }
        touched += 1;
        if let Some(a) = upsert_incident(&mut tx, tenant, &d).await? {
            alerts.push(a);
        }
    }
    tx.commit().await?;
    for (kind, detail) in alerts {
        let _ = sec::try_record(pool, Some(tenant), kind, None, None, detail).await;
    }
    Ok(touched)
}

/// Creates or updates the open incident for (rule, subject) and handles its actions. Returns a security event to
/// record after commit when containment was applied.
async fn upsert_incident(tx: &mut Tx, tenant: Uuid, d: &Detection) -> ApiResult<Option<(Kind, Value)>> {
    let existing: Option<(Uuid, Value)> =
        sqlx::query_as("SELECT id, event_ids FROM incidents WHERE rule = $1 AND subject = $2 AND status IN ('open', 'contained') FOR UPDATE")
            .bind(d.rule)
            .bind(&d.subject)
            .fetch_optional(&mut **tx)
            .await?;
    let id = match existing {
        Some((id, ev)) => {
            let mut all: Vec<String> = serde_json::from_value(ev).unwrap_or_default();
            for e in &d.event_ids {
                if !all.contains(e) && all.len() < 500 {
                    all.push(e.clone());
                }
            }
            sqlx::query("UPDATE incidents SET event_ids = $2, last_seen = now(), summary = $3 WHERE id = $1")
                .bind(id)
                .bind(json!(all))
                .bind(&d.summary)
                .execute(&mut **tx)
                .await?;
            id
        }
        None => {
            let id = Uuid::now_v7();
            sqlx::query("INSERT INTO incidents (id, tenant_id, rule, severity, subject, summary, event_ids) VALUES ($1,$2,$3,$4,$5,$6,$7)")
                .bind(id)
                .bind(tenant)
                .bind(d.rule)
                .bind(d.severity)
                .bind(&d.subject)
                .bind(&d.summary)
                .bind(json!(d.event_ids))
                .execute(&mut **tx)
                .await?;
            audit::append(tx, tenant, Record { actor_user_id: None, actor_kind: "system", action: "incident.opened", target_type: "incident",
                target_id: Some(id.to_string()), details: json!({ "rule": d.rule, "severity": d.severity, "subject": d.subject, "rules": security_detection::RULES_VERSION }) }).await?;
            id
        }
    };
    let mut applied = Vec::new();
    for a in &d.actions {
        let exists: Option<(Uuid, String, OffsetDateTime)> = sqlx::query_as(
            "SELECT id, status, updated_at FROM response_actions WHERE incident_id = $1 AND kind = $2 AND target = $3",
        )
        .bind(id)
        .bind(a.kind.code())
        .bind(&a.target)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some((aid, status, at)) = exists {
            // A returning attacker after a time-limited action expired is contained again (final review M1).
            let time_limited = matches!(
                a.kind,
                ActionKind::BlockIp | ActionKind::RestrictUploads | ActionKind::SuspendExtraction
            );
            let ids: Vec<Uuid> = d.event_ids.iter().filter_map(|e| Uuid::parse_str(e).ok()).collect();
            let fresh: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM security_events WHERE id = ANY($1) AND occurred_at > $2)",
            )
            .bind(&ids)
            .bind(at)
            .fetch_one(&mut **tx)
            .await?;
            if a.policy == Policy::PreAuthorised
                && time_limited
                && status == "executed"
                && fresh
                && !still_active(tx, a.kind, &a.target).await?
            {
                let (st, detail) = execute(tx, tenant, aid, a.kind, &a.target, false).await?;
                finish_action(tx, tenant, aid, st, &format!("re-applied: {detail}"), None).await?;
                if st == "executed" {
                    applied.push(json!({ "action": a.kind.code(), "target": a.target, "reapplied": true }));
                }
            }
            continue;
        }
        let aid = Uuid::now_v7();
        sqlx::query("INSERT INTO response_actions (id, tenant_id, incident_id, kind, target, policy, status) VALUES ($1,$2,$3,$4,$5,$6,'proposed')")
            .bind(aid)
            .bind(tenant)
            .bind(id)
            .bind(a.kind.code())
            .bind(&a.target)
            .bind(a.policy.code())
            .execute(&mut **tx)
            .await?;
        if a.policy == Policy::PreAuthorised {
            let (status, detail) = execute(tx, tenant, aid, a.kind, &a.target, false).await?;
            finish_action(tx, tenant, aid, status, &detail, None).await?;
            if status == "executed" && a.kind != ActionKind::NotifyAdmins {
                applied.push(json!({ "action": a.kind.code(), "target": a.target }));
            }
        }
    }
    let executed: i64 = sqlx::query_scalar("SELECT count(*) FROM response_actions WHERE incident_id = $1 AND status = 'executed' AND kind <> 'notify_admins'")
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    if executed > 0 {
        sqlx::query("UPDATE incidents SET status = 'contained' WHERE id = $1 AND status = 'open'")
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }
    Ok((!applied.is_empty()).then(|| {
        (
            Kind::ContainmentApplied,
            json!({ "incident_id": id, "rule": d.rule, "actions": applied }),
        )
    }))
}

async fn finish_action(
    tx: &mut Tx,
    tenant: Uuid,
    aid: Uuid,
    status: &str,
    detail: &str,
    by: Option<Uuid>,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE response_actions SET status = $2, detail = $3, decided_by = $4, updated_at = now() WHERE id = $1",
    )
    .bind(aid)
    .bind(status)
    .bind(detail)
    .bind(by)
    .execute(&mut **tx)
    .await?;
    audit::append(
        tx,
        tenant,
        Record {
            actor_user_id: by,
            actor_kind: if by.is_some() { "user" } else { "system" },
            action: "response.action",
            target_type: "response_action",
            target_id: Some(aid.to_string()),
            details: json!({ "status": status, "detail": detail }),
        },
    )
    .await?;
    Ok(())
}

async fn is_last_active_admin(tx: &mut Tx, user: Uuid) -> ApiResult<bool> {
    let admins: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE role = 'tenant_admin' AND status = 'active' FOR UPDATE")
            .fetch_all(&mut **tx)
            .await?;
    Ok(admins.len() == 1 && admins[0] == user)
}

/// Executes one containment action. Guards: never disable the last active administrator; never auto-block an IP an
/// active administrator used in the last 24 h (escalated to approval instead). Returns (status, detail).
async fn execute(
    tx: &mut Tx,
    tenant: Uuid,
    aid: Uuid,
    kind: ActionKind,
    target: &str,
    approved: bool,
) -> ApiResult<(&'static str, String)> {
    let user = || Uuid::parse_str(target).map_err(|_| ApiError::BadRequest("invalid_target"));
    Ok(match kind {
        ActionKind::NotifyAdmins => {
            tracing::warn!(%tenant, target, "security incident: administrators notified in-app");
            ("executed", "in-app notification (incident dashboard)".into())
        }
        ActionKind::RevokeUserSessions => {
            let n = revoke_user_sessions(tx, tenant, user()?, None, "incident_response").await?;
            ("executed", format!("{n} sessions revoked"))
        }
        ActionKind::LockAccount => {
            let u = user()?;
            if is_last_active_admin(tx, u).await? {
                (
                    "refused",
                    "would disable the last active administrator; use the emergency procedure instead".into(),
                )
            } else {
                sqlx::query(
                    "UPDATE users SET status = 'disabled', disabled_by_action = $2, updated_at = now() WHERE id = $1",
                )
                .bind(u)
                .bind(aid)
                .execute(&mut **tx)
                .await?;
                let n = revoke_user_sessions(tx, tenant, u, None, "account_locked_incident").await?;
                ("executed", format!("account disabled, {n} sessions revoked"))
            }
        }
        ActionKind::BlockIp => {
            let admin_ip: Option<String> = sqlx::query_scalar(
                "SELECT s.client_ip FROM sessions s JOIN users u ON u.id = s.user_id WHERE u.role = 'tenant_admin' AND u.status = 'active' \
                 AND s.client_ip = $1 AND s.created_at > now() - interval '24 hours' LIMIT 1",
            )
            .bind(target)
            .fetch_optional(&mut **tx)
            .await?;
            if admin_ip.is_some() && !approved {
                sqlx::query("UPDATE response_actions SET policy = 'requires_approval' WHERE id = $1")
                    .bind(aid)
                    .execute(&mut **tx)
                    .await?;
                return Ok((
                    "proposed",
                    "address used by an active administrator; escalated for approval".into(),
                ));
            }
            sqlx::query(
                "INSERT INTO ip_blocks (tenant_id, ip, until, action_id) VALUES ($1,$2, now() + make_interval(secs => $3), $4) \
                 ON CONFLICT (tenant_id, ip) DO UPDATE SET until = EXCLUDED.until, action_id = EXCLUDED.action_id",
            )
            .bind(tenant)
            .bind(target)
            .bind(BLOCK_SECS as f64)
            .bind(aid)
            .execute(&mut **tx)
            .await?;
            (
                "executed",
                format!("logins from {target} blocked for {} minutes", BLOCK_SECS / 60),
            )
        }
        ActionKind::QuarantineArtifact => {
            let a = Uuid::parse_str(target).map_err(|_| ApiError::BadRequest("invalid_target"))?;
            sqlx::query(
                "UPDATE artifacts SET status = 'quarantined', scan_result = scan_result || jsonb_build_object('quarantined_by_incident', $2::text) WHERE id = $1",
            )
            .bind(a)
            .bind(aid.to_string())
            .execute(&mut **tx)
            .await?;
            ("executed", "artifact quarantined".into())
        }
        ActionKind::RestrictUploads => {
            sqlx::query(
                "UPDATE users SET upload_blocked_until = now() + make_interval(secs => $2), upload_blocked_by_action = $3 \
                 WHERE id = $1",
            )
            .bind(user()?)
            .bind(UPLOAD_RESTRICT_SECS as f64)
            .bind(aid)
            .execute(&mut **tx)
            .await?;
            (
                "executed",
                format!("uploads restricted for {} minutes", UPLOAD_RESTRICT_SECS / 60),
            )
        }
        ActionKind::SuspendExtraction => {
            sqlx::query(
                "INSERT INTO tenant_controls (tenant_id, extraction_suspended_until) VALUES ($1, now() + make_interval(secs => $2)) \
                 ON CONFLICT (tenant_id) DO UPDATE SET extraction_suspended_until = EXCLUDED.extraction_suspended_until",
            )
            .bind(tenant)
            .bind(EXTRACTION_SUSPEND_SECS as f64)
            .execute(&mut **tx)
            .await?;
            (
                "executed",
                format!("extraction suspended for {} minutes", EXTRACTION_SUSPEND_SECS / 60),
            )
        }
    })
}

async fn still_active(tx: &mut Tx, kind: ActionKind, target: &str) -> ApiResult<bool> {
    let q = match kind {
        ActionKind::BlockIp => "SELECT EXISTS (SELECT 1 FROM ip_blocks WHERE ip = $1 AND until > now())",
        ActionKind::RestrictUploads => {
            "SELECT EXISTS (SELECT 1 FROM users WHERE id::text = $1 AND upload_blocked_until > now())"
        }
        ActionKind::SuspendExtraction => {
            "SELECT EXISTS (SELECT 1 FROM tenant_controls WHERE extraction_suspended_until > now() AND $1 = $1)"
        }
        _ => return Ok(false),
    };
    Ok(sqlx::query_scalar(q).bind(target).fetch_one(&mut **tx).await?)
}

/// Undoes a containment action where that is possible.
async fn revert(
    tx: &mut Tx,
    tenant: Uuid,
    aid: Uuid,
    kind: &str,
    target: &str,
) -> ApiResult<Result<String, &'static str>> {
    Ok(match kind {
        "lock_account" => {
            let u = Uuid::parse_str(target).map_err(|_| ApiError::BadRequest("invalid_target"))?;
            // Only undo this action's own lock, never a later deliberate disable (final review M4).
            let n = sqlx::query(
                "UPDATE users SET status = 'active', disabled_by_action = NULL, failed_logins = 0, locked_until = NULL \
                 WHERE id = $1 AND disabled_by_action = $2",
            )
            .bind(u)
            .bind(aid)
            .execute(&mut **tx)
            .await?
            .rows_affected();
            if n == 0 {
                return Ok(Err("account_disabled_for_another_reason"));
            }
            Ok("account re-enabled".into())
        }
        "block_ip" => {
            // Only this action's own block: a newer block of the same address stays (review M5-M7 L5).
            let n = sqlx::query("DELETE FROM ip_blocks WHERE ip = $1 AND action_id = $2")
                .bind(target)
                .bind(aid)
                .execute(&mut **tx)
                .await?
                .rows_affected();
            Ok(if n == 0 {
                "block already replaced by a newer action; left in place".into()
            } else {
                "block lifted".into()
            })
        }
        "quarantine_artifact" => Err("release_via_artifact_review"),
        "restrict_uploads" => {
            let u = Uuid::parse_str(target).map_err(|_| ApiError::BadRequest("invalid_target"))?;
            let n = sqlx::query(
                "UPDATE users SET upload_blocked_until = NULL, upload_blocked_by_action = NULL \
                 WHERE id = $1 AND upload_blocked_by_action = $2",
            )
            .bind(u)
            .bind(aid)
            .execute(&mut **tx)
            .await?
            .rows_affected();
            Ok(if n == 0 {
                "restriction already replaced by a newer action; left in place".into()
            } else {
                "upload restriction lifted".into()
            })
        }
        "suspend_extraction" => {
            sqlx::query("UPDATE tenant_controls SET extraction_suspended_until = NULL WHERE tenant_id = $1")
                .bind(tenant)
                .execute(&mut **tx)
                .await?;
            Ok("extraction resumed".into())
        }
        _ => Err("not_reversible"),
    })
}

/// Emergency recovery (operator CLI): re-enables all administrators of a tenant, clears their lockouts, lifts all
/// IP blocks and restrictions. Audited. Guarantees containment can never lock out every administrator permanently.
pub async fn emergency_unlock(pool: &PgPool, tenant: Uuid) -> ApiResult<Value> {
    let mut tx = db::tenant_tx(pool, tenant).await?;
    // Re-enable only administrators disabled by automated containment; deliberately disabled accounts stay
    // disabled (final review M4). Lockouts and restrictions are cleared for all administrators.
    let reenabled: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE users SET status = 'active', disabled_by_action = NULL WHERE role = 'tenant_admin' AND status = 'disabled' \
         AND disabled_by_action IS NOT NULL RETURNING id",
    )
    .fetch_all(&mut *tx)
    .await?;
    let unlocked: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE users SET failed_logins = 0, locked_until = NULL, upload_blocked_until = NULL WHERE role = 'tenant_admin' RETURNING id",
    )
    .fetch_all(&mut *tx)
    .await?;
    let admins = reenabled.len() as u64;
    let blocks = sqlx::query("DELETE FROM ip_blocks")
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sqlx::query("UPDATE tenant_controls SET extraction_suspended_until = NULL")
        .execute(&mut *tx)
        .await?;
    let out = json!({ "admins_reenabled": admins, "reenabled_ids": reenabled, "unlocked_admin_ids": unlocked, "ip_blocks_lifted": blocks });
    audit::append(
        &mut tx,
        tenant,
        Record {
            actor_user_id: None,
            actor_kind: "operator",
            action: "incident.emergency_unlock",
            target_type: "tenant",
            target_id: Some(tenant.to_string()),
            details: out.clone(),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(out)
}

/// An administrator never decides on containment that targets themselves (final review M3).
async fn not_about_self(st: &AppState, p: &Principal, subject: &str, target: Option<&str>) -> ApiResult<()> {
    let me = p.user_id.to_string();
    let about_me = subject == format!("user:{me}") || target == Some(me.as_str());
    p.require_on(
        st,
        Permission::IncidentManage,
        &policy_engine::Resource {
            tenant_id: p.tenant_id,
            kind: "incident",
            owner_user_id: about_me.then_some(p.user_id),
        },
    )
    .await
}

// ---- enforcement helpers ----

pub async fn ip_blocked(pool: &PgPool, tenant: Uuid, ip: &str) -> ApiResult<bool> {
    let mut tx = db::tenant_tx(pool, tenant).await?;
    let b: Option<i32> = sqlx::query_scalar("SELECT 1 FROM ip_blocks WHERE ip = $1 AND until > now()")
        .bind(ip)
        .fetch_optional(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(b.is_some())
}

pub async fn uploads_blocked(tx: &mut Tx, user: Uuid) -> ApiResult<bool> {
    let b: Option<Option<bool>> = sqlx::query_scalar("SELECT upload_blocked_until > now() FROM users WHERE id = $1")
        .bind(user)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(b.flatten().unwrap_or(false))
}

pub async fn extraction_suspended(tx: &mut Tx) -> ApiResult<bool> {
    let b: Option<Option<bool>> = sqlx::query_scalar("SELECT extraction_suspended_until > now() FROM tenant_controls")
        .fetch_optional(&mut **tx)
        .await?;
    Ok(b.flatten().unwrap_or(false))
}

// ---- API ----

#[derive(Serialize, sqlx::FromRow)]
pub struct IncidentOut {
    pub id: Uuid,
    pub rule: String,
    pub severity: String,
    pub subject: String,
    pub summary: String,
    pub status: String,
    pub event_ids: Value,
    #[serde(with = "time::serde::rfc3339")]
    pub first_seen: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen: OffsetDateTime,
    pub resolution: Option<String>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct ActionOut {
    pub id: Uuid,
    pub kind: String,
    pub target: String,
    pub policy: String,
    pub status: String,
    pub detail: Option<String>,
    pub decided_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

const INC_COLS: &str = "id, rule, severity, subject, summary, status, event_ids, first_seen, last_seen, resolution";
const ACT_COLS: &str = "id, kind, target, policy, status, detail, decided_by, updated_at";

pub async fn list(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Vec<IncidentOut>>> {
    p.require(&st, Permission::SecurityEventRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, IncidentOut>(&format!(
        "SELECT {INC_COLS} FROM incidents ORDER BY last_seen DESC LIMIT 200"
    ))
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

pub async fn get(State(st): State<AppState>, p: Principal, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::SecurityEventRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let inc = sqlx::query_as::<_, IncidentOut>(&format!("SELECT {INC_COLS} FROM incidents WHERE id = $1"))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    let actions = sqlx::query_as::<_, ActionOut>(&format!(
        "SELECT {ACT_COLS} FROM response_actions WHERE incident_id = $1 ORDER BY created_at"
    ))
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    let ids: Vec<Uuid> = serde_json::from_value::<Vec<String>>(inc.event_ids.clone())
        .unwrap_or_default()
        .iter()
        .filter_map(|s| Uuid::parse_str(s).ok())
        .collect();
    let events: Vec<Value> = sqlx::query_scalar(
        "SELECT json_build_object('id', id, 'kind', kind, 'severity', severity, 'user_id', user_id, 'client_ip', client_ip, 'at', occurred_at)::jsonb \
         FROM security_events WHERE id = ANY($1) ORDER BY occurred_at LIMIT 200",
    )
    .bind(&ids)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "incident": inc, "actions": actions, "events": events })))
}

#[derive(Deserialize)]
pub struct ActionReq {
    pub comment: Option<String>,
}

async fn load_action(tx: &mut Tx, incident: Uuid, action: Uuid) -> ApiResult<(String, String, String, String)> {
    sqlx::query_as(
        "SELECT kind, target, policy, status FROM response_actions WHERE id = $1 AND incident_id = $2 FOR UPDATE",
    )
    .bind(action)
    .bind(incident)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)
}

fn kind_from(code: &str) -> ApiResult<ActionKind> {
    use ActionKind::*;
    [
        RevokeUserSessions,
        LockAccount,
        BlockIp,
        QuarantineArtifact,
        RestrictUploads,
        SuspendExtraction,
        NotifyAdmins,
    ]
    .into_iter()
    .find(|k| k.code() == code)
    .ok_or_else(|| ApiError::internal("unknown action kind"))
}

/// Approve (execute) or reject a proposed high-impact action. Requires IncidentManage (admin + MFA).
pub async fn decide_action(
    State(st): State<AppState>,
    p: Principal,
    Path((incident, action, verb)): Path<(Uuid, Uuid, String)>,
    ApiJson(req): ApiJson<ActionReq>,
) -> ApiResult<Json<ActionOut>> {
    p.require(&st, Permission::IncidentManage).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (kind, target, _policy, status) = load_action(&mut tx, incident, action).await?;
    let (subject, inc_status): (String, String) = sqlx::query_as("SELECT subject, status FROM incidents WHERE id = $1")
        .bind(incident)
        .fetch_one(&mut *tx)
        .await?;
    not_about_self(&st, &p, &subject, Some(&target)).await?;
    // Actions of a closed incident can only be reverted, never approved (review M5-M7 L5).
    if verb == "approve" && !matches!(inc_status.as_str(), "open" | "contained") {
        return Err(ApiError::Conflict("incident_closed"));
    }
    let comment = req.comment.unwrap_or_default().chars().take(500).collect::<String>();
    match verb.as_str() {
        "approve" => {
            if status != "proposed" {
                return Err(ApiError::Conflict("action_not_proposed"));
            }
            let (s, detail) = execute(&mut tx, p.tenant_id, action, kind_from(&kind)?, &target, true).await?;
            finish_action(
                &mut tx,
                p.tenant_id,
                action,
                s,
                &format!("{detail}; approved: {comment}"),
                Some(p.user_id),
            )
            .await?;
            if s == "executed" {
                sqlx::query("UPDATE incidents SET status = 'contained' WHERE id = $1 AND status = 'open'")
                    .bind(incident)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        "reject" => {
            if status != "proposed" {
                return Err(ApiError::Conflict("action_not_proposed"));
            }
            finish_action(&mut tx, p.tenant_id, action, "rejected", &comment, Some(p.user_id)).await?;
        }
        "revert" => {
            if status != "executed" {
                return Err(ApiError::Conflict("action_not_executed"));
            }
            match revert(&mut tx, p.tenant_id, action, &kind, &target).await? {
                Ok(detail) => {
                    finish_action(
                        &mut tx,
                        p.tenant_id,
                        action,
                        "reverted",
                        &format!("{detail}; {comment}"),
                        Some(p.user_id),
                    )
                    .await?
                }
                Err(code) => return Err(ApiError::Conflict(code)),
            }
        }
        _ => return Err(ApiError::NotFound),
    }
    let out = sqlx::query_as::<_, ActionOut>(&format!("SELECT {ACT_COLS} FROM response_actions WHERE id = $1"))
        .bind(action)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct ResolveReq {
    pub status: String,
    pub comment: String,
}

pub async fn resolve(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(req): ApiJson<ResolveReq>,
) -> ApiResult<Json<IncidentOut>> {
    p.require(&st, Permission::IncidentManage).await?;
    if !matches!(req.status.as_str(), "resolved" | "false_positive")
        || req.comment.trim().is_empty()
        || req.comment.len() > 2000
    {
        return Err(ApiError::BadRequest("invalid_resolution"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let subject: Option<String> = sqlx::query_scalar("SELECT subject FROM incidents WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    not_about_self(&st, &p, &subject.ok_or(ApiError::NotFound)?, None).await?;
    let r = sqlx::query(
        "UPDATE incidents SET status = $2, resolution = $3 WHERE id = $1 AND status IN ('open', 'contained')",
    )
    .bind(id)
    .bind(&req.status)
    .bind(req.comment.trim())
    .execute(&mut *tx)
    .await?;
    if r.rows_affected() == 0 {
        return Err(ApiError::Conflict("incident_not_open"));
    }
    // Pending proposals of a closed incident are closed with it.
    sqlx::query(
        "UPDATE response_actions SET status = 'rejected', detail = 'incident closed', decided_by = $2, updated_at = now() \
         WHERE incident_id = $1 AND status = 'proposed'",
    )
    .bind(id)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "incident.resolved",
            "incident",
            id,
            json!({ "status": req.status }),
        ),
    )
    .await?;
    let out = sqlx::query_as::<_, IncidentOut>(&format!("SELECT {INC_COLS} FROM incidents WHERE id = $1"))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Security dashboard summary (last 24 h).
pub async fn summary(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::SecurityEventRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let by_kind: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT kind, severity, count(*) FROM security_events WHERE tenant_id = kz_tenant_id() AND occurred_at > now() - interval '24 hours' GROUP BY 1, 2 ORDER BY 3 DESC",
    )
    .fetch_all(&mut *tx)
    .await?;
    let open: i64 = sqlx::query_scalar("SELECT count(*) FROM incidents WHERE status IN ('open', 'contained')")
        .fetch_one(&mut *tx)
        .await?;
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM response_actions WHERE status = 'proposed'")
        .fetch_one(&mut *tx)
        .await?;
    let blocks: Vec<(String, OffsetDateTime)> = sqlx::query_as("SELECT ip, until FROM ip_blocks WHERE until > now()")
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut kinds = BTreeMap::new();
    for (k, s, n) in by_kind {
        kinds.insert(k, json!({ "severity": s, "count": n }));
    }
    Ok(Json(json!({
        "events_24h": kinds,
        "open_incidents": open,
        "actions_awaiting_approval": pending,
        "active_ip_blocks": blocks.into_iter().map(|(ip, until)| json!({ "ip": ip, "until": until.unix_timestamp() })).collect::<Vec<_>>(),
    })))
}

pub async fn trigger(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::IncidentManage).await?;
    let n = run_detection(&st.pool, p.tenant_id).await?;
    Ok(Json(json!({ "detections": n })))
}

/// Periodic sweep over all active tenants (started by the server binary).
pub async fn sweep_all(pool: &PgPool) -> ApiResult<()> {
    let tenants: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM tenants WHERE status = 'active'")
        .fetch_all(pool)
        .await?;
    for t in tenants {
        if let Err(e) = run_detection(pool, t).await {
            tracing::error!(tenant = %t, error = %e, "detection sweep failed");
        }
    }
    Ok(())
}

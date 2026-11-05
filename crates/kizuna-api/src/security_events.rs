//! Security event recording. Written in their own transaction so they persist even when the triggering
//! request fails. Never include passwords, tokens or document content in `details`.

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::Principal;
use crate::db;
use crate::error::{ApiQuery, ApiResult};
use policy_engine::Permission;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    LoginFailed,
    LoginSucceeded,
    AccountLocked,
    RateLimited,
    RefreshTokenReuse,
    AuthzDenied,
    InvalidToken,
    MfaEnrolled,
    MfaReset,
    PasswordChanged,
    RoleChanged,
    UserDisabled,
    SessionsRevoked,
    AuditChainBroken,
    ArtifactIntegrityFailure,
    UploadRejected,
    ArtifactQuarantined,
    QuarantineReleased,
    ExtractionFailed,
    ContainmentApplied,
    ExportReport,
    BlockedRequest,
    /// A document label looked like an instruction to the AI and was refused (ADR 0016).
    PromptInjectionSuspected,
}

impl Kind {
    pub fn code(self) -> &'static str {
        match self {
            Kind::LoginFailed => "auth.login_failed",
            Kind::LoginSucceeded => "auth.login_succeeded",
            Kind::AccountLocked => "auth.account_locked",
            Kind::RateLimited => "auth.rate_limited",
            Kind::RefreshTokenReuse => "auth.refresh_token_reuse",
            Kind::AuthzDenied => "authz.denied",
            Kind::InvalidToken => "auth.invalid_token",
            Kind::MfaEnrolled => "auth.mfa_enrolled",
            Kind::MfaReset => "auth.mfa_reset",
            Kind::PasswordChanged => "auth.password_changed",
            Kind::RoleChanged => "admin.role_changed",
            Kind::UserDisabled => "admin.user_disabled",
            Kind::SessionsRevoked => "auth.sessions_revoked",
            Kind::AuditChainBroken => "integrity.audit_chain_broken",
            Kind::ArtifactIntegrityFailure => "integrity.artifact_mismatch",
            Kind::UploadRejected => "ingest.upload_rejected",
            Kind::ArtifactQuarantined => "ingest.artifact_quarantined",
            Kind::QuarantineReleased => "response.quarantine_released",
            Kind::ExtractionFailed => "ingest.extraction_failed",
            Kind::ContainmentApplied => "response.containment_applied",
            Kind::ExportReport => "export.report",
            Kind::BlockedRequest => "response.blocked_request",
            Kind::PromptInjectionSuspected => "ingest.prompt_injection_suspected",
        }
    }

    pub fn severity(self) -> &'static str {
        match self {
            Kind::LoginSucceeded | Kind::MfaEnrolled | Kind::PasswordChanged | Kind::ExportReport => "info",
            Kind::LoginFailed | Kind::InvalidToken | Kind::SessionsRevoked | Kind::ExtractionFailed => "low",
            Kind::AccountLocked
            | Kind::RateLimited
            | Kind::AuthzDenied
            | Kind::MfaReset
            | Kind::RoleChanged
            | Kind::UserDisabled
            | Kind::UploadRejected
            | Kind::ArtifactQuarantined
            | Kind::QuarantineReleased
            | Kind::ContainmentApplied
            | Kind::BlockedRequest
            | Kind::PromptInjectionSuspected => "medium",
            Kind::RefreshTokenReuse => "high",
            Kind::AuditChainBroken | Kind::ArtifactIntegrityFailure => "critical",
        }
    }
}

/// Records an event; failures are logged, never propagated (the security log must not break requests).
pub async fn record(
    pool: &PgPool,
    tenant: Option<Uuid>,
    kind: Kind,
    user: Option<Uuid>,
    ip: Option<&str>,
    details: Value,
) {
    if let Err(e) = try_record(pool, tenant, kind, user, ip, details).await {
        tracing::error!(kind = kind.code(), error = %e, "failed to record security event");
        return;
    }
    // High-signal events trigger detection immediately; a periodic sweep covers the rest.
    if let Some(t) = tenant
        && kind.triggers_detection()
        && let Err(e) = crate::incidents::run_detection(pool, t).await
    {
        tracing::error!(error = %e, "detection failed");
    }
}

impl Kind {
    pub fn triggers_detection(self) -> bool {
        matches!(
            self,
            Kind::LoginFailed
                | Kind::RefreshTokenReuse
                | Kind::AuthzDenied
                | Kind::RoleChanged
                | Kind::UserDisabled
                | Kind::MfaReset
                | Kind::SessionsRevoked
                | Kind::ArtifactIntegrityFailure
                | Kind::AuditChainBroken
                | Kind::UploadRejected
                | Kind::ArtifactQuarantined
                | Kind::ExtractionFailed
                | Kind::ExportReport
        )
    }
}

pub async fn try_record(
    pool: &PgPool,
    tenant: Option<Uuid>,
    kind: Kind,
    user: Option<Uuid>,
    ip: Option<&str>,
    details: Value,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    if let Some(t) = tenant {
        db::set_tenant(&mut tx, t).await?;
    }
    sqlx::query(
        "INSERT INTO security_events (id, tenant_id, kind, severity, user_id, client_ip, details) \
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant)
    .bind(kind.code())
    .bind(kind.severity())
    .bind(user)
    .bind(ip)
    .bind(details)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::info!(kind = kind.code(), severity = kind.severity(), tenant = ?tenant, "security event");
    Ok(())
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
    pub kind: Option<String>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct SecurityEventOut {
    pub id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub kind: String,
    pub severity: String,
    pub user_id: Option<Uuid>,
    pub client_ip: Option<String>,
    pub details: Value,
}

pub async fn list(
    State(st): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> ApiResult<Json<Vec<SecurityEventOut>>> {
    p.require(&st, Permission::SecurityEventRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, SecurityEventOut>(
        "SELECT id, occurred_at, kind, severity, user_id, client_ip, details FROM security_events \
         WHERE tenant_id = kz_tenant_id() AND ($1::text IS NULL OR kind = $1) ORDER BY occurred_at DESC LIMIT $2",
    )
    .bind(q.kind)
    .bind(q.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

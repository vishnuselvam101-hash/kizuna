//! Supplier portal (ADR 0025). A supplier user is a separate, least-privilege principal scoped to one supplier record
//! of one tenant. It can list the requests a person approved for its supplier, read their text, upload documents
//! against them and see each upload's status — nothing else. Its token (`kzp.`) is refused by the tenant API and the
//! tenant token (`kza.`) by the portal; reads run in `db::portal_tx`, where RLS denies every row outside the scope.
//! Uploads go through the one ingestion pipeline (`artifacts::ingest`) marked `origin = supplier_portal`; extracted
//! claims are reviewed by tenant reviewers as usual, so a supplier upload never approves anything.

use axum::Json;
use axum::extract::{FromRequestParts, Multipart, Path, State};
use axum::http::request::Parts;
use domain_manufacturing::resupply::{self, Lang, RequestDoc};
use policy_engine::Permission;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::app::AppState;
use crate::artifacts::{self, Session, Uploader};
use crate::audit::{self, Record};
use crate::auth::{self, ClientIp, Principal};
use crate::crypto::{self, TokenKind};
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiResult};
use crate::evidence_requests::iso_date;
use crate::security_events::{self as sec, Kind};

/// Invitation links are single-use and expire after this many hours.
pub const INVITE_TTL_HOURS: i64 = 72;
/// A portal session lasts at most 8 hours and ends after 30 idle minutes (no refresh tokens: sign in again).
pub const SESSION_TTL_SECS: i64 = 8 * 3600;
pub const IDLE_TTL_SECS: i64 = 30 * 60;
/// Recorded portal denials per source and minute (a flood is still denied, just not recorded unboundedly).
const DENIAL_EVENTS_PER_MIN: u32 = 30;

/// Records a refused portal request as a security event (rate-bounded per source). Never fails the request path.
pub async fn deny(st: &AppState, tenant: Uuid, supplier_user: Option<Uuid>, ip: &str, reason: &str, path: &str) {
    if !st.limiter.check(&format!("portal-denied:{ip}"), DENIAL_EVENTS_PER_MIN) {
        return;
    }
    // The tenant id inside a token is caller-supplied: record it only when it names a real tenant.
    let known: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM tenants WHERE id = $1)")
        .bind(tenant)
        .fetch_one(&st.pool)
        .await
        .unwrap_or(false);
    sec::record(
        &st.pool,
        known.then_some(tenant),
        Kind::PortalDenied,
        None,
        Some(ip),
        json!({ "reason": reason, "path": path.chars().take(200).collect::<String>(), "supplier_user": supplier_user,
                "claimed_tenant": (!known).then_some(tenant) }),
    )
    .await;
}

/// The normalised supplier name used for `suppliers.name_norm`.
pub fn supplier_norm(name: &str) -> String {
    norm(name)
}

fn norm(name: &str) -> String {
    entity_resolution::safe_normalize(name.trim())
        .chars()
        .take(200)
        .collect()
}

fn audit_rec<'a>(
    actor: Uuid,
    action: &'a str,
    target_type: &'a str,
    target: impl ToString,
    details: Value,
) -> Record<'a> {
    Record {
        actor_user_id: Some(actor),
        actor_kind: "supplier_user",
        action,
        target_type,
        target_id: Some(target.to_string()),
        details,
    }
}

// ---------------------------------------------------------------- tenant admin: invite / list / revoke

#[derive(Serialize, sqlx::FromRow)]
pub struct SupplierUserOut {
    pub id: Uuid,
    pub supplier_id: Uuid,
    pub supplier: String,
    pub email: String,
    pub display_name: String,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub invited_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub invite_expires_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub activated_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
}

const SU_COLS: &str = "u.id, u.supplier_id, s.name AS supplier, u.email, u.display_name, u.status, u.invited_at, \
                       u.invite_expires_at, u.activated_at, u.revoked_at";

async fn fetch_su(tx: &mut Tx, id: Uuid) -> ApiResult<SupplierUserOut> {
    sqlx::query_as::<_, SupplierUserOut>(&format!(
        "SELECT {SU_COLS} FROM supplier_users u JOIN suppliers s ON s.tenant_id = u.tenant_id AND s.id = u.supplier_id \
         WHERE u.id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)
}

/// GET /portal-users — supplier portal accounts of the tenant.
pub async fn list_users(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Vec<SupplierUserOut>>> {
    p.require(&st, Permission::UserRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, SupplierUserOut>(&format!(
        "SELECT {SU_COLS} FROM supplier_users u JOIN suppliers s ON s.tenant_id = u.tenant_id AND s.id = u.supplier_id \
         ORDER BY u.invited_at DESC LIMIT 500"
    ))
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct InviteReq {
    /// An existing supplier record, or…
    pub supplier_id: Option<Uuid>,
    /// …the name of a NEW supplier record (refused if a record with the same normalised name exists: the admin then
    /// picks it by id, so similar names never silently merge into one scope).
    pub supplier: Option<String>,
    pub email: String,
    pub display_name: String,
}

#[derive(Serialize)]
pub struct InviteResp {
    pub user: SupplierUserOut,
    /// Shown once; only its hash is stored. The admin sends the link to the supplier by their own channel.
    pub invite_token: String,
}

/// POST /portal-users — invite a supplier user (tenant admin, MFA-verified session).
pub async fn invite(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<InviteReq>,
) -> ApiResult<Json<InviteResp>> {
    p.require(&st, Permission::UserManage).await?;
    let email = auth::normalize_email(&req.email);
    let name = req.display_name.trim();
    if email.len() < 3 || email.len() > 254 || !email.contains('@') || email.chars().any(char::is_whitespace) {
        return Err(ApiError::BadRequest("invalid_email"));
    }
    if name.is_empty() || name.chars().count() > 200 {
        return Err(ApiError::BadRequest("invalid_display_name"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let sid = resolve_supplier(&mut tx, &p, req.supplier_id, req.supplier.as_deref()).await?;
    let taken: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM supplier_users WHERE email = $1 AND status <> 'revoked'")
            .bind(&email)
            .fetch_optional(&mut *tx)
            .await?;
    if taken.is_some() {
        return Err(ApiError::Conflict("email_exists"));
    }
    let token = crypto::issue_token(TokenKind::PortalInvite, p.tenant_id);
    let id = Uuid::now_v7();
    let expires = st.clock.now() + Duration::hours(INVITE_TTL_HOURS);
    sqlx::query(
        "INSERT INTO supplier_users (id, tenant_id, supplier_id, email, display_name, invite_hash, invite_expires_at, \
         invited_by, invited_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(id)
    .bind(p.tenant_id)
    .bind(sid)
    .bind(&email)
    .bind(name)
    .bind(token.hash.to_vec())
    .bind(expires)
    .bind(p.user_id)
    .bind(st.clock.now())
    .execute(&mut *tx)
    .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "portal_user.invited",
            "supplier_user",
            id,
            json!({ "email": email, "supplier_id": sid, "expires_at": expires.unix_timestamp() }),
        ),
    )
    .await?;
    let user = fetch_su(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(InviteResp {
        user,
        invite_token: token.value,
    }))
}

/// The supplier record for an invitation: an existing one by id, or a new one by name (never matched by name).
async fn resolve_supplier(tx: &mut Tx, p: &Principal, id: Option<Uuid>, name: Option<&str>) -> ApiResult<Uuid> {
    match (id, name.map(str::trim)) {
        (Some(id), None) => sqlx::query_scalar("SELECT id FROM suppliers WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(ApiError::BadRequest("unknown_supplier")),
        (None, Some(name)) => {
            let n = norm(name);
            if n.is_empty() || name.chars().count() > 200 {
                return Err(ApiError::BadRequest("invalid_supplier"));
            }
            let id = Uuid::now_v7();
            let created = sqlx::query(
                "INSERT INTO suppliers (id, tenant_id, name, name_norm, created_by) VALUES ($1,$2,$3,$4,$5) \
                 ON CONFLICT (tenant_id, name_norm) DO NOTHING",
            )
            .bind(id)
            .bind(p.tenant_id)
            .bind(name)
            .bind(&n)
            .bind(p.user_id)
            .execute(&mut **tx)
            .await?
            .rows_affected();
            if created == 0 {
                return Err(ApiError::Conflict("supplier_exists"));
            }
            audit::append(
                tx,
                p.tenant_id,
                Record::user(p.user_id, "supplier.created", "supplier", id, json!({ "name": name })),
            )
            .await?;
            Ok(id)
        }
        _ => Err(ApiError::BadRequest("invalid_supplier")),
    }
}

#[derive(Serialize, sqlx::FromRow)]
pub struct SupplierOut {
    pub id: Uuid,
    pub name: String,
}

/// GET /suppliers — supplier records of the tenant (for invitations and request attachment).
pub async fn list_suppliers(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Vec<SupplierOut>>> {
    p.require(&st, Permission::UserRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, SupplierOut>("SELECT id, name FROM suppliers ORDER BY name LIMIT 1000")
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct AttachReq {
    pub supplier_id: Uuid,
}

/// POST /evidence-requests/{id}/attach-supplier — addresses a request to a supplier record (tenant admin, audited).
/// The only way a request becomes visible in the portal; the link is final (database guard).
pub async fn attach_supplier(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(req): ApiJson<AttachReq>,
) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::UserManage).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (current, text): (Option<Uuid>, String) =
        sqlx::query_as("SELECT supplier_id, supplier FROM evidence_requests WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    if current.is_some() {
        return Err(ApiError::Conflict("supplier_already_attached"));
    }
    let name: String = sqlx::query_scalar("SELECT name FROM suppliers WHERE id = $1")
        .bind(req.supplier_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::BadRequest("unknown_supplier"))?;
    sqlx::query("UPDATE evidence_requests SET supplier_id = $2 WHERE id = $1")
        .bind(id)
        .bind(req.supplier_id)
        .execute(&mut *tx)
        .await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "evidence_request.supplier_attached",
            "evidence_request",
            id,
            json!({ "supplier_id": req.supplier_id, "supplier": name, "request_supplier_text": text }),
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "id": id, "supplier_id": req.supplier_id })))
}

/// POST /portal-users/{id}/revoke — ends access at once: open invitation voided, every session revoked.
pub async fn revoke(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<SupplierUserOut>> {
    p.require(&st, Permission::UserManage).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let status: String = sqlx::query_scalar("SELECT status FROM supplier_users WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    if status == "revoked" {
        return Err(ApiError::Conflict("already_revoked"));
    }
    sqlx::query(
        "UPDATE supplier_users SET status = 'revoked', revoked_at = $2, revoked_by = $3, invite_hash = NULL, \
         totp_pending_enc = NULL WHERE id = $1",
    )
    .bind(id)
    .bind(st.clock.now())
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    let ended = sqlx::query(
        "UPDATE supplier_sessions SET revoked_at = $2, revoke_reason = 'account_revoked' \
         WHERE supplier_user_id = $1 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(st.clock.now())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "portal_user.revoked",
            "supplier_user",
            id,
            json!({ "sessions_ended": ended }),
        ),
    )
    .await?;
    let out = fetch_su(&mut tx, id).await?;
    tx.commit().await?;
    sec::record(
        &st.pool,
        Some(p.tenant_id),
        Kind::SessionsRevoked,
        Some(p.user_id),
        Some(&p.ip),
        json!({ "supplier_user": id, "sessions": ended, "reason": "portal_account_revoked" }),
    )
    .await;
    Ok(Json(out))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct InternalUploadOut {
    pub id: Uuid,
    pub artifact_id: Option<Uuid>,
    pub original_filename: String,
    pub size_bytes: i64,
    pub status: String,
    pub uploaded_by: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// GET /evidence-requests/{id}/portal-uploads — what the supplier sent against a request (for tenant reviewers).
pub async fn request_uploads(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<InternalUploadOut>>> {
    p.require(&st, Permission::DecisionRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, InternalUploadOut>(
        "SELECT pu.id, pu.artifact_id, pu.original_filename, pu.size_bytes, pu.status, u.email AS uploaded_by, pu.created_at \
         FROM portal_uploads pu JOIN supplier_users u ON u.tenant_id = pu.tenant_id AND u.id = pu.supplier_user_id \
         WHERE pu.request_id = $1 ORDER BY pu.created_at DESC LIMIT 500",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

// ---------------------------------------------------------------- portal principal

/// An authenticated supplier user. Holds no role and no tenant permission: only portal handlers accept it.
pub struct PortalPrincipal {
    pub tenant_id: Uuid,
    pub supplier_id: Uuid,
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub ip: String,
}

impl PortalPrincipal {
    async fn deny(&self, st: &AppState, reason: &str, path: &str) {
        deny(st, self.tenant_id, Some(self.user_id), &self.ip, reason, path).await;
    }
}

impl FromRequestParts<AppState> for PortalPrincipal {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        let ClientIp(ip) = ClientIp::from_request_parts(parts, st).await?;
        let token = auth::bearer(&parts.headers).ok_or(ApiError::Unauthorized("missing_token"))?;
        if let Some((tenant, _)) = crypto::parse_token(TokenKind::Access, token) {
            deny(st, tenant, None, &ip, "tenant_token_on_portal", parts.uri.path()).await;
            return Err(ApiError::Forbidden("tenant_token_on_portal"));
        }
        let Some((tenant, hash)) = crypto::parse_token(TokenKind::Portal, token) else {
            return Err(ApiError::Unauthorized("invalid_token"));
        };
        let now = st.clock.now();
        let mut tx = db::tenant_tx(&st.pool, tenant).await?;
        type Row = (Uuid, Uuid, Uuid, OffsetDateTime);
        let row: Option<Row> = sqlx::query_as(
            "SELECT s.id, u.id, u.supplier_id, s.last_activity_at FROM supplier_sessions s \
             JOIN supplier_users u ON u.tenant_id = s.tenant_id AND u.id = s.supplier_user_id \
             JOIN tenants t ON t.id = s.tenant_id \
             WHERE s.access_hash = $1 AND s.revoked_at IS NULL AND s.expires_at > $2 AND u.status = 'active' \
               AND t.status = 'active'",
        )
        .bind(hash.to_vec())
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((session_id, user_id, supplier_id, last)) = row else {
            tx.commit().await?;
            deny(st, tenant, None, &ip, "invalid_portal_session", parts.uri.path()).await;
            return Err(ApiError::Unauthorized("invalid_token"));
        };
        // Every portal request (reads write audit rows) is rate limited per supplier user (security review L3).
        if !st
            .limiter
            .check(&format!("portal:user:{user_id}"), PORTAL_REQUESTS_PER_MIN)
        {
            tx.commit().await?;
            return Err(ApiError::TooManyRequests);
        }
        if now - last > Duration::seconds(IDLE_TTL_SECS) {
            sqlx::query("UPDATE supplier_sessions SET revoked_at = $2, revoke_reason = 'idle_timeout' WHERE id = $1")
                .bind(session_id)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Err(ApiError::Unauthorized("session_idle"));
        }
        if now - last > Duration::seconds(60) {
            sqlx::query("UPDATE supplier_sessions SET last_activity_at = $2 WHERE id = $1")
                .bind(session_id)
                .bind(now)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(PortalPrincipal {
            tenant_id: tenant,
            supplier_id,
            user_id,
            session_id,
            ip,
        })
    }
}

#[derive(Serialize)]
pub struct PortalToken {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: i64,
}

async fn start_session(st: &AppState, tx: &mut Tx, tenant: Uuid, user: Uuid, ip: &str) -> ApiResult<PortalToken> {
    let now = st.clock.now();
    let t = crypto::issue_token(TokenKind::Portal, tenant);
    let sid = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO supplier_sessions (id, tenant_id, supplier_user_id, access_hash, created_at, expires_at, \
         last_activity_at, client_ip) VALUES ($1,$2,$3,$4,$5,$6,$5,$7)",
    )
    .bind(sid)
    .bind(tenant)
    .bind(user)
    .bind(t.hash.to_vec())
    .bind(now)
    .bind(now + Duration::seconds(SESSION_TTL_SECS))
    .bind(ip)
    .execute(&mut **tx)
    .await?;
    audit::append(
        tx,
        tenant,
        audit_rec(
            user,
            "portal.login",
            "supplier_session",
            sid,
            json!({ "ip": ip, "mfa": true }),
        ),
    )
    .await?;
    Ok(PortalToken {
        access_token: t.value,
        token_type: "Bearer",
        expires_in: SESSION_TTL_SECS,
    })
}

fn ip_limited(st: &AppState, ip: &str) -> bool {
    !st.limiter
        .check(&format!("portal-auth:ip:{ip}"), st.cfg.rate.login_per_ip_per_min)
}

async fn rate_limited(st: &AppState, ip: &str, endpoint: &str) -> ApiError {
    sec::record(
        &st.pool,
        None,
        Kind::RateLimited,
        None,
        Some(ip),
        json!({ "endpoint": endpoint, "scope": "ip" }),
    )
    .await;
    ApiError::TooManyRequests
}

// ---------------------------------------------------------------- invitation acceptance (first sign-in, MFA)

#[derive(Deserialize)]
pub struct InviteStartReq {
    pub token: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct InviteStartResp {
    pub email: String,
    pub display_name: String,
    pub supplier: String,
    pub secret_base32: String,
    pub otpauth_uri: String,
}

type InviteRow = (Uuid, Uuid, String, String, String, OffsetDateTime, Option<Vec<u8>>);

/// The open, unexpired invitation behind `token` (locked for update), or a recorded denial.
async fn open_invite(st: &AppState, tx: &mut Tx, tenant: Uuid, hash: [u8; 32], ip: &str) -> ApiResult<InviteRow> {
    let row: Option<InviteRow> = sqlx::query_as(
        "SELECT u.id, u.supplier_id, u.email, u.display_name, s.name, u.invite_expires_at, u.totp_pending_enc \
         FROM supplier_users u JOIN suppliers s ON s.tenant_id = u.tenant_id AND s.id = u.supplier_id \
         WHERE u.invite_hash = $1 AND u.status = 'invited' FOR UPDATE OF u",
    )
    .bind(hash.to_vec())
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(r) if r.5 > st.clock.now() => Ok(r),
        r => {
            let reason = if r.is_some() {
                "invite_expired"
            } else {
                "invite_invalid"
            };
            deny(st, tenant, r.map(|x| x.0), ip, reason, "/portal/invite").await;
            Err(ApiError::Unauthorized("invalid_invite"))
        }
    }
}

/// POST /portal/invite/start — sets the password and returns a fresh authenticator secret to enrol.
pub async fn invite_start(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<InviteStartReq>,
) -> ApiResult<Json<InviteStartResp>> {
    if ip_limited(&st, &ip) {
        return Err(rate_limited(&st, &ip, "portal_invite").await);
    }
    let Some((tenant, hash)) = crypto::parse_token(TokenKind::PortalInvite, req.token.trim()) else {
        return Err(ApiError::Unauthorized("invalid_invite"));
    };
    if req.password.len() > 1024 {
        return Err(ApiError::BadRequest("field_too_long"));
    }
    let mut tx = db::tenant_tx(&st.pool, tenant).await?;
    let (uid, _sid, email, display_name, supplier, _, _) = open_invite(&st, &mut tx, tenant, hash, &ip).await?;
    crypto::check_password_policy(&req.password, &email).map_err(|e| ApiError::BadRequest(e.code()))?;
    let pw = req.password;
    let ph = tokio::task::spawn_blocking(move || crypto::hash_password(&pw))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    let secret = crypto::random_bytes::<20>();
    let sealed = st
        .cfg
        .data_keys
        .seal(&secret, &crypto::portal_totp_aad(tenant, uid, false))
        .map_err(ApiError::internal)?;
    sqlx::query("UPDATE supplier_users SET password_hash = $2, totp_pending_enc = $3 WHERE id = $1")
        .bind(uid)
        .bind(ph)
        .bind(sealed)
        .execute(&mut *tx)
        .await?;
    audit::append(
        &mut tx,
        tenant,
        audit_rec(
            uid,
            "portal_user.invite_opened",
            "supplier_user",
            uid,
            json!({ "ip": ip }),
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(InviteStartResp {
        otpauth_uri: crypto::otpauth_uri("KIZUNA Supplier", &email, &secret),
        secret_base32: crypto::base32(&secret),
        email,
        display_name,
        supplier,
    }))
}

#[derive(Deserialize)]
pub struct InviteFinishReq {
    pub token: String,
    pub code: String,
}

/// POST /portal/invite/finish — confirms the authenticator, consumes the invitation and signs in.
pub async fn invite_finish(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<InviteFinishReq>,
) -> ApiResult<Json<PortalToken>> {
    if ip_limited(&st, &ip) {
        return Err(rate_limited(&st, &ip, "portal_invite").await);
    }
    let Some((tenant, hash)) = crypto::parse_token(TokenKind::PortalInvite, req.token.trim()) else {
        return Err(ApiError::Unauthorized("invalid_invite"));
    };
    let mut tx = db::tenant_tx(&st.pool, tenant).await?;
    let (uid, ..) = open_invite(&st, &mut tx, tenant, hash, &ip).await?;
    let pending: Option<Vec<u8>> = sqlx::query_scalar("SELECT totp_pending_enc FROM supplier_users WHERE id = $1")
        .bind(uid)
        .fetch_one(&mut *tx)
        .await?;
    let secret = pending
        .and_then(|p| st.cfg.data_keys.open(&p, &crypto::portal_totp_aad(tenant, uid, false)))
        .ok_or(ApiError::Conflict("invite_not_started"))?;
    let Some(step) = crypto::totp_verify(&secret, req.code.trim(), st.clock.now().unix_timestamp(), None) else {
        tx.commit().await?;
        sec::record(
            &st.pool,
            Some(tenant),
            Kind::LoginFailed,
            None,
            Some(&ip),
            json!({ "reason": "bad_totp", "supplier_user": uid, "flow": "portal_invite" }),
        )
        .await;
        return Err(ApiError::Unauthorized("invalid_code"));
    };
    let sealed = st
        .cfg
        .data_keys
        .seal(&secret, &crypto::portal_totp_aad(tenant, uid, true))
        .map_err(ApiError::internal)?;
    sqlx::query(
        "UPDATE supplier_users SET status = 'active', invite_hash = NULL, totp_pending_enc = NULL, totp_secret_enc = $2, \
         totp_last_step = $3, activated_at = $4 WHERE id = $1",
    )
    .bind(uid)
    .bind(sealed)
    .bind(step)
    .bind(st.clock.now())
    .execute(&mut *tx)
    .await?;
    audit::append(
        &mut tx,
        tenant,
        audit_rec(
            uid,
            "portal_user.activated",
            "supplier_user",
            uid,
            json!({ "mfa": "totp" }),
        ),
    )
    .await?;
    let t = start_session(&st, &mut tx, tenant, uid, &ip).await?;
    tx.commit().await?;
    sec::record(
        &st.pool,
        Some(tenant),
        Kind::MfaEnrolled,
        None,
        Some(&ip),
        json!({ "supplier_user": uid, "method": "totp", "flow": "portal_invite" }),
    )
    .await;
    Ok(Json(t))
}

// ---------------------------------------------------------------- sign-in / out

#[derive(Deserialize)]
pub struct LoginReq {
    pub tenant: String,
    pub email: String,
    pub password: String,
    pub totp_code: String,
}

/// POST /portal/login — password AND authenticator code, always (no session without MFA).
pub async fn login(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<LoginReq>,
) -> ApiResult<Json<PortalToken>> {
    let fail = || ApiError::Unauthorized("invalid_credentials");
    if ip_limited(&st, &ip) {
        return Err(rate_limited(&st, &ip, "portal_login").await);
    }
    let email = auth::normalize_email(&req.email);
    if req.tenant.len() > 64 || email.len() > 254 || req.password.len() > 1024 || req.totp_code.len() > 16 {
        return Err(ApiError::BadRequest("field_too_long"));
    }
    if !st.limiter.check(
        &format!("portal-login:acct:{}:{email}", req.tenant),
        st.cfg.rate.login_per_account_per_min,
    ) {
        return Err(rate_limited(&st, &ip, "portal_login").await);
    }
    let tenant: Option<Uuid> = sqlx::query_scalar("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
        .bind(&req.tenant)
        .fetch_optional(&st.pool)
        .await?;
    type Row = (
        Uuid,
        Option<String>,
        Option<Vec<u8>>,
        Option<i64>,
        Option<OffsetDateTime>,
    );
    let row: Option<(Uuid, Row)> = match tenant {
        Some(t) => {
            let mut tx = db::tenant_tx(&st.pool, t).await?;
            let r: Option<Row> = sqlx::query_as(
                "SELECT id, password_hash, totp_secret_enc, totp_last_step, locked_until FROM supplier_users \
                 WHERE email = $1 AND status = 'active'",
            )
            .bind(&email)
            .fetch_optional(&mut *tx)
            .await?;
            tx.commit().await?;
            r.map(|r| (t, r))
        }
        None => None,
    };
    let ok = auth::verify_pw_blocking(&st, req.password, row.as_ref().and_then(|r| r.1.1.clone())).await?;
    let Some((tenant, (uid, _, totp_enc, last_step, locked))) = row else {
        sec::record(
            &st.pool,
            tenant,
            Kind::LoginFailed,
            None,
            Some(&ip),
            json!({ "reason": "unknown_account", "flow": "portal" }),
        )
        .await;
        return Err(fail());
    };
    let now = st.clock.now();
    let failed = |reason: &'static str| {
        let st = st.clone();
        let ip = ip.clone();
        async move {
            let mut tx = db::tenant_tx(&st.pool, tenant).await?;
            let n: i32 = sqlx::query_scalar(
                "UPDATE supplier_users SET failed_logins = failed_logins + 1 WHERE id = $1 RETURNING failed_logins",
            )
            .bind(uid)
            .fetch_one(&mut *tx)
            .await?;
            let lock = reason != "locked" && n >= st.cfg.lockout_threshold;
            if lock {
                sqlx::query(
                    "UPDATE supplier_users SET failed_logins = 0, locked_until = $2 + make_interval(secs => $3) WHERE id = $1",
                )
                .bind(uid)
                .bind(st.clock.now())
                .bind(st.cfg.lockout_secs as f64)
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
            sec::record(
                &st.pool,
                Some(tenant),
                Kind::LoginFailed,
                None,
                Some(&ip),
                json!({ "reason": reason, "supplier_user": uid, "flow": "portal" }),
            )
            .await;
            if lock {
                sec::record(
                    &st.pool,
                    Some(tenant),
                    Kind::AccountLocked,
                    None,
                    Some(&ip),
                    json!({ "supplier_user": uid, "flow": "portal" }),
                )
                .await;
            }
            Ok::<ApiError, ApiError>(ApiError::Unauthorized("invalid_credentials"))
        }
    };
    if locked.is_some_and(|l| l > now) {
        return Err(failed("locked").await?);
    }
    if !ok {
        return Err(failed("bad_password").await?);
    }
    let secret = totp_enc
        .and_then(|e| st.cfg.data_keys.open(&e, &crypto::portal_totp_aad(tenant, uid, true)))
        .ok_or(ApiError::Internal("portal totp seed unreadable".into()))?;
    let Some(step) = crypto::totp_verify(&secret, req.totp_code.trim(), now.unix_timestamp(), last_step) else {
        return Err(failed("bad_totp").await?);
    };
    let mut tx = db::tenant_tx(&st.pool, tenant).await?;
    // Replay guard: the step only moves forward, under the row lock.
    let moved = sqlx::query(
        "UPDATE supplier_users SET failed_logins = 0, locked_until = NULL, totp_last_step = $2 \
         WHERE id = $1 AND status = 'active' AND (totp_last_step IS NULL OR totp_last_step < $2)",
    )
    .bind(uid)
    .bind(step)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if moved == 0 {
        return Err(fail());
    }
    let t = start_session(&st, &mut tx, tenant, uid, &ip).await?;
    tx.commit().await?;
    Ok(Json(t))
}

/// POST /portal/logout
pub async fn logout(State(st): State<AppState>, pp: PortalPrincipal) -> ApiResult<Json<Value>> {
    let mut tx = db::tenant_tx(&st.pool, pp.tenant_id).await?;
    sqlx::query(
        "UPDATE supplier_sessions SET revoked_at = $2, revoke_reason = 'logout' WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(pp.session_id)
    .bind(st.clock.now())
    .execute(&mut *tx)
    .await?;
    audit::append(
        &mut tx,
        pp.tenant_id,
        audit_rec(
            pp.user_id,
            "portal.logout",
            "supplier_session",
            pp.session_id,
            json!({}),
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Serialize)]
pub struct PortalMe {
    pub email: String,
    pub display_name: String,
    pub supplier: String,
}

/// GET /portal/me
pub async fn me(State(st): State<AppState>, pp: PortalPrincipal) -> ApiResult<Json<PortalMe>> {
    let mut tx = db::portal_tx(&st.pool, pp.tenant_id, pp.supplier_id).await?;
    let supplier: String = sqlx::query_scalar("SELECT name FROM suppliers")
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut tx = db::tenant_tx(&st.pool, pp.tenant_id).await?;
    let (email, display_name): (String, String) =
        sqlx::query_as("SELECT email, display_name FROM supplier_users WHERE id = $1")
            .bind(pp.user_id)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(Json(PortalMe {
        email,
        display_name,
        supplier,
    }))
}

// ---------------------------------------------------------------- requests and uploads (supplier scope)

#[derive(Serialize)]
pub struct PortalItem {
    pub part: String,
    pub what_ja: String,
    pub what_en: String,
    pub purchase_order: Option<String>,
    pub po_line: Option<String>,
    pub lot: Option<String>,
    pub heat: Option<String>,
}

#[derive(Serialize)]
pub struct PortalDoc {
    /// The supplier's own earlier document that is to be resent (as named in the letter).
    pub name: String,
    pub items: Vec<PortalItem>,
}

#[derive(Serialize)]
pub struct PortalRequest {
    pub id: Uuid,
    /// Reference shown as the request title (e.g. RR-0193A7…).
    pub reference: String,
    #[serde(with = "iso_date::option")]
    pub due_on: Option<time::Date>,
    pub documents: Vec<PortalDoc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_ja: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_en: Option<String>,
}

/// Projects the stored request documents onto what a supplier may see: no internal ids, hashes, decisions or reasons.
fn project_docs(v: &Value) -> Vec<PortalDoc> {
    let docs: Vec<RequestDoc> = serde_json::from_value(v.clone()).unwrap_or_default();
    docs.into_iter()
        .map(|d| PortalDoc {
            name: d.name,
            items: d
                .items
                .into_iter()
                .map(|i| PortalItem {
                    what_ja: resupply::label(&i.predicate, Lang::Ja),
                    what_en: resupply::label(&i.predicate, Lang::En),
                    part: i.part,
                    purchase_order: i.purchase_order,
                    po_line: i.po_line,
                    lot: i.lot,
                    heat: i.heat,
                })
                .collect(),
        })
        .collect()
}

type ReqRow = (Uuid, String, Option<time::Date>, Value, String, String);
/// Requests per minute per supplier user across all portal routes.
pub const PORTAL_REQUESTS_PER_MIN: u32 = 120;

const PORTAL_REQ_COLS: &str = "id, reference, due_on, documents, body_ja, body_en";

fn to_portal(r: ReqRow, with_body: bool) -> PortalRequest {
    PortalRequest {
        id: r.0,
        reference: r.1,
        due_on: r.2,
        documents: project_docs(&r.3),
        body_ja: with_body.then_some(r.4),
        body_en: with_body.then_some(r.5),
    }
}

async fn portal_audit(
    st: &AppState,
    pp: &PortalPrincipal,
    action: &str,
    target_type: &str,
    target: Uuid,
    details: Value,
) -> ApiResult<()> {
    let mut tx = db::tenant_tx(&st.pool, pp.tenant_id).await?;
    audit::append(
        &mut tx,
        pp.tenant_id,
        audit_rec(pp.user_id, action, target_type, target, details),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// GET /portal/requests — open (approved) requests addressed to the caller's supplier.
pub async fn list_requests(State(st): State<AppState>, pp: PortalPrincipal) -> ApiResult<Json<Vec<PortalRequest>>> {
    let mut tx = db::portal_tx(&st.pool, pp.tenant_id, pp.supplier_id).await?;
    // RLS is the boundary; the predicates repeat it so a policy regression cannot widen this route on its own.
    let rows: Vec<ReqRow> = sqlx::query_as(&format!(
        "SELECT {PORTAL_REQ_COLS} FROM evidence_requests WHERE supplier_id = $1 AND status = 'approved' \
         ORDER BY due_on NULLS LAST, reviewed_at DESC LIMIT 200"
    ))
    .bind(pp.supplier_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    portal_audit(
        &st,
        &pp,
        "portal.requests_listed",
        "supplier",
        pp.supplier_id,
        json!({ "count": rows.len() }),
    )
    .await?;
    Ok(Json(rows.into_iter().map(|r| to_portal(r, false)).collect()))
}

/// The request if it is in the caller's scope; otherwise a recorded denial and 404 (no existence oracle).
async fn scoped_request(st: &AppState, pp: &PortalPrincipal, id: Uuid, path: &str) -> ApiResult<ReqRow> {
    let mut tx = db::portal_tx(&st.pool, pp.tenant_id, pp.supplier_id).await?;
    let row: Option<ReqRow> = sqlx::query_as(&format!(
        "SELECT {PORTAL_REQ_COLS} FROM evidence_requests WHERE id = $1 AND supplier_id = $2 AND status = 'approved'"
    ))
    .bind(id)
    .bind(pp.supplier_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    match row {
        Some(r) => Ok(r),
        None => {
            pp.deny(st, "request_out_of_scope", path).await;
            Err(ApiError::NotFound)
        }
    }
}

/// GET /portal/requests/{id} — one request with the approved letter text.
pub async fn get_request(
    State(st): State<AppState>,
    pp: PortalPrincipal,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PortalRequest>> {
    let r = scoped_request(&st, &pp, id, "/portal/requests/{id}").await?;
    portal_audit(&st, &pp, "portal.request_viewed", "evidence_request", id, json!({})).await?;
    Ok(Json(to_portal(r, true)))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct PortalUploadOut {
    pub id: Uuid,
    pub original_filename: String,
    pub size_bytes: i64,
    /// received → scanning → quarantined | received_for_review | not_accepted; follows the document's real state.
    pub status: String,
    /// Neutral reason when `not_accepted`: file_type_not_accepted | not_accepted_after_review | upload_failed.
    pub reason_code: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Sealed delivery receipt (innovation I7), downloadable at `/portal/uploads/{id}/receipt`; none when the file was
    /// refused before it was stored.
    pub receipt_id: Option<Uuid>,
}

const UP_COLS: &str = "id, original_filename, size_bytes, status, reason_code, created_at, \
    (SELECT r.id FROM delivery_receipts r WHERE r.portal_upload_id = portal_uploads.id) AS receipt_id";

/// GET /portal/requests/{id}/uploads — the caller's supplier's uploads against the request, with status.
pub async fn list_uploads(
    State(st): State<AppState>,
    pp: PortalPrincipal,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<PortalUploadOut>>> {
    scoped_request(&st, &pp, id, "/portal/requests/{id}/uploads").await?;
    let mut tx = db::portal_tx(&st.pool, pp.tenant_id, pp.supplier_id).await?;
    let rows = sqlx::query_as::<_, PortalUploadOut>(&format!(
        "SELECT {UP_COLS} FROM portal_uploads WHERE request_id = $1 AND supplier_id = $2 ORDER BY created_at DESC LIMIT 200"
    ))
    .bind(id)
    .bind(pp.supplier_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

/// POST /portal/requests/{id}/uploads — multipart `file`; goes through the normal ingestion pipeline.
pub async fn upload(
    State(st): State<AppState>,
    pp: PortalPrincipal,
    Path(id): Path<Uuid>,
    mp: Multipart,
) -> ApiResult<Json<PortalUploadOut>> {
    const PATH: &str = "/portal/requests/{id}/uploads";
    if !st.limiter.check(
        &format!("portal-upload:supplier:{}", pp.supplier_id),
        st.cfg.rate.uploads_per_user_per_min,
    ) {
        sec::record(
            &st.pool,
            Some(pp.tenant_id),
            Kind::RateLimited,
            None,
            Some(&pp.ip),
            json!({ "endpoint": "portal_upload", "supplier_id": pp.supplier_id, "supplier_user": pp.user_id }),
        )
        .await;
        return Err(ApiError::TooManyRequests);
    }
    scoped_request(&st, &pp, id, PATH).await?;
    let (max_mb, per_day, today): (i32, i32, i64) = {
        let mut tx = db::portal_tx(&st.pool, pp.tenant_id, pp.supplier_id).await?;
        let r = sqlx::query_as(
            "SELECT s.max_upload_mb, s.max_uploads_per_day, \
               (SELECT count(*) FROM portal_uploads p WHERE p.supplier_id = s.id AND p.created_at > now() - interval '1 day') \
             FROM suppliers s WHERE s.id = $1",
        )
        .bind(pp.supplier_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        r
    };
    if today >= i64::from(per_day) {
        pp.deny(&st, "daily_upload_limit", PATH).await;
        return Err(ApiError::TooManyRequests);
    }
    let tenant_mb = {
        let mut tx = db::tenant_tx(&st.pool, pp.tenant_id).await?;
        let mb = artifacts::tenant_upload_mb(&mut tx).await?;
        tx.commit().await?;
        mb
    };
    let limit = artifacts::upload_limit(
        artifacts::upload_limit(st.cfg.max_upload_bytes, tenant_mb),
        Some(i64::from(max_mb)),
    );
    // The supplier answers with inspection documents; it does not choose how the tenant classifies them.
    let (data, name, declared, _kind) = artifacts::read_upload(mp, limit).await?;
    let upload_id = Uuid::now_v7();
    {
        let mut tx = db::tenant_tx(&st.pool, pp.tenant_id).await?;
        // The daily cap is re-checked and the row inserted under a per-supplier lock: parallel uploads cannot overshoot
        // it (security review L2; the early check above only avoids reading bodies that will be refused).
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 43))")
            .bind(pp.supplier_id.to_string())
            .execute(&mut *tx)
            .await?;
        let today: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM portal_uploads WHERE supplier_id = $1 AND created_at > now() - interval '1 day'",
        )
        .bind(pp.supplier_id)
        .fetch_one(&mut *tx)
        .await?;
        if today >= i64::from(per_day) {
            tx.commit().await?;
            pp.deny(&st, "daily_upload_limit", PATH).await;
            return Err(ApiError::TooManyRequests);
        }
        sqlx::query(
            "INSERT INTO portal_uploads (id, tenant_id, supplier_id, supplier_user_id, request_id, original_filename, size_bytes) \
             VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(upload_id)
        .bind(pp.tenant_id)
        .bind(pp.supplier_id)
        .bind(pp.user_id)
        .bind(id)
        .bind(&name)
        .bind(data.len() as i64)
        .execute(&mut *tx)
        .await?;
        audit::append(
            &mut tx,
            pp.tenant_id,
            audit_rec(
                pp.user_id,
                "portal.upload_received",
                "portal_upload",
                upload_id,
                json!({ "request_id": id, "size": data.len(), "filename": name }),
            ),
        )
        .await?;
        artifacts::set_portal_status(&mut tx, upload_id, "scanning", None, None).await?;
        tx.commit().await?;
    }
    let up = Uploader {
        tenant: pp.tenant_id,
        actor: pp.user_id,
        session: Session::Supplier(pp.session_id),
        ip: &pp.ip,
    };
    if let Err(e) = artifacts::ingest(&st, &up, data, &name, declared, "inspection_report", Some(upload_id)).await {
        // Whatever stopped it, the supplier sees the upload as not accepted (never stuck in "scanning").
        let mut tx = db::tenant_tx(&st.pool, pp.tenant_id).await?;
        sqlx::query("UPDATE portal_uploads SET status = 'not_accepted', reason_code = 'upload_failed', updated_at = now() WHERE id = $1 AND status IN ('received', 'scanning')")
            .bind(upload_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Err(e);
    }
    let mut tx = db::portal_tx(&st.pool, pp.tenant_id, pp.supplier_id).await?;
    let out = sqlx::query_as::<_, PortalUploadOut>(&format!("SELECT {UP_COLS} FROM portal_uploads WHERE id = $1"))
        .bind(upload_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(out))
}

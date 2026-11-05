//! Authentication: login (password + TOTP), token refresh with rotation and reuse detection, logout,
//! MFA enrolment, password change, and the `Principal` extractor used by every protected handler.

use std::net::{IpAddr, SocketAddr};

use axum::Json;
use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use policy_engine::{Decision, Permission, Resource, Role, Subject};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::crypto::{self, TokenKind};
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiResult};
use crate::security_events::{self as sec, Kind};

/// Client IP. With `trusted_proxy_hops = N > 0`, each trusted proxy appends the address it saw, so the client is
/// the N-th entry from the right of `X-Forwarded-For`; entries further left are client-controlled and ignored.
#[derive(Debug, Clone)]
pub struct ClientIp(pub String);

pub fn client_ip_from(xff: Option<&str>, socket: Option<IpAddr>, hops: usize) -> String {
    if hops > 0
        && let Some(xff) = xff
    {
        let parts: Vec<&str> = xff.split(',').map(str::trim).collect();
        if parts.len() >= hops
            && let Ok(ip) = parts[parts.len() - hops].parse::<IpAddr>()
        {
            return ip.to_string();
        }
    }
    socket.map(|i| i.to_string()).unwrap_or_else(|| "unknown".into())
}

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        let xff = parts.headers.get("x-forwarded-for").and_then(|v| v.to_str().ok());
        let socket = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());
        Ok(ClientIp(client_ip_from(xff, socket, st.cfg.trusted_proxy_hops)))
    }
}

/// Authenticated caller, resolved from a live, unrevoked session.
#[derive(Debug, Clone)]
pub struct Principal {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub role: Role,
    pub mfa_verified: bool,
    pub ip: String,
}

impl Principal {
    pub fn subject(&self) -> Subject {
        Subject {
            tenant_id: self.tenant_id,
            user_id: self.user_id,
            role: self.role,
            mfa_verified: self.mfa_verified,
        }
    }

    /// Deny-by-default check against a resource in the caller's tenant.
    pub async fn require(&self, st: &AppState, perm: Permission) -> ApiResult<()> {
        self.require_on(
            st,
            perm,
            &Resource {
                tenant_id: self.tenant_id,
                kind: "tenant",
                owner_user_id: None,
            },
        )
        .await
    }

    pub async fn require_on(&self, st: &AppState, perm: Permission, res: &Resource<'_>) -> ApiResult<()> {
        match st.policy.authorize(&self.subject(), perm, res) {
            Decision::Allow => Ok(()),
            Decision::Deny(reason) => {
                sec::record(
                    &st.pool,
                    Some(self.tenant_id),
                    Kind::AuthzDenied,
                    Some(self.user_id),
                    Some(&self.ip),
                    json!({ "permission": format!("{perm:?}"), "reason": reason.code(), "resource": res.kind }),
                )
                .await;
                Err(ApiError::Forbidden(reason.code()))
            }
        }
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

impl FromRequestParts<AppState> for Principal {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        let ClientIp(ip) = ClientIp::from_request_parts(parts, st).await?;
        let token = bearer(&parts.headers).ok_or(ApiError::Unauthorized("missing_token"))?;
        let Some((tenant, hash)) = crypto::parse_token(TokenKind::Access, token) else {
            return Err(ApiError::Unauthorized("invalid_token"));
        };
        let mut tx = db::tenant_tx(&st.pool, tenant).await?;
        let row: Option<(Uuid, Uuid, bool, String)> = sqlx::query_as(
            "SELECT s.id, s.user_id, s.mfa_verified, u.role FROM sessions s \
             JOIN users u ON u.tenant_id = s.tenant_id AND u.id = s.user_id \
             JOIN tenants t ON t.id = s.tenant_id \
             WHERE s.access_hash = $1 AND s.revoked_at IS NULL AND s.access_expires_at > $2 \
               AND s.expires_at > $2 AND u.status = 'active' AND t.status = 'active'",
        )
        .bind(hash.to_vec())
        .bind(st.clock.now())
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        let Some((session_id, user_id, mfa_verified, role)) = row else {
            return Err(ApiError::Unauthorized("invalid_token"));
        };
        let role = Role::parse(&role).map_err(|_| ApiError::Forbidden("unknown_role"))?;
        Ok(Principal {
            tenant_id: tenant,
            user_id,
            session_id,
            role,
            mfa_verified,
            ip,
        })
    }
}

#[derive(Deserialize)]
pub struct LoginReq {
    pub tenant: String,
    pub email: String,
    pub password: String,
    pub totp_code: Option<String>,
}

#[derive(Serialize)]
pub struct TokenResp {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: &'static str,
    pub expires_in: i64,
    pub mfa_verified: bool,
}

/// Runs argon2 off the async runtime, bounded by `hash_permits`; `None` hash burns a dummy verification.
async fn verify_pw_blocking(st: &AppState, pw: String, hash: Option<String>) -> ApiResult<bool> {
    let permit = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        st.hash_permits.clone().acquire_owned(),
    )
    .await
    .map_err(|_| ApiError::TooManyRequests)?
    .map_err(ApiError::internal)?;
    let ok = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        match hash {
            Some(h) => crypto::verify_password(&pw, &h),
            None => {
                crypto::dummy_verify(&pw);
                false
            }
        }
    })
    .await
    .map_err(ApiError::internal)?;
    Ok(ok)
}

/// Counts a failed credential check atomically; locks the account at the threshold. Returns true if it locked now.
pub async fn register_failure(st: &AppState, tenant: Uuid, uid: Uuid) -> ApiResult<bool> {
    let mut tx = db::tenant_tx(&st.pool, tenant).await?;
    let n: i32 =
        sqlx::query_scalar("UPDATE users SET failed_logins = failed_logins + 1 WHERE id = $1 RETURNING failed_logins")
            .bind(uid)
            .fetch_one(&mut *tx)
            .await?;
    let locked = n >= st.cfg.lockout_threshold;
    if locked {
        sqlx::query("UPDATE users SET failed_logins = 0, locked_until = $3 + make_interval(secs => $2) WHERE id = $1")
            .bind(uid)
            .bind(st.cfg.lockout_secs as f64)
            .bind(st.clock.now())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(locked)
}

pub fn normalize_email(e: &str) -> String {
    e.trim().to_lowercase()
}

/// Creates a session + first refresh token in `tx`.
async fn start_session(
    st: &AppState,
    tx: &mut Tx,
    tenant: Uuid,
    user: Uuid,
    mfa: bool,
    ip: &str,
) -> ApiResult<TokenResp> {
    let now = st.clock.now();
    let access = crypto::issue_token(TokenKind::Access, tenant);
    let refresh = crypto::issue_token(TokenKind::Refresh, tenant);
    let sid = Uuid::now_v7();
    let exp = now + Duration::seconds(st.cfg.session_ttl_secs);
    sqlx::query(
        "INSERT INTO sessions (id, tenant_id, user_id, access_hash, access_expires_at, expires_at, mfa_verified, client_ip) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(sid)
    .bind(tenant)
    .bind(user)
    .bind(access.hash.to_vec())
    .bind(now + Duration::seconds(st.cfg.access_ttl_secs))
    .bind(exp)
    .bind(mfa)
    .bind(ip)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO refresh_tokens (id, tenant_id, session_id, token_hash, expires_at) VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant)
    .bind(sid)
    .bind(refresh.hash.to_vec())
    .bind(exp)
    .execute(&mut **tx)
    .await?;
    audit::append(
        tx,
        tenant,
        Record::user(user, "auth.login", "session", sid, json!({ "mfa": mfa, "ip": ip })),
    )
    .await?;
    Ok(TokenResp {
        access_token: access.value,
        refresh_token: refresh.value,
        token_type: "Bearer",
        expires_in: st.cfg.access_ttl_secs,
        mfa_verified: mfa,
    })
}

/// A user whose password (and account state) checked out; the second factor is still pending.
pub(crate) struct FirstFactor {
    pub tenant_id: Uuid,
    pub uid: Uuid,
    totp_enabled: bool,
    totp_enc: Option<Vec<u8>>,
    last_step: Option<i64>,
    pub webauthn_enabled: bool,
}

type LoginUser = (
    Uuid,
    String,
    String,
    Option<OffsetDateTime>,
    bool,
    Option<Vec<u8>>,
    Option<i64>,
    bool,
);

/// Records a failed login (counting toward lockout for credential failures) and returns the generic error.
pub(crate) async fn login_failed(st: &AppState, tenant: Uuid, uid: Uuid, ip: &str, reason: &str) -> ApiError {
    let counted = matches!(
        reason,
        "bad_password" | "bad_totp" | "bad_webauthn" | "bad_recovery_code"
    );
    let locked = match counted {
        true => register_failure(st, tenant, uid).await,
        false => Ok(false),
    };
    let locked = match locked {
        Ok(l) => l,
        Err(e) => return e,
    };
    sec::record(
        &st.pool,
        Some(tenant),
        Kind::LoginFailed,
        Some(uid),
        Some(ip),
        json!({ "reason": reason }),
    )
    .await;
    if locked {
        sec::record(
            &st.pool,
            Some(tenant),
            Kind::AccountLocked,
            Some(uid),
            Some(ip),
            json!({ "minutes": st.cfg.lockout_secs / 60 }),
        )
        .await;
    }
    ApiError::Unauthorized("invalid_credentials")
}

/// Rate limits, tenant/user lookup, password and account-state checks shared by every login flow.
pub(crate) async fn first_factor(
    st: &AppState,
    ip: &str,
    tenant_slug: &str,
    email: &str,
    password: String,
) -> ApiResult<FirstFactor> {
    let fail = || ApiError::Unauthorized("invalid_credentials");
    if !st
        .limiter
        .check(&format!("login:ip:{ip}"), st.cfg.rate.login_per_ip_per_min)
    {
        sec::record(
            &st.pool,
            None,
            Kind::RateLimited,
            None,
            Some(ip),
            json!({ "endpoint": "login", "scope": "ip" }),
        )
        .await;
        return Err(ApiError::TooManyRequests);
    }
    let email = normalize_email(email);
    if tenant_slug.len() > 64 || email.len() > 254 || password.len() > 1024 {
        return Err(ApiError::BadRequest("field_too_long"));
    }
    // Keyed by slug (not tenant id) and checked before the tenant lookup: identical behaviour for unknown tenants.
    if !st.limiter.check(
        &format!("login:acct:{tenant_slug}:{email}"),
        st.cfg.rate.login_per_account_per_min,
    ) {
        sec::record(
            &st.pool,
            None,
            Kind::RateLimited,
            None,
            Some(ip),
            json!({ "endpoint": "login", "scope": "account" }),
        )
        .await;
        return Err(ApiError::TooManyRequests);
    }
    let tenant: Option<Uuid> = sqlx::query_scalar("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
        .bind(tenant_slug)
        .fetch_optional(&st.pool)
        .await?;
    let Some(tenant_id) = tenant else {
        verify_pw_blocking(st, password, None).await?;
        sec::record(
            &st.pool,
            None,
            Kind::LoginFailed,
            None,
            Some(ip),
            json!({ "reason": "unknown_tenant" }),
        )
        .await;
        return Err(fail());
    };

    if crate::incidents::ip_blocked(&st.pool, tenant_id, ip).await? {
        sec::try_record(
            &st.pool,
            Some(tenant_id),
            Kind::BlockedRequest,
            None,
            Some(ip),
            json!({ "endpoint": "login" }),
        )
        .await
        .ok();
        return Err(ApiError::Forbidden("blocked"));
    }
    // Read without holding a row lock during the (slow) password hash.
    let mut tx = db::tenant_tx(&st.pool, tenant_id).await?;
    let user: Option<LoginUser> = sqlx::query_as(
        "SELECT id, password_hash, status, locked_until, totp_enabled, totp_secret_enc, totp_last_step, \
         webauthn_enabled FROM users WHERE email = $1",
    )
    .bind(&email)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    let Some((uid, phash, status, locked_until, totp_enabled, totp_enc, last_step, webauthn_enabled)) = user else {
        verify_pw_blocking(st, password, None).await?;
        sec::record(
            &st.pool,
            Some(tenant_id),
            Kind::LoginFailed,
            None,
            Some(ip),
            json!({ "reason": "unknown_user" }),
        )
        .await;
        return Err(fail());
    };
    let pw_ok = verify_pw_blocking(st, password, Some(phash)).await?;
    let reason = if status != "active" {
        Some("disabled")
    } else if locked_until.is_some_and(|t| t > st.clock.now()) {
        Some("locked")
    } else if !pw_ok {
        Some("bad_password")
    } else {
        None
    };
    if let Some(reason) = reason {
        return Err(login_failed(st, tenant_id, uid, ip, reason).await);
    }
    Ok(FirstFactor {
        tenant_id,
        uid,
        totp_enabled,
        totp_enc,
        last_step,
        webauthn_enabled,
    })
}

/// Re-checks account state under lock (it may have changed while hashing), consumes the TOTP step and starts the
/// session. `mfa` = a second factor was verified; `via_webauthn` = that factor was a security key. A user who enrolled
/// a key while the password was being hashed is refused unless the key was used (no downgrade race).
pub(crate) async fn complete_login(
    st: &AppState,
    tenant_id: Uuid,
    uid: Uuid,
    totp_step: Option<i64>,
    mfa: bool,
    via_webauthn: bool,
    ip: &str,
) -> ApiResult<TokenResp> {
    let mut tx = db::tenant_tx(&st.pool, tenant_id).await?;
    let ok: Option<Uuid> = sqlx::query_scalar(
        "UPDATE users SET failed_logins = 0, locked_until = NULL, \
           totp_last_step = CASE WHEN $2::bigint IS NULL THEN totp_last_step ELSE $2 END \
         WHERE id = $1 AND status = 'active' AND (locked_until IS NULL OR locked_until <= $3) \
           AND ($2::bigint IS NULL OR totp_last_step IS NULL OR totp_last_step < $2) \
           AND (NOT webauthn_enabled OR $4) RETURNING id",
    )
    .bind(uid)
    .bind(totp_step)
    .bind(st.clock.now())
    .bind(via_webauthn)
    .fetch_optional(&mut *tx)
    .await?;
    if ok.is_none() {
        tx.rollback().await?;
        return Err(login_failed(st, tenant_id, uid, ip, "state_changed_or_replay").await);
    }
    let resp = start_session(st, &mut tx, tenant_id, uid, mfa, ip).await?;
    tx.commit().await?;
    sec::record(
        &st.pool,
        Some(tenant_id),
        Kind::LoginSucceeded,
        Some(uid),
        Some(ip),
        json!({ "mfa": mfa }),
    )
    .await;
    Ok(resp)
}

pub async fn login(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<LoginReq>,
) -> ApiResult<Json<TokenResp>> {
    let ff = first_factor(&st, &ip, &req.tenant, &req.email, req.password).await?;
    // An enrolled security key is mandatory: password + TOTP no longer suffices (use /auth/webauthn/login/*).
    if ff.webauthn_enabled {
        return Err(ApiError::Unauthorized("webauthn_required"));
    }
    let mut totp_step = None;
    if ff.totp_enabled {
        let Some(code) = req.totp_code.as_deref() else {
            return Err(ApiError::Unauthorized("mfa_required"));
        };
        let secret = ff
            .totp_enc
            .and_then(|enc| st.cfg.data_keys.open(&enc, &crypto::totp_aad(ff.tenant_id, ff.uid)))
            .ok_or_else(|| ApiError::internal("totp secret undecryptable"))?;
        totp_step = crypto::totp_verify(&secret, code, st.clock.now().unix_timestamp(), ff.last_step);
        if totp_step.is_none() {
            return Err(login_failed(&st, ff.tenant_id, ff.uid, &ip, "bad_totp").await);
        }
    }
    let resp = complete_login(&st, ff.tenant_id, ff.uid, totp_step, ff.totp_enabled, false, &ip).await?;
    Ok(Json(resp))
}

#[derive(Deserialize)]
pub struct RefreshReq {
    pub refresh_token: String,
}

pub async fn refresh(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<RefreshReq>,
) -> ApiResult<Json<TokenResp>> {
    if !st
        .limiter
        .check(&format!("refresh:ip:{ip}"), st.cfg.rate.refresh_per_ip_per_min)
    {
        sec::record(
            &st.pool,
            None,
            Kind::RateLimited,
            None,
            Some(&ip),
            json!({ "endpoint": "refresh" }),
        )
        .await;
        return Err(ApiError::TooManyRequests);
    }
    let invalid = || ApiError::Unauthorized("invalid_token");
    let (tenant, hash) = crypto::parse_token(TokenKind::Refresh, &req.refresh_token).ok_or_else(invalid)?;
    let mut tx = db::tenant_tx(&st.pool, tenant).await?;
    let row: Option<(Uuid, Uuid, Option<OffsetDateTime>, OffsetDateTime)> = sqlx::query_as(
        "SELECT id, session_id, used_at, expires_at FROM refresh_tokens WHERE token_hash = $1 FOR UPDATE",
    )
    .bind(hash.to_vec())
    .fetch_optional(&mut *tx)
    .await?;
    let Some((rt_id, sid, used_at, rt_exp)) = row else {
        tx.commit().await?;
        return Err(invalid());
    };
    let sess: (Uuid, bool, Option<OffsetDateTime>, OffsetDateTime, String) = sqlx::query_as(
        "SELECT s.user_id, s.mfa_verified, s.revoked_at, s.expires_at, u.status FROM sessions s \
         JOIN users u ON u.tenant_id = s.tenant_id AND u.id = s.user_id WHERE s.id = $1 FOR UPDATE OF s",
    )
    .bind(sid)
    .fetch_one(&mut *tx)
    .await?;
    let (uid, mfa, revoked_at, s_exp, ustatus) = sess;
    if used_at.is_some() {
        // Reuse of a rotated token: assume theft, kill the whole token family.
        if revoked_at.is_none() {
            revoke_session(&mut tx, tenant, sid, "refresh_token_reuse").await?;
            audit::append(
                &mut tx,
                tenant,
                Record {
                    actor_user_id: None,
                    actor_kind: "system",
                    action: "auth.session_revoked",
                    target_type: "session",
                    target_id: Some(sid.to_string()),
                    details: json!({ "reason": "refresh_token_reuse" }),
                },
            )
            .await?;
        }
        tx.commit().await?;
        sec::record(
            &st.pool,
            Some(tenant),
            Kind::RefreshTokenReuse,
            Some(uid),
            Some(&ip),
            json!({ "session_id": sid }),
        )
        .await;
        return Err(invalid());
    }
    let now = st.clock.now();
    if revoked_at.is_some() || s_exp <= now || rt_exp <= now || ustatus != "active" {
        tx.commit().await?;
        return Err(invalid());
    }
    let access = crypto::issue_token(TokenKind::Access, tenant);
    let next = crypto::issue_token(TokenKind::Refresh, tenant);
    sqlx::query("UPDATE refresh_tokens SET used_at = $2 WHERE id = $1")
        .bind(rt_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO refresh_tokens (id, tenant_id, session_id, token_hash, expires_at) VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant)
    .bind(sid)
    .bind(next.hash.to_vec())
    .bind(s_exp)
    .execute(&mut *tx)
    .await?;
    let acc_exp = (now + Duration::seconds(st.cfg.access_ttl_secs)).min(s_exp);
    sqlx::query("UPDATE sessions SET access_hash = $2, access_expires_at = $3 WHERE id = $1")
        .bind(sid)
        .bind(access.hash.to_vec())
        .bind(acc_exp)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(TokenResp {
        access_token: access.value,
        refresh_token: next.value,
        token_type: "Bearer",
        expires_in: (acc_exp - now).whole_seconds(),
        mfa_verified: mfa,
    }))
}

pub async fn revoke_session(tx: &mut Tx, tenant: Uuid, sid: Uuid, reason: &str) -> Result<(), sqlx::Error> {
    db::set_tenant(tx, tenant).await?;
    sqlx::query("UPDATE sessions SET revoked_at = now(), revoked_reason = $2 WHERE id = $1 AND revoked_at IS NULL")
        .bind(sid)
        .bind(reason)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Revokes all live sessions of a user, optionally keeping one. Returns the number revoked.
pub async fn revoke_user_sessions(
    tx: &mut Tx,
    tenant: Uuid,
    user: Uuid,
    keep: Option<Uuid>,
    reason: &str,
) -> Result<u64, sqlx::Error> {
    db::set_tenant(tx, tenant).await?;
    let r = sqlx::query(
        "UPDATE sessions SET revoked_at = now(), revoked_reason = $3 \
         WHERE user_id = $1 AND revoked_at IS NULL AND ($2::uuid IS NULL OR id <> $2)",
    )
    .bind(user)
    .bind(keep)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    Ok(r.rows_affected())
}

pub async fn logout(State(st): State<AppState>, p: Principal) -> ApiResult<Json<serde_json::Value>> {
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    revoke_session(&mut tx, p.tenant_id, p.session_id, "logout").await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(p.user_id, "auth.logout", "session", p.session_id, json!({})),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Serialize)]
pub struct MeResp {
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: &'static str,
    pub mfa_enabled: bool,
    pub session_mfa_verified: bool,
}

pub async fn me(State(st): State<AppState>, p: Principal) -> ApiResult<Json<MeResp>> {
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (email, display_name, mfa_enabled): (String, String, bool) =
        sqlx::query_as("SELECT email, display_name, totp_enabled FROM users WHERE id = $1")
            .bind(p.user_id)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(Json(MeResp {
        user_id: p.user_id,
        tenant_id: p.tenant_id,
        email,
        display_name,
        role: p.role.as_str(),
        mfa_enabled,
        session_mfa_verified: p.mfa_verified,
    }))
}

#[derive(Serialize)]
pub struct TotpSetupResp {
    pub secret_base32: String,
    pub otpauth_uri: String,
}

#[derive(Deserialize)]
pub struct PasswordReq {
    pub password: String,
}

/// Re-authenticates the session's user with their password; failures count toward lockout and revoke on lock.
pub(crate) async fn reauthenticate(st: &AppState, p: &Principal, password: String) -> ApiResult<()> {
    if password.len() > 1024 {
        return Err(ApiError::BadRequest("field_too_long"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let phash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(p.user_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    if verify_pw_blocking(st, password, Some(phash)).await? {
        return Ok(());
    }
    let locked = register_failure(st, p.tenant_id, p.user_id).await?;
    sec::record(
        &st.pool,
        Some(p.tenant_id),
        Kind::LoginFailed,
        Some(p.user_id),
        Some(&p.ip),
        json!({ "reason": "reauth_bad_password" }),
    )
    .await;
    if locked {
        let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
        revoke_user_sessions(&mut tx, p.tenant_id, p.user_id, None, "locked_after_reauth_failures").await?;
        tx.commit().await?;
        sec::record(
            &st.pool,
            Some(p.tenant_id),
            Kind::AccountLocked,
            Some(p.user_id),
            Some(&p.ip),
            json!({ "via": "reauth" }),
        )
        .await;
    }
    Err(ApiError::Unauthorized("invalid_credentials"))
}

/// Starts TOTP enrolment. Requires the current password so a stolen access token alone cannot bind an authenticator.
pub async fn totp_setup(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<PasswordReq>,
) -> ApiResult<Json<TotpSetupResp>> {
    reauthenticate(&st, &p, req.password).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (email, enabled): (String, bool) =
        sqlx::query_as("SELECT email, totp_enabled FROM users WHERE id = $1 FOR UPDATE")
            .bind(p.user_id)
            .fetch_one(&mut *tx)
            .await?;
    if enabled {
        return Err(ApiError::Conflict("mfa_already_enabled"));
    }
    let secret = crypto::random_bytes::<20>();
    let enc = st
        .cfg
        .data_keys
        .seal(&secret, &crypto::totp_aad(p.tenant_id, p.user_id))
        .map_err(ApiError::internal)?;
    sqlx::query("UPDATE users SET totp_pending_enc = $2, updated_at = now() WHERE id = $1")
        .bind(p.user_id)
        .bind(enc)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(TotpSetupResp {
        secret_base32: crypto::base32(&secret),
        otpauth_uri: crypto::otpauth_uri("KIZUNA", &email, &secret),
    }))
}

#[derive(Deserialize)]
pub struct CodeReq {
    pub code: String,
}

pub async fn totp_confirm(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<CodeReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (pending, enabled): (Option<Vec<u8>>, bool) =
        sqlx::query_as("SELECT totp_pending_enc, totp_enabled FROM users WHERE id = $1 FOR UPDATE")
            .bind(p.user_id)
            .fetch_one(&mut *tx)
            .await?;
    if enabled {
        return Err(ApiError::Conflict("mfa_already_enabled"));
    }
    let pending = pending.ok_or(ApiError::Conflict("mfa_setup_not_started"))?;
    let secret = st
        .cfg
        .data_keys
        .open(&pending, &crypto::totp_aad(p.tenant_id, p.user_id))
        .ok_or_else(|| ApiError::internal("pending totp undecryptable"))?;
    let step = crypto::totp_verify(&secret, &req.code, st.clock.now().unix_timestamp(), None)
        .ok_or(ApiError::BadRequest("invalid_code"))?;
    sqlx::query(
        "UPDATE users SET totp_secret_enc = totp_pending_enc, totp_pending_enc = NULL, totp_enabled = true, \
         totp_last_step = $2, updated_at = now() WHERE id = $1",
    )
    .bind(p.user_id)
    .bind(step)
    .execute(&mut *tx)
    .await?;
    // The enrolling session is NOT upgraded: privileged actions need a fresh login with password + TOTP,
    // so a password-only attacker cannot turn enrolment into an MFA-verified session.
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "auth.mfa_enrolled",
            "user",
            p.user_id,
            json!({ "method": "totp" }),
        ),
    )
    .await?;
    tx.commit().await?;
    sec::record(
        &st.pool,
        Some(p.tenant_id),
        Kind::MfaEnrolled,
        Some(p.user_id),
        Some(&p.ip),
        json!({ "method": "totp" }),
    )
    .await;
    Ok(Json(
        json!({ "ok": true, "relogin_required_for_privileged_actions": true }),
    ))
}

#[derive(Deserialize)]
pub struct PasswordChangeReq {
    pub current_password: String,
    pub new_password: String,
}

pub async fn change_password(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<PasswordChangeReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let email: String = {
        let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
        let e = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
            .bind(p.user_id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        e
    };
    crypto::check_password_policy(&req.new_password, &email).map_err(|e| ApiError::BadRequest(e.code()))?;
    reauthenticate(&st, &p, req.current_password).await?;
    let newpw = req.new_password;
    let h = tokio::task::spawn_blocking(move || crypto::hash_password(&newpw))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    sqlx::query("UPDATE users SET password_hash = $2, updated_at = now() WHERE id = $1")
        .bind(p.user_id)
        .bind(h)
        .execute(&mut *tx)
        .await?;
    let n = revoke_user_sessions(&mut tx, p.tenant_id, p.user_id, Some(p.session_id), "password_changed").await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "auth.password_changed",
            "user",
            p.user_id,
            json!({ "other_sessions_revoked": n }),
        ),
    )
    .await?;
    tx.commit().await?;
    sec::record(
        &st.pool,
        Some(p.tenant_id),
        Kind::PasswordChanged,
        Some(p.user_id),
        Some(&p.ip),
        json!({}),
    )
    .await;
    Ok(Json(
        json!({ "ok": true, "relogin_required_for_privileged_actions": true }),
    ))
}

#[cfg(test)]
mod tests {
    use super::client_ip_from;

    #[test]
    fn client_ip_uses_rightmost_trusted_hop() {
        let sock = Some("10.0.0.1".parse().unwrap());
        assert_eq!(client_ip_from(Some("6.6.6.6, 203.0.113.9"), sock, 1), "203.0.113.9");
        assert_eq!(
            client_ip_from(Some("1.1.1.1, 6.6.6.6, 203.0.113.9, 10.1.1.1"), sock, 2),
            "203.0.113.9"
        );
        assert_eq!(
            client_ip_from(Some("6.6.6.6"), sock, 0),
            "10.0.0.1",
            "XFF ignored without trusted proxies"
        );
        assert_eq!(client_ip_from(Some("not-an-ip"), sock, 1), "10.0.0.1");
        assert_eq!(client_ip_from(None, sock, 1), "10.0.0.1");
    }
}

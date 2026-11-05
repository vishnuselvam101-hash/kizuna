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

/// Client IP from the socket; `X-Forwarded-For` (first hop) only when the deployment trusts its proxy.
#[derive(Debug, Clone)]
pub struct ClientIp(pub String);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        if st.cfg.trust_proxy
            && let Some(ip) = parts
                .headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .and_then(|v| v.trim().parse::<IpAddr>().ok())
        {
            return Ok(ClientIp(ip.to_string()));
        }
        let ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip().to_string())
            .unwrap_or_else(|| "unknown".into());
        Ok(ClientIp(ip))
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

    pub async fn require_on(
        &self,
        st: &AppState,
        perm: Permission,
        res: &Resource<'_>,
    ) -> ApiResult<()> {
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
             WHERE s.access_hash = $1 AND s.revoked_at IS NULL AND s.access_expires_at > now() \
               AND s.expires_at > now() AND u.status = 'active' AND t.status = 'active'",
        )
        .bind(hash.to_vec())
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

async fn verify_pw_blocking(pw: String, hash: Option<String>) -> bool {
    tokio::task::spawn_blocking(move || match hash {
        Some(h) => crypto::verify_password(&pw, &h),
        None => {
            crypto::dummy_verify(&pw);
            false
        }
    })
    .await
    .unwrap_or(false)
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
    let now = OffsetDateTime::now_utc();
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
    sqlx::query("INSERT INTO refresh_tokens (id, tenant_id, session_id, token_hash, expires_at) VALUES ($1,$2,$3,$4,$5)")
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
        Record::user(
            user,
            "auth.login",
            "session",
            sid,
            json!({ "mfa": mfa, "ip": ip }),
        ),
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

type LoginUser = (
    Uuid,
    String,
    String,
    i32,
    Option<OffsetDateTime>,
    bool,
    Option<Vec<u8>>,
    Option<i64>,
);

pub async fn login(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<LoginReq>,
) -> ApiResult<Json<TokenResp>> {
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
            Some(&ip),
            json!({ "endpoint": "login", "scope": "ip" }),
        )
        .await;
        return Err(ApiError::TooManyRequests);
    }
    let email = normalize_email(&req.email);
    if req.tenant.len() > 64 || email.len() > 254 || req.password.len() > 1024 {
        return Err(ApiError::BadRequest("field_too_long"));
    }
    let tenant: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, status FROM tenants WHERE slug = $1")
            .bind(&req.tenant)
            .fetch_optional(&st.pool)
            .await?;
    let Some((tenant_id, tstatus)) = tenant.filter(|t| t.1 == "active").map(|t| (t.0, t.1)) else {
        verify_pw_blocking(req.password, None).await;
        sec::record(
            &st.pool,
            None,
            Kind::LoginFailed,
            None,
            Some(&ip),
            json!({ "reason": "unknown_tenant" }),
        )
        .await;
        return Err(fail());
    };
    let _ = tstatus;
    if !st.limiter.check(
        &format!("login:acct:{tenant_id}:{email}"),
        st.cfg.rate.login_per_account_per_min,
    ) {
        sec::record(
            &st.pool,
            Some(tenant_id),
            Kind::RateLimited,
            None,
            Some(&ip),
            json!({ "endpoint": "login", "scope": "account" }),
        )
        .await;
        return Err(ApiError::TooManyRequests);
    }

    let mut tx = db::tenant_tx(&st.pool, tenant_id).await?;
    let user: Option<LoginUser> = sqlx::query_as(
        "SELECT id, password_hash, status, failed_logins, locked_until, totp_enabled, totp_secret_enc, totp_last_step \
         FROM users WHERE email = $1 FOR UPDATE",
    )
    .bind(&email)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((uid, phash, status, failed, locked_until, totp_enabled, totp_enc, last_step)) = user
    else {
        tx.commit().await?;
        verify_pw_blocking(req.password, None).await;
        sec::record(
            &st.pool,
            Some(tenant_id),
            Kind::LoginFailed,
            None,
            Some(&ip),
            json!({ "reason": "unknown_user" }),
        )
        .await;
        return Err(fail());
    };
    let pw_ok = verify_pw_blocking(req.password, Some(phash)).await;
    let now = OffsetDateTime::now_utc();
    let reason = if status != "active" {
        Some("disabled")
    } else if locked_until.is_some_and(|t| t > now) {
        Some("locked")
    } else if !pw_ok {
        Some("bad_password")
    } else {
        None
    };
    if let Some(reason) = reason {
        let mut locked = false;
        if reason == "bad_password" {
            let n = failed + 1;
            locked = n >= st.cfg.lockout_threshold;
            sqlx::query("UPDATE users SET failed_logins = $2, locked_until = $3 WHERE id = $1")
                .bind(uid)
                .bind(if locked { 0 } else { n })
                .bind(
                    locked
                        .then(|| now + Duration::seconds(st.cfg.lockout_secs))
                        .or(locked_until),
                )
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        sec::record(
            &st.pool,
            Some(tenant_id),
            Kind::LoginFailed,
            Some(uid),
            Some(&ip),
            json!({ "reason": reason }),
        )
        .await;
        if locked {
            sec::record(
                &st.pool,
                Some(tenant_id),
                Kind::AccountLocked,
                Some(uid),
                Some(&ip),
                json!({ "minutes": st.cfg.lockout_secs / 60 }),
            )
            .await;
        }
        return Err(fail());
    }

    if totp_enabled {
        let Some(code) = req.totp_code.as_deref() else {
            tx.commit().await?;
            return Err(ApiError::Unauthorized("mfa_required"));
        };
        let secret = totp_enc
            .and_then(|enc| {
                crypto::open(
                    st.cfg.data_key.expose(),
                    &enc,
                    &crypto::totp_aad(tenant_id, uid),
                )
            })
            .ok_or_else(|| ApiError::internal("totp secret undecryptable"))?;
        match crypto::totp_verify(&secret, code, now.unix_timestamp(), last_step) {
            Some(step) => {
                sqlx::query("UPDATE users SET totp_last_step = $2 WHERE id = $1")
                    .bind(uid)
                    .bind(step)
                    .execute(&mut *tx)
                    .await?;
            }
            None => {
                let n = failed + 1;
                let locked = n >= st.cfg.lockout_threshold;
                sqlx::query("UPDATE users SET failed_logins = $2, locked_until = $3 WHERE id = $1")
                    .bind(uid)
                    .bind(if locked { 0 } else { n })
                    .bind(locked.then(|| now + Duration::seconds(st.cfg.lockout_secs)))
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                sec::record(
                    &st.pool,
                    Some(tenant_id),
                    Kind::LoginFailed,
                    Some(uid),
                    Some(&ip),
                    json!({ "reason": "bad_totp" }),
                )
                .await;
                return Err(fail());
            }
        }
    }
    sqlx::query("UPDATE users SET failed_logins = 0, locked_until = NULL WHERE id = $1")
        .bind(uid)
        .execute(&mut *tx)
        .await?;
    let resp = start_session(&st, &mut tx, tenant_id, uid, totp_enabled, &ip).await?;
    tx.commit().await?;
    sec::record(
        &st.pool,
        Some(tenant_id),
        Kind::LoginSucceeded,
        Some(uid),
        Some(&ip),
        json!({ "mfa": totp_enabled }),
    )
    .await;
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
    if !st.limiter.check(
        &format!("refresh:ip:{ip}"),
        st.cfg.rate.refresh_per_ip_per_min,
    ) {
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
    let (tenant, hash) =
        crypto::parse_token(TokenKind::Refresh, &req.refresh_token).ok_or_else(invalid)?;
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
    let now = OffsetDateTime::now_utc();
    if revoked_at.is_some() || s_exp <= now || rt_exp <= now || ustatus != "active" {
        tx.commit().await?;
        return Err(invalid());
    }
    let access = crypto::issue_token(TokenKind::Access, tenant);
    let next = crypto::issue_token(TokenKind::Refresh, tenant);
    sqlx::query("UPDATE refresh_tokens SET used_at = now() WHERE id = $1")
        .bind(rt_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO refresh_tokens (id, tenant_id, session_id, token_hash, expires_at) VALUES ($1,$2,$3,$4,$5)")
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

pub async fn revoke_session(
    tx: &mut Tx,
    tenant: Uuid,
    sid: Uuid,
    reason: &str,
) -> Result<(), sqlx::Error> {
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

pub async fn logout(
    State(st): State<AppState>,
    p: Principal,
) -> ApiResult<Json<serde_json::Value>> {
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

pub async fn totp_setup(
    State(st): State<AppState>,
    p: Principal,
) -> ApiResult<Json<TotpSetupResp>> {
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
    let enc = crypto::seal(
        st.cfg.data_key.expose(),
        &secret,
        &crypto::totp_aad(p.tenant_id, p.user_id),
    )
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
    let secret = crypto::open(
        st.cfg.data_key.expose(),
        &pending,
        &crypto::totp_aad(p.tenant_id, p.user_id),
    )
    .ok_or_else(|| ApiError::internal("pending totp undecryptable"))?;
    let step = crypto::totp_verify(
        &secret,
        &req.code,
        OffsetDateTime::now_utc().unix_timestamp(),
        None,
    )
    .ok_or(ApiError::BadRequest("invalid_code"))?;
    sqlx::query(
        "UPDATE users SET totp_secret_enc = totp_pending_enc, totp_pending_enc = NULL, totp_enabled = true, \
         totp_last_step = $2, updated_at = now() WHERE id = $1",
    )
    .bind(p.user_id)
    .bind(step)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE sessions SET mfa_verified = true WHERE id = $1")
        .bind(p.session_id)
        .execute(&mut *tx)
        .await?;
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
    Ok(Json(json!({ "ok": true })))
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
    crypto::check_password_policy(&req.new_password)
        .map_err(|_| ApiError::BadRequest("password_policy"))?;
    if req.current_password.len() > 1024 {
        return Err(ApiError::BadRequest("field_too_long"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let phash: String =
        sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1 FOR UPDATE")
            .bind(p.user_id)
            .fetch_one(&mut *tx)
            .await?;
    if !verify_pw_blocking(req.current_password, Some(phash)).await {
        tx.commit().await?;
        sec::record(
            &st.pool,
            Some(p.tenant_id),
            Kind::LoginFailed,
            Some(p.user_id),
            Some(&p.ip),
            json!({ "reason": "password_change_bad_current" }),
        )
        .await;
        return Err(ApiError::Unauthorized("invalid_credentials"));
    }
    let newpw = req.new_password;
    let h = tokio::task::spawn_blocking(move || crypto::hash_password(&newpw))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    sqlx::query("UPDATE users SET password_hash = $2, updated_at = now() WHERE id = $1")
        .bind(p.user_id)
        .bind(h)
        .execute(&mut *tx)
        .await?;
    let n = revoke_user_sessions(
        &mut tx,
        p.tenant_id,
        p.user_id,
        Some(p.session_id),
        "password_changed",
    )
    .await?;
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
    Ok(Json(json!({ "ok": true })))
}

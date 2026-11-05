//! WebAuthn (phishing-resistant second factor, ADR 0006) via `webauthn-rs`, plus one-time recovery codes.
//!
//! - Enrolment needs the current password; finishing it revokes every session of the user (fresh login required).
//! - Once enrolled, `/auth/login` refuses the account (`webauthn_required`); sessions come only from
//!   `/auth/webauthn/login/*` (password, then assertion with user verification) and are MFA-verified.
//! - Lockout-proof recovery, in order: a one-time recovery code (issued at first enrolment) clears the user's
//!   credentials and yields a non-MFA session; another tenant admin can reset MFA (`/users/{id}/reset-mfa`, never
//!   on themselves); the operator CLI `kizuna-admin reset-mfa` works when no admin can sign in.
//! - Ceremony state is stored server-side, sealed with the data key, single use (DELETE ... RETURNING), 5 min TTL.

use axum::Json;
use axum::extract::State;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::Duration;
use uuid::Uuid;
use webauthn_rs::prelude::*;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::{
    self, ClientIp, PasswordReq, Principal, TokenResp, complete_login, first_factor, login_failed, reauthenticate,
};
use crate::crypto;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiResult};
use crate::security_events::{self as sec, Kind};

const CHALLENGE_TTL_SECS: i64 = 300;
const RECOVERY_CODES: usize = 10;

pub fn build(rp_id: &str, origin: &str) -> Result<Webauthn, WebauthnError> {
    let url = Url::parse(origin).map_err(|_| WebauthnError::Configuration)?;
    WebauthnBuilder::new(rp_id, &url)?.rp_name("KIZUNA").build()
}

fn aad(label: &[u8], tenant: Uuid, user: Uuid, id: Uuid) -> Vec<u8> {
    [label, tenant.as_bytes(), user.as_bytes(), id.as_bytes()].concat()
}

pub fn credential_aad(tenant: Uuid, user: Uuid, id: Uuid) -> Vec<u8> {
    aad(b"kizuna-webauthn-cred-v1", tenant, user, id)
}

pub fn challenge_aad(tenant: Uuid, user: Uuid, id: Uuid) -> Vec<u8> {
    aad(b"kizuna-webauthn-chal-v1", tenant, user, id)
}

fn seal_json(st: &AppState, v: &impl Serialize, aad: &[u8]) -> ApiResult<Vec<u8>> {
    let pt = serde_json::to_vec(v).map_err(ApiError::internal)?;
    st.cfg.data_keys.seal(&pt, aad).map_err(ApiError::internal)
}

fn open_json<T: DeserializeOwned>(st: &AppState, blob: &[u8], aad: &[u8]) -> ApiResult<T> {
    let pt = st
        .cfg
        .data_keys
        .open(blob, aad)
        .ok_or_else(|| ApiError::internal("webauthn blob undecryptable"))?;
    serde_json::from_slice(&pt).map_err(ApiError::internal)
}

async fn store_challenge(
    st: &AppState,
    tx: &mut Tx,
    tenant: Uuid,
    uid: Uuid,
    purpose: &str,
    state: &impl Serialize,
) -> ApiResult<Uuid> {
    // Bounded storage: drop the user's expired challenges and any earlier one for the same purpose.
    sqlx::query("DELETE FROM webauthn_challenges WHERE user_id = $1 AND (expires_at <= $2 OR purpose = $3)")
        .bind(uid)
        .bind(st.clock.now())
        .bind(purpose)
        .execute(&mut **tx)
        .await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO webauthn_challenges (id, tenant_id, user_id, purpose, state_enc, expires_at) \
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(id)
    .bind(tenant)
    .bind(uid)
    .bind(purpose)
    .bind(seal_json(st, state, &challenge_aad(tenant, uid, id))?)
    .bind(st.clock.now() + Duration::seconds(CHALLENGE_TTL_SECS))
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

/// Consumes a live challenge (single use). `uid` restricts it to that user.
async fn take_challenge<T: DeserializeOwned>(
    st: &AppState,
    tx: &mut Tx,
    tenant: Uuid,
    id: Uuid,
    purpose: &str,
    uid: Option<Uuid>,
) -> ApiResult<Option<(Uuid, T)>> {
    let row: Option<(Uuid, Vec<u8>)> = sqlx::query_as(
        "DELETE FROM webauthn_challenges WHERE id = $1 AND purpose = $2 AND expires_at > $3 \
           AND ($4::uuid IS NULL OR user_id = $4) RETURNING user_id, state_enc",
    )
    .bind(id)
    .bind(purpose)
    .bind(st.clock.now())
    .bind(uid)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        None => Ok(None),
        Some((u, blob)) => Ok(Some((u, open_json(st, &blob, &challenge_aad(tenant, u, id))?))),
    }
}

async fn load_passkeys(st: &AppState, tx: &mut Tx, tenant: Uuid, uid: Uuid) -> ApiResult<Vec<(Uuid, Passkey)>> {
    let rows: Vec<(Uuid, Vec<u8>)> =
        sqlx::query_as("SELECT id, passkey_enc FROM webauthn_credentials WHERE user_id = $1 ORDER BY created_at")
            .bind(uid)
            .fetch_all(&mut **tx)
            .await?;
    rows.into_iter()
        .map(|(id, blob)| Ok((id, open_json(st, &blob, &credential_aad(tenant, uid, id))?)))
        .collect()
}

/// Removes every WebAuthn credential, pending ceremony and recovery code of a user (MFA reset paths).
pub async fn clear_all(tx: &mut Tx, uid: Uuid) -> Result<(), sqlx::Error> {
    for q in [
        "DELETE FROM webauthn_credentials WHERE user_id = $1",
        "DELETE FROM webauthn_challenges WHERE user_id = $1",
        "DELETE FROM mfa_recovery_codes WHERE user_id = $1",
        "UPDATE users SET webauthn_enabled = false WHERE id = $1",
    ] {
        sqlx::query(q).bind(uid).execute(&mut **tx).await?;
    }
    Ok(())
}

fn hash_recovery_code(code: &str) -> [u8; 32] {
    let norm: String = code
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    crypto::sha256(norm.as_bytes())
}

/// Replaces the user's recovery codes; returns them once (128 random bits each, base32, dash-grouped).
async fn issue_recovery_codes(tx: &mut Tx, tenant: Uuid, uid: Uuid) -> ApiResult<Vec<String>> {
    sqlx::query("DELETE FROM mfa_recovery_codes WHERE user_id = $1")
        .bind(uid)
        .execute(&mut **tx)
        .await?;
    let mut out = Vec::with_capacity(RECOVERY_CODES);
    for _ in 0..RECOVERY_CODES {
        let raw = crypto::base32(&crypto::random_bytes::<16>());
        let code = raw
            .as_bytes()
            .chunks(6)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect::<Vec<_>>()
            .join("-");
        sqlx::query("INSERT INTO mfa_recovery_codes (id, tenant_id, user_id, code_hash) VALUES ($1,$2,$3,$4)")
            .bind(Uuid::now_v7())
            .bind(tenant)
            .bind(uid)
            .bind(hash_recovery_code(&code).to_vec())
            .execute(&mut **tx)
            .await?;
        out.push(code);
    }
    Ok(out)
}

#[derive(Serialize)]
pub struct RegisterStartResp {
    pub challenge_id: Uuid,
    pub public_key: CreationChallengeResponse,
}

/// Starts security-key enrolment; needs the current password (a stolen access token alone cannot bind a key).
pub async fn register_start(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<PasswordReq>,
) -> ApiResult<Json<RegisterStartResp>> {
    reauthenticate(&st, &p, req.password).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (email, name, enabled): (String, String, bool) =
        sqlx::query_as("SELECT email, display_name, webauthn_enabled FROM users WHERE id = $1")
            .bind(p.user_id)
            .fetch_one(&mut *tx)
            .await?;
    // Adding a key to an account already protected by one needs a session that used a key.
    if enabled && !p.mfa_verified {
        return Err(ApiError::Forbidden("mfa_required"));
    }
    let existing = load_passkeys(&st, &mut tx, p.tenant_id, p.user_id).await?;
    let exclude = existing.iter().map(|(_, k)| k.cred_id().clone()).collect();
    let (ccr, state) = st
        .webauthn
        .start_passkey_registration(p.user_id, &email, &name, Some(exclude))
        .map_err(ApiError::internal)?;
    let challenge_id = store_challenge(&st, &mut tx, p.tenant_id, p.user_id, "register", &state).await?;
    tx.commit().await?;
    Ok(Json(RegisterStartResp {
        challenge_id,
        public_key: ccr,
    }))
}

#[derive(Deserialize)]
pub struct RegisterFinishReq {
    pub challenge_id: Uuid,
    pub credential: RegisterPublicKeyCredential,
    pub label: Option<String>,
}

pub async fn register_finish(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<RegisterFinishReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let label = req.label.unwrap_or_else(|| "security key".into());
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 100 {
        return Err(ApiError::BadRequest("invalid_label"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let Some((_, state)) =
        take_challenge::<PasskeyRegistration>(&st, &mut tx, p.tenant_id, req.challenge_id, "register", Some(p.user_id))
            .await?
    else {
        return Err(ApiError::BadRequest("invalid_challenge"));
    };
    let passkey = match st.webauthn.finish_passkey_registration(&req.credential, &state) {
        Ok(k) => k,
        Err(_) => {
            tx.commit().await?; // the challenge stays consumed
            return Err(ApiError::BadRequest("webauthn_registration_failed"));
        }
    };
    let cred_row = Uuid::now_v7();
    let ins = sqlx::query(
        "INSERT INTO webauthn_credentials (id, tenant_id, user_id, cred_id, passkey_enc, label) \
         VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (tenant_id, cred_id) DO NOTHING",
    )
    .bind(cred_row)
    .bind(p.tenant_id)
    .bind(p.user_id)
    .bind(passkey.cred_id().to_vec())
    .bind(seal_json(
        &st,
        &passkey,
        &credential_aad(p.tenant_id, p.user_id, cred_row),
    )?)
    .bind(label)
    .execute(&mut *tx)
    .await?;
    if ins.rows_affected() == 0 {
        return Err(ApiError::Conflict("credential_exists"));
    }
    let was_enabled: bool = sqlx::query_scalar("SELECT webauthn_enabled FROM users WHERE id = $1 FOR UPDATE")
        .bind(p.user_id)
        .fetch_one(&mut *tx)
        .await?;
    if was_enabled && !p.mfa_verified {
        return Err(ApiError::Forbidden("mfa_required"));
    }
    sqlx::query("UPDATE users SET webauthn_enabled = true, updated_at = now() WHERE id = $1")
        .bind(p.user_id)
        .execute(&mut *tx)
        .await?;
    let recovery_codes = match was_enabled {
        true => None,
        false => Some(issue_recovery_codes(&mut tx, p.tenant_id, p.user_id).await?),
    };
    // Existing sessions were established without the key; all must log in again through WebAuthn.
    let n = auth::revoke_user_sessions(&mut tx, p.tenant_id, p.user_id, None, "webauthn_enrolled").await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "auth.mfa_enrolled",
            "user",
            p.user_id,
            json!({ "method": "webauthn", "credential": cred_row, "sessions_revoked": n }),
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
        json!({ "method": "webauthn" }),
    )
    .await;
    Ok(Json(json!({
        "ok": true,
        "credential_id": cred_row,
        "recovery_codes": recovery_codes,
        "relogin_required": true,
    })))
}

#[derive(Deserialize)]
pub struct WebauthnLoginStartReq {
    pub tenant: String,
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginStartResp {
    pub challenge_id: Uuid,
    pub public_key: RequestChallengeResponse,
}

pub async fn login_start(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<WebauthnLoginStartReq>,
) -> ApiResult<Json<LoginStartResp>> {
    let ff = first_factor(&st, &ip, &req.tenant, &req.email, req.password).await?;
    if !ff.webauthn_enabled {
        return Err(ApiError::Conflict("webauthn_not_enrolled"));
    }
    let mut tx = db::tenant_tx(&st.pool, ff.tenant_id).await?;
    let keys: Vec<Passkey> = load_passkeys(&st, &mut tx, ff.tenant_id, ff.uid)
        .await?
        .into_iter()
        .map(|(_, k)| k)
        .collect();
    let (rcr, state) = st
        .webauthn
        .start_passkey_authentication(&keys)
        .map_err(ApiError::internal)?;
    let challenge_id = store_challenge(&st, &mut tx, ff.tenant_id, ff.uid, "login", &state).await?;
    tx.commit().await?;
    Ok(Json(LoginStartResp {
        challenge_id,
        public_key: rcr,
    }))
}

#[derive(Deserialize)]
pub struct WebauthnLoginFinishReq {
    pub tenant: String,
    pub challenge_id: Uuid,
    pub credential: PublicKeyCredential,
}

pub async fn login_finish(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<WebauthnLoginFinishReq>,
) -> ApiResult<Json<TokenResp>> {
    let invalid = || ApiError::Unauthorized("invalid_credentials");
    if !st
        .limiter
        .check(&format!("login:ip:{ip}"), st.cfg.rate.login_per_ip_per_min)
    {
        return Err(ApiError::TooManyRequests);
    }
    if req.tenant.len() > 64 {
        return Err(ApiError::BadRequest("field_too_long"));
    }
    let tenant: Option<Uuid> = sqlx::query_scalar("SELECT id FROM tenants WHERE slug = $1 AND status = 'active'")
        .bind(&req.tenant)
        .fetch_optional(&st.pool)
        .await?;
    let tenant = tenant.ok_or_else(invalid)?;
    // A block applied after the challenge was issued still applies (review M5-M7 I2).
    if crate::incidents::ip_blocked(&st.pool, tenant, &ip).await? {
        sec::try_record(
            &st.pool,
            Some(tenant),
            Kind::BlockedRequest,
            None,
            Some(&ip),
            json!({ "endpoint": "webauthn_login" }),
        )
        .await
        .ok();
        return Err(ApiError::Forbidden("blocked"));
    }
    let mut tx = db::tenant_tx(&st.pool, tenant).await?;
    let Some((uid, state)) =
        take_challenge::<PasskeyAuthentication>(&st, &mut tx, tenant, req.challenge_id, "login", None).await?
    else {
        return Err(invalid());
    };
    let res = match st.webauthn.finish_passkey_authentication(&req.credential, &state) {
        Ok(r) => r,
        Err(_) => {
            tx.commit().await?; // the challenge stays consumed
            return Err(login_failed(&st, tenant, uid, &ip, "bad_webauthn").await);
        }
    };
    // The credential must still be registered (an MFA reset between start and finish removes it).
    let row: Option<(Uuid, Vec<u8>)> = sqlx::query_as(
        "SELECT id, passkey_enc FROM webauthn_credentials WHERE user_id = $1 AND cred_id = $2 FOR UPDATE",
    )
    .bind(uid)
    .bind(res.cred_id().to_vec())
    .fetch_optional(&mut *tx)
    .await?;
    let Some((cred_row, blob)) = row else {
        tx.commit().await?;
        return Err(login_failed(&st, tenant, uid, &ip, "bad_webauthn").await);
    };
    let aad = credential_aad(tenant, uid, cred_row);
    let mut passkey: Passkey = open_json(&st, &blob, &aad)?;
    passkey.update_credential(&res);
    sqlx::query("UPDATE webauthn_credentials SET passkey_enc = $2, last_used_at = $3 WHERE id = $1")
        .bind(cred_row)
        .bind(seal_json(&st, &passkey, &aad)?)
        .bind(st.clock.now())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(complete_login(&st, tenant, uid, None, true, true, &ip).await?))
}

#[derive(Deserialize)]
pub struct RecoverReq {
    pub tenant: String,
    pub email: String,
    pub password: String,
    pub recovery_code: String,
}

/// Lost security key: password + one-time recovery code. Clears all of the user's WebAuthn credentials and
/// returns a session that is NOT MFA-verified (privileged actions need a re-enrolled factor and a new login).
pub async fn recover(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<RecoverReq>,
) -> ApiResult<Json<TokenResp>> {
    if req.recovery_code.len() > 64 {
        return Err(ApiError::BadRequest("field_too_long"));
    }
    let ff = first_factor(&st, &ip, &req.tenant, &req.email, req.password).await?;
    if !ff.webauthn_enabled {
        return Err(ApiError::Conflict("webauthn_not_enrolled"));
    }
    let mut tx = db::tenant_tx(&st.pool, ff.tenant_id).await?;
    let used: Option<Uuid> = sqlx::query_scalar(
        "UPDATE mfa_recovery_codes SET used_at = $3 WHERE user_id = $1 AND code_hash = $2 AND used_at IS NULL \
         RETURNING id",
    )
    .bind(ff.uid)
    .bind(hash_recovery_code(&req.recovery_code).to_vec())
    .bind(st.clock.now())
    .fetch_optional(&mut *tx)
    .await?;
    if used.is_none() {
        tx.rollback().await?;
        return Err(login_failed(&st, ff.tenant_id, ff.uid, &ip, "bad_recovery_code").await);
    }
    clear_all(&mut tx, ff.uid).await?;
    let n = auth::revoke_user_sessions(&mut tx, ff.tenant_id, ff.uid, None, "mfa_recovery").await?;
    audit::append(
        &mut tx,
        ff.tenant_id,
        Record::user(
            ff.uid,
            "auth.mfa_recovery_used",
            "user",
            ff.uid,
            json!({ "method": "webauthn", "sessions_revoked": n }),
        ),
    )
    .await?;
    tx.commit().await?;
    sec::record(
        &st.pool,
        Some(ff.tenant_id),
        Kind::MfaReset,
        Some(ff.uid),
        Some(&ip),
        json!({ "via": "recovery_code", "method": "webauthn" }),
    )
    .await;
    Ok(Json(
        complete_login(&st, ff.tenant_id, ff.uid, None, false, false, &ip).await?,
    ))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct CredentialOut {
    pub id: Uuid,
    pub label: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<time::OffsetDateTime>,
}

#[derive(Serialize)]
pub struct MfaStatus {
    pub totp_enabled: bool,
    pub webauthn_enabled: bool,
    pub credentials: Vec<CredentialOut>,
    pub recovery_codes_remaining: i64,
}

/// The caller's own second factors (no secret material): enrolled methods, security keys, unused recovery codes.
pub async fn status(State(st): State<AppState>, p: Principal) -> ApiResult<Json<MfaStatus>> {
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (totp_enabled, webauthn_enabled): (bool, bool) =
        sqlx::query_as("SELECT totp_enabled, webauthn_enabled FROM users WHERE id = $1")
            .bind(p.user_id)
            .fetch_one(&mut *tx)
            .await?;
    let credentials: Vec<CredentialOut> = sqlx::query_as(
        "SELECT id, label, created_at, last_used_at FROM webauthn_credentials WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(p.user_id)
    .fetch_all(&mut *tx)
    .await?;
    let recovery_codes_remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mfa_recovery_codes WHERE user_id = $1 AND used_at IS NULL")
            .bind(p.user_id)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(Json(MfaStatus {
        totp_enabled,
        webauthn_enabled,
        credentials,
        recovery_codes_remaining,
    }))
}

/// Step-up for changes to an existing security-key setup: the current password AND a session that was established
/// with a security key (a stolen password-only or recovery session cannot weaken or re-key the account).
async fn step_up_with_key(st: &AppState, p: &Principal, password: String) -> ApiResult<()> {
    reauthenticate(st, p, password).await?;
    if !p.mfa_verified {
        return Err(ApiError::Forbidden("mfa_required"));
    }
    Ok(())
}

/// Removes one of the caller's security keys. Removing the last key turns security-key sign-in off, deletes the
/// recovery codes and ends every other session of the user.
pub async fn remove_credential(
    State(st): State<AppState>,
    p: Principal,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    ApiJson(req): ApiJson<PasswordReq>,
) -> ApiResult<Json<serde_json::Value>> {
    step_up_with_key(&st, &p, req.password).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    sqlx::query("SELECT 1 FROM users WHERE id = $1 FOR UPDATE")
        .bind(p.user_id)
        .execute(&mut *tx)
        .await?;
    let gone = sqlx::query("DELETE FROM webauthn_credentials WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(p.user_id)
        .execute(&mut *tx)
        .await?;
    if gone.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM webauthn_credentials WHERE user_id = $1")
        .bind(p.user_id)
        .fetch_one(&mut *tx)
        .await?;
    let mut revoked = 0;
    if left == 0 {
        clear_all(&mut tx, p.user_id).await?;
        sqlx::query("UPDATE users SET webauthn_enabled = false, updated_at = now() WHERE id = $1")
            .bind(p.user_id)
            .execute(&mut *tx)
            .await?;
        revoked =
            auth::revoke_user_sessions(&mut tx, p.tenant_id, p.user_id, Some(p.session_id), "webauthn_removed").await?;
    }
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "auth.mfa_credential_removed",
            "user",
            p.user_id,
            json!({ "method": "webauthn", "credential": id, "remaining": left, "sessions_revoked": revoked }),
        ),
    )
    .await?;
    tx.commit().await?;
    if left == 0 {
        sec::record(
            &st.pool,
            Some(p.tenant_id),
            Kind::MfaReset,
            Some(p.user_id),
            Some(&p.ip),
            json!({ "via": "self_removed_last_key", "method": "webauthn" }),
        )
        .await;
    }
    Ok(Json(
        json!({ "ok": true, "remaining": left, "webauthn_enabled": left > 0 }),
    ))
}

/// Issues a fresh set of recovery codes (shown once); every earlier code stops working.
pub async fn regenerate_recovery_codes(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<PasswordReq>,
) -> ApiResult<Json<serde_json::Value>> {
    step_up_with_key(&st, &p, req.password).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let enabled: bool = sqlx::query_scalar("SELECT webauthn_enabled FROM users WHERE id = $1 FOR UPDATE")
        .bind(p.user_id)
        .fetch_one(&mut *tx)
        .await?;
    if !enabled {
        return Err(ApiError::Conflict("webauthn_not_enrolled"));
    }
    let codes = issue_recovery_codes(&mut tx, p.tenant_id, p.user_id).await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "auth.recovery_codes_regenerated",
            "user",
            p.user_id,
            json!({ "count": codes.len() }),
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "recovery_codes": codes })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_code_hash_normalises_formatting_only() {
        assert_eq!(hash_recovery_code("ABCDEF-GH"), hash_recovery_code(" abcdef gh "));
        assert_ne!(hash_recovery_code("ABCDEF-GH"), hash_recovery_code("ABCDEF-GI"));
    }

    #[test]
    fn relying_party_config_is_validated() {
        assert!(build("localhost", "http://localhost:8080").is_ok());
        assert!(build("kizuna.example", "https://app.kizuna.example").is_ok());
        assert!(build("kizuna.example", "https://evil.example").is_err());
        assert!(build("localhost", "not a url").is_err());
    }

    #[test]
    fn aad_binds_kind_tenant_user_and_row() {
        let (t, u, i) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        assert_ne!(credential_aad(t, u, i), challenge_aad(t, u, i));
        assert_ne!(credential_aad(t, u, i), credential_aad(u, t, i));
    }
}

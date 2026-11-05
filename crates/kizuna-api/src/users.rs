//! Tenant user administration (privileged: requires tenant_admin with an MFA-verified session).

use axum::Json;
use axum::extract::{Path, State};
use policy_engine::{Permission, Role};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, Record};
use crate::auth::{Principal, normalize_email, revoke_user_sessions};
use crate::crypto;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiResult};
use crate::security_events::{self as sec, Kind};

pub fn valid_email(e: &str) -> bool {
    let n = e.len();
    (3..=254).contains(&n)
        && e.split_once('@')
            .is_some_and(|(l, d)| !l.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.'))
        && !e.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Inserts a user in `tx` (tenant context must already be set). Used by the API and the admin CLI.
pub async fn insert_user(
    tx: &mut Tx,
    tenant: Uuid,
    email: &str,
    name: &str,
    role: Role,
    password: &str,
) -> ApiResult<Uuid> {
    let email = normalize_email(email);
    if !valid_email(&email) {
        return Err(ApiError::BadRequest("invalid_email"));
    }
    if name.trim().is_empty() || name.chars().count() > 200 {
        return Err(ApiError::BadRequest("invalid_display_name"));
    }
    crypto::check_password_policy(password, &email).map_err(|e| ApiError::BadRequest(e.code()))?;
    let pw = password.to_owned();
    let hash = tokio::task::spawn_blocking(move || crypto::hash_password(&pw))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    let id = Uuid::now_v7();
    db::set_tenant(tx, tenant).await?;
    let r = sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name, password_hash, role) VALUES ($1,$2,$3,$4,$5,$6) \
         ON CONFLICT (tenant_id, email) DO NOTHING",
    )
    .bind(id)
    .bind(tenant)
    .bind(&email)
    .bind(name.trim())
    .bind(hash)
    .bind(role.as_str())
    .execute(&mut **tx)
    .await?;
    if r.rows_affected() == 0 {
        return Err(ApiError::Conflict("email_exists"));
    }
    Ok(id)
}

#[derive(Serialize, sqlx::FromRow)]
pub struct UserOut {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub status: String,
    pub totp_enabled: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub locked_until: Option<OffsetDateTime>,
}

const USER_COLS: &str = "id, email, display_name, role, status, totp_enabled, locked_until";

pub async fn list(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Vec<UserOut>>> {
    p.require(&st, Permission::UserRead).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let rows = sqlx::query_as::<_, UserOut>(&format!("SELECT {USER_COLS} FROM users ORDER BY email"))
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct CreateReq {
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub password: String,
}

pub async fn create(
    State(st): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<CreateReq>,
) -> ApiResult<Json<UserOut>> {
    p.require(&st, Permission::UserManage).await?;
    let role = Role::parse(&req.role).map_err(|_| ApiError::BadRequest("invalid_role"))?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let id = insert_user(&mut tx, p.tenant_id, &req.email, &req.display_name, role, &req.password).await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(p.user_id, "user.created", "user", id, json!({ "role": role.as_str() })),
    )
    .await?;
    let out = sqlx::query_as::<_, UserOut>(&format!("SELECT {USER_COLS} FROM users WHERE id = $1"))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct PatchReq {
    pub role: Option<String>,
    pub status: Option<String>,
    pub display_name: Option<String>,
}

/// Locks all active admins and fails if `user` is the last one and would lose admin status.
async fn guard_last_admin(tx: &mut Tx, user: Uuid) -> ApiResult<()> {
    let admins: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE role = 'tenant_admin' AND status = 'active' FOR UPDATE")
            .fetch_all(&mut **tx)
            .await?;
    if admins.contains(&user) && admins.len() == 1 {
        return Err(ApiError::Conflict("last_active_admin"));
    }
    Ok(())
}

pub async fn patch(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
    ApiJson(req): ApiJson<PatchReq>,
) -> ApiResult<Json<UserOut>> {
    p.require(&st, Permission::UserManage).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let (old_role, old_status): (String, String) =
        sqlx::query_as("SELECT role, status FROM users WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    let new_role = match &req.role {
        Some(r) => Role::parse(r)
            .map_err(|_| ApiError::BadRequest("invalid_role"))?
            .as_str()
            .to_owned(),
        None => old_role.clone(),
    };
    let new_status = match req.status.as_deref() {
        None => old_status.clone(),
        Some(s @ ("active" | "disabled")) => s.to_owned(),
        Some(_) => return Err(ApiError::BadRequest("invalid_status")),
    };
    if let Some(n) = &req.display_name
        && (n.trim().is_empty() || n.chars().count() > 200)
    {
        return Err(ApiError::BadRequest("invalid_display_name"));
    }
    let loses_admin = old_role == "tenant_admin" && (new_role != "tenant_admin" || new_status != "active");
    if loses_admin {
        guard_last_admin(&mut tx, id).await?;
    }
    sqlx::query(
        "UPDATE users SET role = $2, status = $3, display_name = coalesce($4, display_name), updated_at = now(), \
           failed_logins = CASE WHEN $5 THEN 0 ELSE failed_logins END, \
           locked_until = CASE WHEN $5 THEN NULL ELSE locked_until END WHERE id = $1",
    )
    .bind(id)
    .bind(&new_role)
    .bind(&new_status)
    .bind(req.display_name.as_deref().map(str::trim))
    // An explicit `status: active` is the admin unlock path for locked accounts.
    .bind(req.status.as_deref() == Some("active"))
    .execute(&mut *tx)
    .await?;
    let mut revoked = 0;
    if new_status == "disabled" || new_role != old_role {
        // Role or status change: force re-authentication so sessions never carry stale privileges.
        revoked = revoke_user_sessions(&mut tx, p.tenant_id, id, None, "admin_change").await?;
    }
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "user.updated",
            "user",
            id,
            json!({
                "role": { "from": old_role, "to": new_role },
                "status": { "from": old_status, "to": new_status },
                "sessions_revoked": revoked,
            }),
        ),
    )
    .await?;
    let out = sqlx::query_as::<_, UserOut>(&format!("SELECT {USER_COLS} FROM users WHERE id = $1"))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    if old_role != new_role {
        sec::record(
            &st.pool,
            Some(p.tenant_id),
            Kind::RoleChanged,
            Some(p.user_id),
            Some(&p.ip),
            json!({ "target": id, "from": old_role, "to": new_role }),
        )
        .await;
    }
    if old_status != new_status && new_status == "disabled" {
        sec::record(
            &st.pool,
            Some(p.tenant_id),
            Kind::UserDisabled,
            Some(p.user_id),
            Some(&p.ip),
            json!({ "target": id }),
        )
        .await;
    }
    Ok(Json(out))
}

pub async fn revoke_sessions(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    p.require(&st, Permission::UserManage).await?;
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let exists: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    exists.ok_or(ApiError::NotFound)?;
    let n = revoke_user_sessions(&mut tx, p.tenant_id, id, None, "admin_revoke").await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(p.user_id, "user.sessions_revoked", "user", id, json!({ "count": n })),
    )
    .await?;
    tx.commit().await?;
    sec::record(
        &st.pool,
        Some(p.tenant_id),
        Kind::SessionsRevoked,
        Some(p.user_id),
        Some(&p.ip),
        json!({ "target": id, "count": n }),
    )
    .await;
    Ok(Json(json!({ "revoked": n })))
}

pub async fn reset_mfa(
    State(st): State<AppState>,
    p: Principal,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    p.require(&st, Permission::UserManage).await?;
    if id == p.user_id {
        return Err(ApiError::Forbidden("self_mfa_reset_not_allowed"));
    }
    let mut tx = db::tenant_tx(&st.pool, p.tenant_id).await?;
    let r = sqlx::query(
        "UPDATE users SET totp_enabled = false, totp_secret_enc = NULL, totp_pending_enc = NULL, totp_last_step = NULL, \
         updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    if r.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    crate::webauthn::clear_all(&mut tx, id).await?;
    let n = revoke_user_sessions(&mut tx, p.tenant_id, id, None, "mfa_reset").await?;
    audit::append(
        &mut tx,
        p.tenant_id,
        Record::user(
            p.user_id,
            "user.mfa_reset",
            "user",
            id,
            json!({ "sessions_revoked": n }),
        ),
    )
    .await?;
    tx.commit().await?;
    sec::record(
        &st.pool,
        Some(p.tenant_id),
        Kind::MfaReset,
        Some(p.user_id),
        Some(&p.ip),
        json!({ "target": id }),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    #[test]
    fn email_validation() {
        assert!(super::valid_email("a@b.co"));
        for bad in ["", "ab", "a@b", "@b.co", "a b@c.co", "a@.co", "a@co."] {
            assert!(!super::valid_email(bad), "{bad}");
        }
    }
}

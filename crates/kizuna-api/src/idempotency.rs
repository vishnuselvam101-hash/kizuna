//! `Idempotency-Key` for every mutating endpoint (STABILITY_2026-11-06).
//!
//! A POST/PUT/PATCH/DELETE carrying `Idempotency-Key` from an authenticated user or supplier is recorded per
//! (tenant, principal, key) with a SHA-256 of method, path, query and body, for 24 h:
//! - first request: the row is claimed `in_progress`, the handler runs, and its response (status, content type and
//!   disposition, body sealed with the data key) is stored;
//! - same key, same request, completed: the stored response is replayed (`idempotent-replayed: true`), the handler
//!   does not run again;
//! - same key, different request: 422 `idempotency_key_reused`;
//! - same key while the first is still running: 409 `idempotency_in_progress` + Retry-After.
//!
//! Responses that mean "not executed, try again" (401, 403, 408, 429, 5xx) are not stored: the key is released so
//! a retry runs the handler. A holder that dies leaves an `in_progress` row that the same request can take over
//! after `LOCK_SECS`. Known gap: a crash after the handler committed but before the response was stored lets a
//! retry after `LOCK_SECS` run the handler a second time (the row and the handler's writes are separate
//! transactions).
//!
//! Login, refresh, MFA ceremonies and portal login/invite are excluded: they have no principal yet, return
//! credentials, and are already safe to repeat. Without the header nothing changes.

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::app::AppState;
use crate::crypto::{self, TokenKind};
use crate::db;
use crate::error::ApiError;

pub const HEADER: &str = "idempotency-key";
pub const REPLAYED: &str = "idempotent-replayed";
/// Replay window.
pub const TTL_SECS: i64 = 24 * 3600;
/// How long an `in_progress` claim blocks the key (request timeout 60 s + admission wait 20 s + margin).
pub const LOCK_SECS: i64 = 120;
/// Larger response bodies are not kept; a replay of such a key answers 409 `idempotency_response_not_stored`.
pub const MAX_STORED_BODY: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Who {
    pub tenant: Uuid,
    pub kind: &'static str,
    pub id: Uuid,
}

/// 1..=200 visible ASCII characters.
pub fn valid_key(v: &HeaderValue) -> Option<String> {
    let s = v.to_str().ok()?;
    (!s.is_empty() && s.len() <= 200 && s.bytes().all(|b| (0x21..=0x7e).contains(&b))).then(|| s.to_string())
}

/// Paths (below `/api/v1`) that never use idempotency keys.
fn excluded(path: &str) -> bool {
    path.starts_with("/auth/")
        || path == "/portal/login"
        || path == "/portal/logout"
        || path.starts_with("/portal/invite/")
}

fn is_upload(method: &Method, path: &str) -> bool {
    method == Method::POST
        && (path == "/artifacts" || (path.starts_with("/portal/requests/") && path.ends_with("/uploads")))
}

pub fn request_hash(method: &Method, uri: &str, body: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(method.as_str().as_bytes());
    h.update([0]);
    h.update(uri.as_bytes());
    h.update([0]);
    h.update(body);
    h.finalize().into()
}

fn aad(w: &Who, key: &str) -> Vec<u8> {
    format!("kizuna-idempotency:{}:{}:{}:{key}", w.tenant, w.kind, w.id).into_bytes()
}

/// The live session behind the bearer token, without side effects (the handler's extractor does the full checks).
async fn resolve(st: &AppState, headers: &HeaderMap) -> Result<Option<Who>, ApiError> {
    let Some(token) = crate::auth::bearer(headers) else {
        return Ok(None);
    };
    let now = st.clock.now();
    let (kind, tenant, hash, sql) = if let Some((t, h)) = crypto::parse_token(TokenKind::Access, token) {
        (
            "user",
            t,
            h,
            "SELECT user_id FROM sessions WHERE access_hash = $1 AND revoked_at IS NULL AND access_expires_at > $2",
        )
    } else if let Some((t, h)) = crypto::parse_token(TokenKind::Portal, token) {
        (
            "supplier",
            t,
            h,
            "SELECT supplier_user_id FROM supplier_sessions WHERE access_hash = $1 AND revoked_at IS NULL \
             AND expires_at > $2",
        )
    } else {
        return Ok(None);
    };
    let mut tx = db::tenant_tx(&st.pool, tenant).await?;
    let id: Option<Uuid> = sqlx::query_scalar(sql)
        .bind(hash.to_vec())
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(id.map(|id| Who { tenant, kind, id }))
}

enum Begin {
    Proceed,
    Mismatch,
    InProgress,
    Replay(Response),
}

type Stored = (Vec<u8>, String, Option<i32>, Option<serde_json::Value>, Option<Vec<u8>>);

async fn begin(st: &AppState, w: &Who, key: &str, hash: &[u8; 32]) -> Result<Begin, ApiError> {
    let now = st.clock.now();
    let mut tx = db::tenant_tx(&st.pool, w.tenant).await?;
    // Claim a fresh key, an expired one, or an abandoned in-progress claim of the same request.
    let claimed: Option<bool> = sqlx::query_scalar(
        "INSERT INTO idempotency_keys (tenant_id, principal_kind, principal_id, key, request_hash, state, created_at, \
                                       locked_until, expires_at) \
         VALUES ($1, $2, $3, $4, $5, 'in_progress', $6, $7, $8) \
         ON CONFLICT (tenant_id, principal_kind, principal_id, key) DO UPDATE \
           SET request_hash = EXCLUDED.request_hash, state = 'in_progress', status_code = NULL, \
               response_headers = NULL, response_body = NULL, created_at = EXCLUDED.created_at, \
               locked_until = EXCLUDED.locked_until, expires_at = EXCLUDED.expires_at \
         WHERE idempotency_keys.expires_at <= $6 \
            OR (idempotency_keys.state = 'in_progress' AND idempotency_keys.locked_until <= $6 \
                AND idempotency_keys.request_hash = EXCLUDED.request_hash) \
         RETURNING true",
    )
    .bind(w.tenant)
    .bind(w.kind)
    .bind(w.id)
    .bind(key)
    .bind(hash.to_vec())
    .bind(now)
    .bind(now + Duration::seconds(LOCK_SECS))
    .bind(now + Duration::seconds(TTL_SECS))
    .fetch_optional(&mut *tx)
    .await?;
    if claimed.is_some() {
        tx.commit().await?;
        return Ok(Begin::Proceed);
    }
    let row: Option<Stored> = sqlx::query_as(
        "SELECT request_hash, state, status_code, response_headers, response_body FROM idempotency_keys \
         WHERE tenant_id = $1 AND principal_kind = $2 AND principal_id = $3 AND key = $4",
    )
    .bind(w.tenant)
    .bind(w.kind)
    .bind(w.id)
    .bind(key)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    let Some((stored_hash, state, status, headers, body)) = row else {
        // Released between the two statements (the first holder got a retryable answer): let the client retry.
        return Ok(Begin::InProgress);
    };
    if stored_hash.as_slice() != hash.as_slice() {
        return Ok(Begin::Mismatch);
    }
    if state != "completed" {
        return Ok(Begin::InProgress);
    }
    Ok(Begin::Replay(replay(st, w, key, status, headers, body)))
}

fn replay(
    st: &AppState,
    w: &Who,
    key: &str,
    status: Option<i32>,
    headers: Option<serde_json::Value>,
    body: Option<Vec<u8>>,
) -> Response {
    let unavailable = |code: &str| (StatusCode::CONFLICT, axum::Json(json!({ "error": code }))).into_response();
    let Some(status) = status.and_then(|s| StatusCode::from_u16(s as u16).ok()) else {
        return unavailable("idempotency_replay_unavailable");
    };
    let Some(sealed) = body else {
        return unavailable("idempotency_response_not_stored");
    };
    let Some(plain) = st.cfg.data_keys.open(&sealed, &aad(w, key)) else {
        return unavailable("idempotency_replay_unavailable");
    };
    st.metrics.idem_replayed.fetch_add(1, Ordering::Relaxed);
    let mut r = Response::new(Body::from(plain));
    *r.status_mut() = status;
    if let Some(serde_json::Value::Object(h)) = headers {
        for (k, v) in h {
            if let (Ok(name), Some(Ok(val))) = (
                header::HeaderName::try_from(k.as_str()),
                v.as_str().map(HeaderValue::from_str),
            ) {
                r.headers_mut().insert(name, val);
            }
        }
    }
    r.headers_mut().insert(REPLAYED, HeaderValue::from_static("true"));
    r
}

fn retryable(s: StatusCode) -> bool {
    s.is_server_error()
        || matches!(
            s,
            StatusCode::UNAUTHORIZED
                | StatusCode::FORBIDDEN
                | StatusCode::REQUEST_TIMEOUT
                | StatusCode::TOO_MANY_REQUESTS
        )
}

async fn release(st: &AppState, w: &Who, key: &str) -> Result<(), ApiError> {
    let mut tx = db::tenant_tx(&st.pool, w.tenant).await?;
    sqlx::query(
        "DELETE FROM idempotency_keys WHERE tenant_id = $1 AND principal_kind = $2 AND principal_id = $3 AND key = $4 \
         AND state = 'in_progress'",
    )
    .bind(w.tenant)
    .bind(w.kind)
    .bind(w.id)
    .bind(key)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn store(
    st: &AppState,
    w: &Who,
    key: &str,
    status: StatusCode,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<(), ApiError> {
    let mut keep = serde_json::Map::new();
    for h in [header::CONTENT_TYPE, header::CONTENT_DISPOSITION] {
        if let Some(v) = headers.get(&h).and_then(|v| v.to_str().ok()) {
            keep.insert(h.as_str().to_string(), json!(v));
        }
    }
    let sealed = if body.len() <= MAX_STORED_BODY {
        Some(st.cfg.data_keys.seal(body, &aad(w, key)).map_err(ApiError::internal)?)
    } else {
        None
    };
    let mut tx = db::tenant_tx(&st.pool, w.tenant).await?;
    sqlx::query(
        "UPDATE idempotency_keys SET state = 'completed', status_code = $5, response_headers = $6, response_body = $7 \
         WHERE tenant_id = $1 AND principal_kind = $2 AND principal_id = $3 AND key = $4 AND state = 'in_progress'",
    )
    .bind(w.tenant)
    .bind(w.kind)
    .bind(w.id)
    .bind(key)
    .bind(i32::from(status.as_u16()))
    .bind(serde_json::Value::Object(keep))
    .bind(sealed)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    st.metrics.idem_stored.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Middleware on the `/api/v1` router.
pub async fn layer(State(st): State<AppState>, req: Request, next: Next) -> Response {
    if !matches!(
        *req.method(),
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        return next.run(req).await;
    }
    let Some(raw) = req.headers().get(HEADER).cloned() else {
        return next.run(req).await;
    };
    let path = req.uri().path().to_string();
    if excluded(&path) {
        return next.run(req).await;
    }
    let Some(key) = valid_key(&raw) else {
        return ApiError::BadRequest("invalid_idempotency_key").into_response();
    };
    let who = match resolve(&st, req.headers()).await {
        Ok(Some(w)) => w,
        // No live session: the handler answers 401 as usual.
        Ok(None) => return next.run(req).await,
        Err(e) => return e.into_response(),
    };
    // The layer sits on the router nested at /api/v1, which sees the URI without that prefix.
    let full_uri = format!("/api/v1{}", req.uri());
    let limit = if is_upload(req.method(), &path) {
        st.cfg.max_upload_bytes + 64 * 1024
    } else {
        st.cfg.max_body_bytes
    };
    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, limit).await else {
        return ApiError::TooLarge.into_response();
    };
    let hash = request_hash(&parts.method, &full_uri, &bytes);
    match begin(&st, &who, &key, &hash).await {
        Err(e) => return e.into_response(),
        Ok(Begin::Replay(r)) => return r,
        Ok(Begin::Mismatch) => {
            st.metrics.idem_mismatch.fetch_add(1, Ordering::Relaxed);
            return ApiError::Unprocessable("idempotency_key_reused").into_response();
        }
        Ok(Begin::InProgress) => {
            st.metrics.idem_in_progress.fetch_add(1, Ordering::Relaxed);
            let mut r = ApiError::Conflict("idempotency_in_progress").into_response();
            r.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            return r;
        }
        Ok(Begin::Proceed) => {}
    }
    let resp = next.run(Request::from_parts(parts, Body::from(bytes))).await;
    let status = resp.status();
    if retryable(status) {
        if let Err(e) = release(&st, &who, &key).await {
            tracing::warn!(error = %e, "idempotency key release failed (expires with its lock)");
        }
        return resp;
    }
    let (rparts, rbody) = resp.into_parts();
    let rbytes = match axum::body::to_bytes(rbody, 64 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            let _ = release(&st, &who, &key).await;
            return ApiError::internal(format!("response body: {e}")).into_response();
        }
    };
    if let Err(e) = store(&st, &who, &key, status, &rparts.headers, &rbytes).await {
        // The handler's effect is committed; the client still gets its response. A retry within LOCK_SECS sees
        // 409 in-progress; after that it would run again (documented gap).
        tracing::error!(error = %e, "idempotency response not stored");
    }
    Response::from_parts(rparts, Body::from(rbytes))
}

/// Deletes expired keys of one tenant (GC job). Returns the number removed.
pub async fn purge_expired(tx: &mut db::Tx, now: OffsetDateTime) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query("DELETE FROM idempotency_keys WHERE expires_at <= $1")
        .bind(now)
        .execute(&mut **tx)
        .await?
        .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_validated() {
        assert!(valid_key(&HeaderValue::from_static("abc-123_X.y")).is_some());
        assert!(valid_key(&HeaderValue::from_static("")).is_none());
        assert!(valid_key(&HeaderValue::from_static("has space")).is_none());
        assert!(valid_key(&HeaderValue::from_str(&"k".repeat(201)).unwrap()).is_none());
        assert!(valid_key(&HeaderValue::from_str(&"k".repeat(200)).unwrap()).is_some());
    }

    #[test]
    fn hash_covers_method_uri_and_body() {
        let a = request_hash(&Method::POST, "/api/v1/claims", b"{}");
        assert_ne!(a, request_hash(&Method::PUT, "/api/v1/claims", b"{}"));
        assert_ne!(a, request_hash(&Method::POST, "/api/v1/claims?x=1", b"{}"));
        assert_ne!(a, request_hash(&Method::POST, "/api/v1/claims", b"{ }"));
        assert_eq!(a, request_hash(&Method::POST, "/api/v1/claims", b"{}"));
    }

    #[test]
    fn exclusions_and_retryable() {
        assert!(excluded("/auth/login") && excluded("/portal/invite/finish") && !excluded("/claims"));
        assert!(is_upload(&Method::POST, "/portal/requests/x/uploads") && !is_upload(&Method::GET, "/artifacts"));
        assert!(retryable(StatusCode::SERVICE_UNAVAILABLE) && retryable(StatusCode::TOO_MANY_REQUESTS));
        assert!(!retryable(StatusCode::OK) && !retryable(StatusCode::CONFLICT) && !retryable(StatusCode::BAD_REQUEST));
    }
}

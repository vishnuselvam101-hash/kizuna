//! API errors: stable machine codes to clients, details only in server logs.

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("bad request: {0}")]
    BadRequest(&'static str),
    #[error("unauthorized: {0}")]
    Unauthorized(&'static str),
    #[error("forbidden: {0}")]
    Forbidden(&'static str),
    #[error("not found")]
    NotFound,
    #[error("conflict: {0}")]
    Conflict(&'static str),
    #[error("payload too large")]
    TooLarge,
    #[error("unsupported media type")]
    UnsupportedMedia,
    #[error("too many requests")]
    TooManyRequests,
    /// The database is restarting or unreachable (connection lost, pool exhausted): retryable.
    #[error("database unavailable: {0}")]
    DbUnavailable(String),
    /// The object store has no space left.
    #[error("storage full")]
    StorageFull,
    #[error("internal: {0}")]
    Internal(String),
}

impl ApiError {
    pub fn internal(e: impl std::fmt::Display) -> Self {
        ApiError::Internal(e.to_string())
    }

    /// Object-store write errors: a full disk is reported as such (507), anything else is internal.
    pub fn storage(e: std::io::Error) -> Self {
        if matches!(
            e.kind(),
            std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
        ) {
            tracing::error!(error = %e, "object store full");
            ApiError::StorageFull
        } else {
            ApiError::Internal(format!("storage: {e}"))
        }
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        // Text PostgreSQL cannot store (a NUL byte in text, or `\u0000` in jsonb) is a client error, not a 500
        // (red-team 2026-11-05 F1).
        if let sqlx::Error::Database(d) = &e
            && matches!(d.code().as_deref(), Some("22021" | "22P05"))
        {
            return ApiError::BadRequest("invalid_text");
        }
        // Connection-level failures (PostgreSQL restarting, connections terminated, pool exhausted) are an outage the
        // client can retry, not an internal error (chaos C5). SQLSTATE class 08 = connection exception, 57P01-57P03 =
        // admin/crash shutdown, cannot connect now.
        let outage = match &e {
            sqlx::Error::Io(_) | sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::WorkerCrashed => {
                true
            }
            sqlx::Error::Database(d) => d
                .code()
                .is_some_and(|c| c.starts_with("08") || matches!(c.as_ref(), "57P01" | "57P02" | "57P03")),
            _ => false,
        };
        if outage {
            ApiError::DbUnavailable(e.to_string())
        } else {
            ApiError::Internal(format!("db: {e}"))
        }
    }
}

impl From<JsonRejection> for ApiError {
    fn from(r: JsonRejection) -> Self {
        if r.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::TooLarge
        } else if r.status() == StatusCode::UNSUPPORTED_MEDIA_TYPE {
            ApiError::UnsupportedMedia
        } else {
            ApiError::BadRequest("invalid_json")
        }
    }
}

impl From<QueryRejection> for ApiError {
    fn from(_: QueryRejection) -> Self {
        ApiError::BadRequest("invalid_query")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            ApiError::BadRequest(c) => (StatusCode::BAD_REQUEST, *c),
            ApiError::Unauthorized(c) => (StatusCode::UNAUTHORIZED, *c),
            ApiError::Forbidden(c) => (StatusCode::FORBIDDEN, *c),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            ApiError::Conflict(c) => (StatusCode::CONFLICT, *c),
            ApiError::TooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large"),
            ApiError::UnsupportedMedia => (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type"),
            ApiError::TooManyRequests => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
            ApiError::DbUnavailable(msg) => {
                tracing::error!(error = %msg, "database unavailable");
                let mut r = (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({ "error": "database_unavailable" })),
                )
                    .into_response();
                r.headers_mut().insert(
                    axum::http::header::RETRY_AFTER,
                    axum::http::HeaderValue::from_static("5"),
                );
                return r;
            }
            ApiError::StorageFull => (StatusCode::INSUFFICIENT_STORAGE, "storage_full"),
            ApiError::Internal(msg) => {
                tracing::error!(error = %msg, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
        };
        (status, Json(serde_json::json!({ "error": code }))).into_response()
    }
}

/// JSON body extractor whose rejections use [`ApiError`].
#[derive(axum::extract::FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct ApiJson<T>(pub T);

/// Query extractor whose rejections use [`ApiError`].
#[derive(axum::extract::FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(ApiError))]
pub struct ApiQuery<T>(pub T);

pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Chaos C5: a database that is restarting or unreachable is a temporary outage, not an internal error: clients and
    /// proxies get 503 `database_unavailable` (retryable) instead of 500 `internal_error`.
    #[test]
    fn database_outage_is_503() {
        for e in [
            sqlx::Error::PoolTimedOut,
            sqlx::Error::PoolClosed,
            sqlx::Error::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
        ] {
            let r = ApiError::from(e).into_response();
            assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
        }
        let r = ApiError::from(sqlx::Error::RowNotFound).into_response();
        assert_eq!(r.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// Chaos C4: a full object store is reported as 507 `storage_full`, not as an anonymous internal error.
    #[test]
    fn storage_full_is_507() {
        let r = ApiError::storage(std::io::Error::from(std::io::ErrorKind::StorageFull)).into_response();
        assert_eq!(r.status(), StatusCode::INSUFFICIENT_STORAGE);
        let r = ApiError::storage(std::io::Error::from(std::io::ErrorKind::PermissionDenied)).into_response();
        assert_eq!(r.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}

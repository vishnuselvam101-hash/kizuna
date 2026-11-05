//! Router, shared state and HTTP hardening layers.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderName, HeaderValue, header};
use axum::routing::{get, patch, post};
use policy_engine::Policy;
use sqlx::PgPool;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;

use crate::config::Config;
use crate::ratelimit::RateLimiter;
use crate::{audit, auth, security_events, users};

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub cfg: Arc<Config>,
    pub limiter: Arc<RateLimiter>,
    pub policy: Arc<Policy>,
}

impl AppState {
    pub fn new(pool: PgPool, cfg: Config) -> Self {
        Self {
            pool,
            cfg: Arc::new(cfg),
            limiter: Arc::new(RateLimiter::new(Duration::from_secs(60))),
            policy: Arc::new(Policy::new()),
        }
    }
}

const SECURITY_HEADERS: [(&str, &str); 7] = [
    ("x-content-type-options", "nosniff"),
    ("x-frame-options", "DENY"),
    (
        "content-security-policy",
        "default-src 'none'; frame-ancestors 'none'",
    ),
    ("referrer-policy", "no-referrer"),
    ("cache-control", "no-store"),
    (
        "strict-transport-security",
        "max-age=63072000; includeSubDomains",
    ),
    ("cross-origin-opener-policy", "same-origin"),
];

pub fn router(st: AppState) -> Router {
    let body_limit = st.cfg.max_body_bytes;
    let api = Router::new()
        .route("/auth/login", post(auth::login))
        .route("/auth/refresh", post(auth::refresh))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/auth/password", post(auth::change_password))
        .route("/auth/mfa/totp/setup", post(auth::totp_setup))
        .route("/auth/mfa/totp/confirm", post(auth::totp_confirm))
        .route("/users", get(users::list).post(users::create))
        .route("/users/{id}", patch(users::patch))
        .route("/users/{id}/revoke-sessions", post(users::revoke_sessions))
        .route("/users/{id}/reset-mfa", post(users::reset_mfa))
        .route("/audit/events", get(audit::list_events))
        .route("/audit/verify", get(audit::verify))
        .route("/security-events", get(security_events::list));

    let mut app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .nest("/api/v1", api)
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(RequestBodyLimitLayer::new(body_limit))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ));
    for (k, v) in SECURITY_HEADERS {
        app = app.layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static(k),
            HeaderValue::from_static(v),
        ));
    }
    app.layer(SetResponseHeaderLayer::if_not_present(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    ))
    .with_state(st)
}

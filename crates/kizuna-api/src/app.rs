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
use crate::storage::{FsStore, ObjectStore};
use crate::{artifacts, audit, auth, claims, security_events, users, verification};

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub cfg: Arc<Config>,
    pub limiter: Arc<RateLimiter>,
    pub policy: Arc<Policy>,
    /// Caps concurrent argon2 computations (CPU/RAM bound) so login floods cannot exhaust the host.
    pub hash_permits: Arc<tokio::sync::Semaphore>,
    pub store: Arc<dyn ObjectStore>,
    /// Caps concurrent sandboxed extraction workers.
    pub worker_permits: Arc<tokio::sync::Semaphore>,
}

impl AppState {
    pub fn new(pool: PgPool, cfg: Config) -> Self {
        let store: Arc<dyn ObjectStore> = Arc::new(FsStore::new(cfg.storage_dir.clone()));
        Self {
            store,
            pool,
            cfg: Arc::new(cfg),
            limiter: Arc::new(RateLimiter::new(Duration::from_secs(60))),
            policy: Arc::new(Policy::new().with_rule(verification::NoSelfReview)),
            worker_permits: Arc::new(tokio::sync::Semaphore::new(2)),
            hash_permits: Arc::new(tokio::sync::Semaphore::new(
                std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
            )),
        }
    }
}

const SECURITY_HEADERS: [(&str, &str); 7] = [
    ("x-content-type-options", "nosniff"),
    ("x-frame-options", "DENY"),
    ("content-security-policy", "default-src 'none'; frame-ancestors 'none'"),
    ("referrer-policy", "no-referrer"),
    ("cache-control", "no-store"),
    ("strict-transport-security", "max-age=63072000; includeSubDomains"),
    ("cross-origin-opener-policy", "same-origin"),
];

pub fn router(st: AppState) -> Router {
    let body_limit = st.cfg.max_body_bytes;
    let upload_limit = st.cfg.max_upload_bytes + 64 * 1024;
    let json_api = Router::new()
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
        .route("/security-events", get(security_events::list))
        .route("/artifacts", get(artifacts::list))
        .route("/artifacts/{id}", get(artifacts::get))
        .route("/artifacts/{id}/content", get(artifacts::content))
        .route("/artifacts/{id}/verify", post(artifacts::verify))
        .route("/artifacts/{id}/release", post(artifacts::release))
        .route("/claims", get(claims::list).post(claims::create))
        .route("/claims/{id}", get(claims::get))
        .route("/claims/{id}/supersede", post(claims::supersede))
        .route("/evidence/{subject_type}/{subject_key}", get(claims::subject))
        .route("/artifacts/{id}/extract", post(verification::extract))
        .route("/claims/{id}/review", post(verification::review_claim))
        .route("/verification/run", post(verification::run_detection))
        .route("/conflicts", get(verification::list_conflicts))
        .route("/conflicts/{id}", get(verification::get_conflict))
        .route("/conflicts/{id}/review", post(verification::review_conflict))
        .route("/entities/candidates", get(verification::entity_candidates))
        .route("/entities/decisions", post(verification::entity_decision))
        .route("/compare/part/{key}", get(verification::compare))
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(RequestBodyLimitLayer::new(body_limit));
    let upload_api = Router::new()
        .route("/artifacts", post(artifacts::upload))
        .layer(DefaultBodyLimit::max(upload_limit))
        .layer(RequestBodyLimitLayer::new(upload_limit));

    let mut app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .nest("/api/v1", upload_api.merge(json_api))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(60),
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

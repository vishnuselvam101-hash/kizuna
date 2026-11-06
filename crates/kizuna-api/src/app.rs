//! Router, shared state and HTTP hardening layers.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderName, HeaderValue, header};
use axum::routing::{get, patch, post, put};
use policy_engine::Policy;
use sqlx::PgPool;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;

use crate::clock::{Clock, SystemClock};
use crate::config::Config;
use crate::ratelimit::RateLimiter;
use crate::storage::{FsStore, ObjectStore};
use crate::{
    artifacts, audit, auth, claims, decisions, evidence_pack, incidents, insights, security_events, users,
    verification, webauthn,
};

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
    /// Source of "now" for every expiry decision; replaceable in tests.
    pub clock: Arc<dyn Clock>,
    pub webauthn: Arc<webauthn_rs::Webauthn>,
    /// Malware scanner after the static scan (ADR 0017); `NotConfigured` unless the server binary sets clamd.
    pub av: Arc<dyn crate::antivirus::MalwareScanner>,
    /// Operator alert channels (ADR 0017); disabled unless the server binary configures them.
    pub alerts: Arc<crate::alerts::Alerter>,
    /// Request metrics (Prometheus text on the loopback metrics port).
    pub metrics: Arc<crate::metrics::Metrics>,
    /// Bounded admission queues for uploads, extraction and heavy routes.
    pub gates: Arc<crate::admission::Gates>,
    /// Set on SIGTERM: readiness fails while in-flight requests drain.
    pub draining: Arc<std::sync::atomic::AtomicBool>,
}

impl AppState {
    pub fn new(pool: PgPool, cfg: Config) -> Self {
        let store: Arc<dyn ObjectStore> = Arc::new(FsStore::new(cfg.storage_dir.clone()));
        crate::decisions::set_inline_limit(cfg.invalidation_inline_limit);
        // Validated in Config::from_lookup.
        let webauthn = crate::webauthn::build(&cfg.webauthn_rp_id, &cfg.webauthn_origin)
            .expect("webauthn relying party validated by Config");
        Self {
            clock: Arc::new(SystemClock),
            webauthn: Arc::new(webauthn),
            av: Arc::new(crate::antivirus::NotConfigured),
            alerts: Arc::new(crate::alerts::Alerter::new(
                crate::alerts::AlertConfig::disabled(),
                None,
            )),
            store,
            pool,
            metrics: Arc::new(crate::metrics::Metrics::default()),
            gates: Arc::new(crate::admission::Gates::new(&cfg.stability)),
            draining: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
        .route("/auth/mfa/webauthn/register/start", post(webauthn::register_start))
        .route("/auth/mfa/webauthn/register/finish", post(webauthn::register_finish))
        .route("/auth/mfa/status", get(webauthn::status))
        .route(
            "/auth/mfa/webauthn/credentials/{id}/remove",
            post(webauthn::remove_credential),
        )
        .route("/auth/mfa/recovery-codes", post(webauthn::regenerate_recovery_codes))
        .route("/auth/webauthn/login/start", post(webauthn::login_start))
        .route("/auth/webauthn/login/finish", post(webauthn::login_finish))
        .route("/auth/webauthn/recover", post(webauthn::recover))
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
        .route("/artifacts/{id}/reextract", post(verification::reextract))
        .route("/claims/{id}/review", post(verification::review_claim))
        .route("/verification/run", post(verification::run_detection))
        .route("/verification/jobs", post(crate::jobs::enqueue_verification))
        .route("/jobs", get(crate::jobs::list))
        .route("/jobs/{id}", get(crate::jobs::get))
        .route("/jobs/{id}/requeue", post(crate::jobs::requeue))
        .route("/conflicts", get(verification::list_conflicts))
        .route("/conflicts/{id}", get(verification::get_conflict))
        .route("/conflicts/{id}/review", post(verification::review_conflict))
        .route("/entities/candidates", get(verification::entity_candidates))
        .route("/entities/decisions", post(verification::entity_decision))
        .route("/compare/{subject_type}/{key}", get(verification::compare))
        .route("/decisions", get(decisions::list).post(decisions::create))
        .route("/decisions/{id}", get(decisions::get))
        .route("/decisions/{id}/trace", get(decisions::trace))
        .route("/decisions/{id}/report", get(decisions::report))
        .route("/decisions/{id}/evidence-pack", post(evidence_pack::export))
        .route("/decisions/{id}/reconfirm", post(decisions::reconfirm))
        .route("/decisions/{id}/revoke", post(decisions::revoke))
        .route("/simulations", post(crate::simulations::run))
        .route("/label-synonyms", get(crate::label_synonyms::list))
        .route("/label-synonyms/{id}/revoke", post(crate::label_synonyms::revoke))
        .route("/review-batches", get(crate::review_batches::list))
        .route("/review-batches/{id}", get(crate::review_batches::get))
        .route("/match/lines", get(crate::matching::list))
        .route("/match/line", get(crate::matching::get))
        .route("/match/line/runs", post(crate::matching::record))
        .route("/match/line/accept", post(crate::matching::accept))
        .route(
            "/evidence-requests",
            get(crate::evidence_requests::list).post(crate::evidence_requests::create),
        )
        .route("/evidence-requests/preview", get(crate::evidence_requests::preview))
        .route("/evidence-requests/{id}", get(crate::evidence_requests::get))
        .route(
            "/evidence-requests/{id}/approve",
            post(crate::evidence_requests::approve),
        )
        .route("/evidence-requests/{id}/cancel", post(crate::evidence_requests::cancel))
        .route(
            "/evidence-requests/{id}/document",
            get(crate::evidence_requests::document),
        )
        .route(
            "/evidence-requests/{id}/portal-uploads",
            get(crate::portal::request_uploads),
        )
        .route(
            "/portal-users",
            get(crate::portal::list_users).post(crate::portal::invite),
        )
        .route("/portal-users/{id}/revoke", post(crate::portal::revoke))
        .route("/suppliers", get(crate::portal::list_suppliers))
        .route(
            "/evidence-requests/{id}/attach-supplier",
            post(crate::portal::attach_supplier),
        )
        // Supplier portal (ADR 0025): a separate principal; these routes accept only `kzp.` sessions.
        .route("/portal/invite/start", post(crate::portal::invite_start))
        .route("/portal/invite/finish", post(crate::portal::invite_finish))
        .route("/portal/login", post(crate::portal::login))
        .route("/portal/logout", post(crate::portal::logout))
        .route("/portal/me", get(crate::portal::me))
        .route("/portal/requests", get(crate::portal::list_requests))
        .route("/portal/requests/{id}", get(crate::portal::get_request))
        .route("/portal/requests/{id}/uploads", get(crate::portal::list_uploads))
        .route("/capa/cases", get(crate::capa::list).post(crate::capa::open))
        .route("/capa/cases/{id}", get(crate::capa::get))
        .route("/capa/cases/{id}/steps/{d}", put(crate::capa::save_step))
        .route("/capa/cases/{id}/steps/{d}/complete", post(crate::capa::complete_step))
        .route("/capa/cases/{id}/steps/{d}/reopen", post(crate::capa::reopen_step))
        .route("/capa/cases/{id}/actions", post(crate::capa::add_action))
        .route("/capa/actions/{id}/done", post(crate::capa::action_done))
        .route("/capa/cases/{id}/closure/request", post(crate::capa::request_closure))
        .route("/capa/cases/{id}/closure/approve", post(crate::capa::approve_closure))
        .route("/capa/cases/{id}/closure/reject", post(crate::capa::reject_closure))
        .route(
            "/capa/cases/{id}/supplier-requests",
            post(crate::capa::create_supplier_request),
        )
        .route(
            "/capa/supplier-requests/{id}/approve",
            post(crate::capa::approve_supplier_request),
        )
        .route(
            "/capa/supplier-requests/{id}/cancel",
            post(crate::capa::cancel_supplier_request),
        )
        .route(
            "/capa/supplier-requests/{id}/document",
            get(crate::capa::supplier_request_document),
        )
        .route(
            "/capa/cases/{id}/supplier-responses",
            post(crate::capa::attach_supplier_response),
        )
        .route(
            "/capa/supplier-requests/{id}/portal",
            post(crate::capa::publish_to_portal),
        )
        .route("/capa/notifications", get(crate::capa::notifications))
        .route("/capa/notifications/{id}/read", post(crate::capa::mark_read))
        .route("/capa/overdue/run", post(crate::capa::overdue_now))
        .route("/security/summary", get(incidents::summary))
        .route("/security/status", get(crate::ops::status))
        .route("/dashboard/summary", get(insights::dashboard))
        .route("/subjects", get(insights::subjects))
        .route("/reports", post(insights::org_report))
        .route(
            "/integrations/import-settings",
            get(insights::get_settings).put(insights::put_settings),
        )
        .route(
            "/integrations/ai-extraction",
            get(crate::tenant_settings::get_ai).put(crate::tenant_settings::put_ai),
        )
        .route(
            "/settings/timezone",
            get(crate::tenant_settings::get_timezone).put(crate::tenant_settings::put_timezone),
        )
        .route(
            "/settings/entity-resolution",
            get(crate::tenant_settings::get_entity_resolution).put(crate::tenant_settings::put_entity_resolution),
        )
        .route(
            "/security/detection-settings",
            get(crate::tenant_settings::get_detection).put(crate::tenant_settings::put_detection),
        )
        .route("/integrity/run", post(crate::integrity::run))
        .route(
            "/integrity/settings",
            get(crate::integrity::get_settings).put(crate::integrity::put_settings),
        )
        .route("/integrity/signals", get(crate::integrity::list_signals))
        .route("/integrity/signals/{id}", get(crate::integrity::get_signal))
        .route("/integrity/signals/{id}/review", post(crate::integrity::review_signal))
        .route("/integrity/drift", get(crate::integrity::list_drift))
        .route("/integrity/drift/{id}", get(crate::integrity::get_drift))
        .route("/integrity/drift/{id}/review", post(crate::integrity::review_drift))
        .route("/users/{id}/sessions", get(insights::user_sessions))
        .route("/security/incidents", get(incidents::list))
        .route("/security/incidents/{id}", get(incidents::get))
        .route("/security/incidents/{id}/resolve", post(incidents::resolve))
        .route(
            "/security/incidents/{id}/actions/{action}/{verb}",
            post(incidents::decide_action),
        )
        .route("/security/detect", post(incidents::trigger))
        .route("/audit/checkpoints", post(audit::create_checkpoint))
        .route("/audit/checkpoints/verify", get(audit::verify_checkpoints))
        .route("/audit/checkpoints/verify-external", post(audit::verify_external))
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(RequestBodyLimitLayer::new(body_limit));
    let upload_api = Router::new()
        .route("/artifacts", post(artifacts::upload))
        .route("/portal/requests/{id}/uploads", post(crate::portal::upload))
        .layer(DefaultBodyLimit::max(upload_limit))
        .layer(RequestBodyLimitLayer::new(upload_limit));

    let api = upload_api.merge(json_api).layer(axum::middleware::from_fn_with_state(
        st.clone(),
        crate::idempotency::layer,
    ));
    let mut app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(crate::metrics::readyz))
        .nest("/api/v1", api)
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
    // Outermost: classify, admit (bounded queues → 429), statement timeout scope, latency metrics.
    .layer(axum::middleware::from_fn_with_state(
        st.clone(),
        crate::metrics::http_layer,
    ))
    .with_state(st)
}

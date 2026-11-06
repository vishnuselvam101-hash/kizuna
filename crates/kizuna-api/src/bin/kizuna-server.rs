//! KIZUNA Verify API server. Configuration: environment variables (see .env.example).

use kizuna_api::config::Config;
use kizuna_api::{AppState, db, router};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("KIZUNA_LOG").unwrap_or_else(|_| "info".into()))
        .init();
    let cfg = Config::from_env()?;
    // The extraction sandbox's process limit only works for a non-root server (final review L2).
    let root = std::fs::metadata("/proc/self")
        .map(|m| std::os::unix::fs::MetadataExt::uid(&m) == 0)
        .unwrap_or(false);
    if root && std::env::var("KIZUNA_ALLOW_ROOT").as_deref() != Ok("1") {
        return Err(
            "refusing to run as root: use a dedicated user (or KIZUNA_ALLOW_ROOT=1 for local development)".into(),
        );
    }
    let pool = db::connect(cfg.database_url.expose(), cfg.stability.db_pool_size).await?;
    // Background jobs get their own small pool: a job backlog never takes request connections.
    let job_pool = db::connect(cfg.database_url.expose(), cfg.stability.job_pool_size).await?;
    let metrics_bind = kizuna_api::metrics::metrics_bind(|k| std::env::var(k).ok())?;
    db::assert_runtime_role(&pool).await?;
    let bind = cfg.bind;
    let sweep_pool = job_pool.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tick.tick().await;
            if let Err(e) = kizuna_api::incidents::sweep_all(&sweep_pool).await {
                tracing::error!(error = %e, "incident sweep failed");
            }
        }
    });
    let (review_chunk, job_workers, job_timeout) = (
        cfg.review_batch_chunk,
        cfg.stability.job_workers,
        cfg.stability.statement_timeout(kizuna_api::admission::RouteClass::Job),
    );
    let staging_max_age = std::time::Duration::from_secs(cfg.staging_max_age_secs);
    let mut state = AppState::new(pool.clone(), cfg);
    kizuna_api::storage::spawn_staging_sweeper(
        job_pool.clone(),
        std::sync::Arc::clone(&state.store),
        staging_max_age,
        std::time::Duration::from_secs(600),
    );
    state.av = kizuna_api::antivirus::from_lookup(|k| std::env::var(k).ok())?;
    if state.av.configured() {
        match state.av.probe() {
            Ok(v) => tracing::info!(version = %v, "antivirus reachable"),
            Err(e) => tracing::warn!(error = %e, "antivirus configured but unreachable: uploads will be quarantined"),
        }
    } else {
        tracing::warn!("no antivirus configured (KIZUNA_CLAMD_SOCKET unset): static upload scan only");
    }
    state.alerts = std::sync::Arc::new(kizuna_api::alerts::Alerter::new(
        kizuna_api::alerts::AlertConfig::from_env()?,
        None,
    ));
    kizuna_api::capa::spawn_overdue_worker(
        job_pool.clone(),
        std::sync::Arc::clone(&state.alerts),
        std::time::Duration::from_secs(300),
    );
    // Review batches, verification runs, GC and alert delivery run as leased, retry-safe jobs (replaces the 2 s
    // review-batch poller and the in-memory incident-alert cursor).
    kizuna_api::jobs::spawn_workers(
        kizuna_api::jobs::JobCtx {
            pool: job_pool.clone(),
            alerts: std::sync::Arc::clone(&state.alerts),
            metrics: std::sync::Arc::clone(&state.metrics),
            review_chunk,
            statement_timeout_ms: job_timeout,
            lease_secs: kizuna_api::jobs::LEASE_SECS,
        },
        job_workers,
        std::time::Duration::from_millis(500),
    );
    kizuna_api::jobs::spawn_scheduler(
        job_pool.clone(),
        state.alerts.enabled(),
        std::time::Duration::from_secs(30),
    );
    drop(pool);
    if let Some(addr) = metrics_bind {
        let l = tokio::net::TcpListener::bind(addr).await?;
        let mr = kizuna_api::metrics::router(state.clone(), Some(job_pool));
        tracing::info!(%addr, "metrics listening (loopback only)");
        tokio::spawn(async move {
            if let Err(e) = axum::serve(l, mr).await {
                tracing::error!(error = %e, "metrics listener failed");
            }
        });
    }
    let draining = std::sync::Arc::clone(&state.draining);
    let app = router(state);
    let limits = kizuna_api::serve::ServeLimits::from_lookup(|k| std::env::var(k).ok())?;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, header_read_timeout = ?limits.header_read_timeout, max_connections = limits.max_connections,
        "kizuna-server listening");
    // Own accept loop: header-read deadline + connection cap (red-team 2026-11-05 F3); SIGTERM/Ctrl-C stop accepting
    // and drain in-flight requests (chaos C3).
    let shutdown = async move {
        shutdown_signal().await;
        kizuna_api::metrics::set_draining(&draining);
    };
    kizuna_api::serve::serve(listener, app, limits, shutdown).await?;
    Ok(())
}

/// Ctrl-C (interactive) or SIGTERM (systemd stop, container runtimes): stop accepting connections and let in-flight
/// requests finish. Without SIGTERM a service restart cut uploads and extractions off mid-request (chaos C2).
async fn shutdown_signal() {
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "SIGTERM handler unavailable; only Ctrl-C shuts down gracefully");
                std::future::pending::<()>().await;
            }
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        () = term => {}
    }
    tracing::info!("shutdown signal received: draining in-flight requests");
}

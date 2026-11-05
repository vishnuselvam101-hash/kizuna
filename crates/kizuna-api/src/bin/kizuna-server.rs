//! KIZUNA Verify API server. Configuration: environment variables (see .env.example).

use std::net::SocketAddr;

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
    let pool = db::connect(cfg.database_url.expose(), 16).await?;
    db::assert_runtime_role(&pool).await?;
    let bind = cfg.bind;
    let sweep_pool = pool.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tick.tick().await;
            if let Err(e) = kizuna_api::incidents::sweep_all(&sweep_pool).await {
                tracing::error!(error = %e, "incident sweep failed");
            }
        }
    });
    let app = router(AppState::new(pool, cfg));
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "kizuna-server listening");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

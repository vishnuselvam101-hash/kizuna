//! KIZUNA Verify API server. Configuration: environment variables (see .env.example).

use std::net::SocketAddr;

use kizuna_api::config::Config;
use kizuna_api::{AppState, db, router};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("KIZUNA_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cfg = Config::from_env()?;
    let pool = db::connect(cfg.database_url.expose(), 16).await?;
    db::assert_runtime_role(&pool).await?;
    let bind = cfg.bind;
    let app = router(AppState::new(pool, cfg));
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "kizuna-server listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}

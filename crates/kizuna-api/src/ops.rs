//! `GET /api/v1/security/status`: honest operational status of the protective layers the pilot depends on —
//! antivirus (configured? reachable?), extraction sandbox and seccomp, alert channels. No secrets, no paths.

use axum::Json;
use axum::extract::State;
use policy_engine::Permission;
use serde_json::{Value, json};

use crate::AppState;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};

pub async fn status(State(st): State<AppState>, p: Principal) -> ApiResult<Json<Value>> {
    p.require(&st, Permission::SecurityEventRead).await?;
    let av = std::sync::Arc::clone(&st.av);
    let probe = tokio::task::spawn_blocking(move || av.probe())
        .await
        .map_err(ApiError::internal)?;
    let antivirus = match (st.av.configured(), probe) {
        (false, Err(note)) | (false, Ok(note)) => {
            json!({ "configured": false, "status": "not_configured", "note": note })
        }
        (true, Ok(version)) => {
            json!({ "configured": true, "engine": st.av.engine(), "status": "ok", "version": version })
        }
        (true, Err(e)) => json!({
            "configured": true, "engine": st.av.engine(), "status": "unavailable", "detail": e,
            "effect": "new uploads are quarantined until the scanner is reachable"
        }),
    };
    let sandbox = json!({
        "mode": st.cfg.extraction_sandbox,
        "seccomp": match (st.cfg.extraction_sandbox.as_str(), crate::seccomp::program_bytes()) {
            ("bwrap", Ok(_)) => json!({ "status": "enabled", "policy": "deny-list (ADR 0016)", "denied_syscalls": crate::seccomp::denied().len() }),
            ("bwrap", Err(e)) => json!({ "status": "unavailable", "detail": e, "effect": "extraction refused" }),
            _ => json!({ "status": "off", "detail": "development sandbox mode" }),
        },
    });
    Ok(Json(json!({
        "antivirus": antivirus,
        "extraction_sandbox": sandbox,
        "alerts": st.alerts.channels(),
    })))
}

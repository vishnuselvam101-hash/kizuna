#![allow(clippy::unwrap_used)]
//! Regression tests for docs/security/REVIEW_DEEP_2026-11-05.md (KZ-A1, KZ-A3, KZ-C3, KZ-I1). KZ-C1 is in
//! m5_defense.rs, KZ-S1/S2/S3 in extraction.rs (runtime sandbox probe), KZ-C2/C4/C5/I2 in unit tests and the
//! supply-chain items in scripts/tests/supply-chain.test.sh.
mod common;

use std::sync::{Arc, Mutex};

use axum::http::{Method, StatusCode};
use common::*;
use kizuna_api::clock::OffsetClock;
use kizuna_api::router;
use serde_json::{Value, json};
use time::Duration;

async fn clocked() -> (Ctx, Arc<OffsetClock>) {
    let mut c = ctx().await;
    let clock = Arc::new(OffsetClock::default());
    c.st.clock = clock.clone();
    c.app = router(c.st.clone());
    (c, clock)
}

async fn refresh(c: &Ctx, rt: &str) -> (StatusCode, Value) {
    let r = c
        .post("/api/v1/auth/refresh", None, json!({ "refresh_token": rt }))
        .await;
    (r.status, r.body)
}

#[tokio::test]
async fn session_idle_timeout_follows_the_clock() {
    let (c, clock) = clocked().await;
    let (t, slug) = c.tenant().await;
    let (uid, email) = c.user(t, "viewer").await;
    let r = c.login(&slug, &email, PW, None).await;
    let mut rt = r.body["refresh_token"].as_str().unwrap().to_owned();
    // Activity keeps the session alive past the idle window.
    for _ in 0..2 {
        clock.advance(Duration::minutes(20));
        let (s, b) = refresh(&c, &rt).await;
        assert_eq!(s, StatusCode::OK, "{b:?}");
        rt = b["refresh_token"].as_str().unwrap().to_owned();
    }
    clock.advance(Duration::minutes(31));
    let (s, b) = refresh(&c, &rt).await;
    assert_eq!(
        (s, b["error"].clone()),
        (StatusCode::UNAUTHORIZED, json!("session_idle"))
    );
    let mut tx = kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap();
    let reason: Option<String> =
        sqlx::query_scalar("SELECT revoked_reason FROM sessions WHERE user_id = $1 ORDER BY created_at DESC LIMIT 1")
            .bind(uid)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(reason.as_deref(), Some("idle_timeout"));
}

#[tokio::test]
async fn privileged_ops_need_fresh_auth() {
    let (c, clock) = clocked().await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "tenant_admin").await;
    let tok = c.token(&slug, &email).await;
    let secret = c.enroll_totp(&tok).await;
    let now = (time::OffsetDateTime::now_utc() + Duration::seconds(0)).unix_timestamp();
    let r = c
        .login(&slug, &email, PW, Some(kizuna_api::crypto::totp_now(&secret, now + 30)))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let (acc, mut rt) = (
        r.body["access_token"].as_str().unwrap().to_owned(),
        r.body["refresh_token"].as_str().unwrap().to_owned(),
    );
    let put = |tok: String| {
        let c = &c;
        async move {
            c.call(
                Method::PUT,
                "/api/v1/security/detection-settings",
                Some(&tok),
                Some(json!({ "expected_version": 0, "settings": {} })),
            )
            .await
        }
    };
    // A non-privileged read works; a privileged write right after login works.
    assert_eq!(c.get("/api/v1/auth/me", &acc).await.status, StatusCode::OK);
    clock.advance(Duration::minutes(10));
    let (s, b) = refresh(&c, &rt).await;
    assert_eq!(s, StatusCode::OK);
    rt = b["refresh_token"].as_str().unwrap().to_owned();
    clock.advance(Duration::minutes(10));
    let (s, b) = refresh(&c, &rt).await;
    assert_eq!(s, StatusCode::OK);
    let acc = b["access_token"].as_str().unwrap().to_owned();
    // 20 min after login: still a valid session, but too old for privileged operations.
    assert_eq!(c.get("/api/v1/auth/me", &acc).await.status, StatusCode::OK);
    let r = put(acc).await;
    assert_eq!(
        (r.status, r.body["error"].clone()),
        (StatusCode::UNAUTHORIZED, json!("reauth_required"))
    );
}

#[tokio::test]
async fn new_checkpoints_are_exported_off_host() {
    use axum::routing::post;
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let s2 = seen.clone();
    let app = axum::Router::new().route(
        "/{bot}/sendMessage",
        post(move |body: String| {
            let s2 = s2.clone();
            async move {
                s2.lock().unwrap().push(body);
                "{\"ok\":true}"
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let mut c = ctx().await;
    let cfg = kizuna_api::alerts::AlertConfig::from_lookup(|k| match k {
        "KIZUNA_ALERT_TELEGRAM_BOT_TOKEN" => Some("123:abc".into()),
        "KIZUNA_ALERT_TELEGRAM_CHAT_ID" => Some("42".into()),
        "KIZUNA_ALERT_TELEGRAM_API_BASE" => Some(format!("http://{addr}")),
        _ => None,
    })
    .unwrap();
    c.st.alerts = Arc::new(kizuna_api::alerts::Alerter::new(cfg, None));
    c.app = router(c.st.clone());
    let (_, _, _, admin) = c.admin().await;
    let cp = c.post("/api/v1/audit/checkpoints", Some(&admin), json!({})).await;
    assert_eq!(cp.status, StatusCode::OK, "{:?}", cp.body);
    let got = seen.lock().unwrap().join("\n");
    assert!(got.contains(cp.body["signature"].as_str().unwrap()), "{got}");
    assert!(got.contains(cp.body["head_hash"].as_str().unwrap()));
}

#[tokio::test]
async fn runtime_role_cannot_update_audit_heads() {
    let c = ctx().await;
    let (t, _, _, admin) = c.admin().await;
    let rt = runtime_pool().await;
    let mut tx = kizuna_api::db::tenant_tx(&rt, t).await.unwrap();
    let e = sqlx::query("UPDATE audit_heads SET last_seq = 0 WHERE tenant_id = $1")
        .bind(t)
        .execute(&mut *tx)
        .await;
    assert!(e.unwrap_err().to_string().contains("permission denied"));
    drop(tx);
    // Audited writes still advance the head (database trigger), and the chain verifies.
    c.call(
        Method::PUT,
        "/api/v1/integrations/import-settings",
        Some(&admin),
        Some(json!({ "expected_version": 0, "settings": { "default_kind": "drawing" } })),
    )
    .await;
    let v = c.get("/api/v1/audit/verify", &admin).await;
    assert_eq!(v.body["intact"], true, "{:?}", v.body);
    assert!(v.body["head_seq"].as_i64().unwrap() > 1);
}

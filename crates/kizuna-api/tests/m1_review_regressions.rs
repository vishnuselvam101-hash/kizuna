#![allow(clippy::unwrap_used)]
//! Regression tests for findings of the M1 independent security review (BUILD_STATUS.md, "Security review M1").
mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::*;
use serde_json::json;

#[tokio::test]
async fn regression_m1r1_spoofed_xff_does_not_bypass_ip_limit() {
    let c = ctx_with(|cfg| {
        cfg.trusted_proxy_hops = 1;
        cfg.rate.login_per_ip_per_min = 3;
    })
    .await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "viewer").await;
    let mut statuses = Vec::new();
    for i in 0..6 {
        let body = json!({ "tenant": slug, "email": email, "password": "wrong password here" }).to_string();
        let req = Request::post("/api/v1/auth/login")
            .header("content-type", "application/json")
            .header("x-forwarded-for", format!("198.51.100.{i}, 203.0.113.7"))
            .body(Body::from(body))
            .unwrap();
        statuses.push(c.raw(req).await.status);
    }
    assert_eq!(statuses[3], StatusCode::TOO_MANY_REQUESTS, "{statuses:?}");
    let ips: Vec<Option<String>> = sqlx::query_scalar("SELECT client_ip FROM security_events WHERE tenant_id = $1")
        .bind(t)
        .fetch_all(&c.owner)
        .await
        .unwrap();
    assert!(ips.iter().all(|i| i.as_deref() == Some("203.0.113.7")), "{ips:?}");
}

#[tokio::test]
async fn regression_m1r4_password_change_is_not_an_unlimited_oracle() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "viewer").await;
    let tok = c.token(&slug, &email).await;
    for _ in 0..5 {
        let r = c
            .post(
                "/api/v1/auth/password",
                Some(&tok),
                json!({"current_password": "wrong guess here", "new_password": "a new long passphrase"}),
            )
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        c.get("/api/v1/auth/me", &tok).await.status,
        StatusCode::UNAUTHORIZED,
        "sessions revoked on lock"
    );
    assert_eq!(
        c.login(&slug, &email, PW, None).await.status,
        StatusCode::UNAUTHORIZED,
        "account locked"
    );
}

#[tokio::test]
async fn regression_m1r5_verify_has_no_false_alarm_under_concurrent_appends() {
    let c = ctx().await;
    let (t, slug, _, tok) = c.admin().await;
    let (_, email) = c.user(t, "viewer").await;
    let logins = async {
        for _ in 0..15 {
            c.login(&slug, &email, PW, None).await;
        }
    };
    let verifies = async {
        let mut bad = 0;
        for _ in 0..15 {
            if c.get("/api/v1/audit/verify", &tok).await.body["intact"] != true {
                bad += 1;
            }
        }
        bad
    };
    let (_, bad) = tokio::join!(logins, verifies);
    assert_eq!(bad, 0);
    assert!(
        !c.security_kinds(t)
            .await
            .contains(&"integrity.audit_chain_broken".to_string())
    );
}

#[tokio::test]
async fn regression_m1r7_no_tenant_enumeration_via_rate_limit() {
    let c = ctx_with(|cfg| cfg.rate.login_per_account_per_min = 3).await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "viewer").await;
    let mut known = Vec::new();
    let mut unknown = Vec::new();
    for _ in 0..5 {
        known.push(c.login(&slug, &email, "wrong password here", None).await.status);
        unknown.push(
            c.login("no-such-tenant-x", &email, "wrong password here", None)
                .await
                .status,
        );
    }
    assert_eq!(known, unknown);
}

#[tokio::test]
async fn regression_m1r8_admin_can_unlock_locked_account() {
    let c = ctx().await;
    let (t, _, _, tok) = c.admin().await;
    let slug: String = sqlx::query_scalar("SELECT slug FROM tenants WHERE id = $1")
        .bind(t)
        .fetch_one(&c.owner)
        .await
        .unwrap();
    let (uid, email) = c.user(t, "viewer").await;
    for _ in 0..5 {
        c.login(&slug, &email, "wrong password here", None).await;
    }
    assert_eq!(c.login(&slug, &email, PW, None).await.status, StatusCode::UNAUTHORIZED);
    let r = c
        .call(
            Method::PATCH,
            &format!("/api/v1/users/{uid}"),
            Some(&tok),
            Some(json!({"status": "active"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(c.login(&slug, &email, PW, None).await.status, StatusCode::OK);
}

#[tokio::test]
async fn regression_m1r3_mfa_setup_requires_password() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "tenant_admin").await;
    let tok = c.token(&slug, &email).await;
    let r = c
        .post(
            "/api/v1/auth/mfa/totp/setup",
            Some(&tok),
            json!({ "password": "not my password!" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = c.post("/api/v1/auth/mfa/totp/setup", Some(&tok), json!({})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

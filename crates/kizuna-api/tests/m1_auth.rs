#![allow(clippy::unwrap_used)]
mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::*;
use serde_json::json;

#[tokio::test]
async fn health_and_security_headers() {
    let c = ctx().await;
    let r = c
        .raw(Request::get("/healthz").body(Body::empty()).unwrap())
        .await;
    assert_eq!(r.status, StatusCode::OK);
    for h in [
        "x-content-type-options",
        "x-frame-options",
        "content-security-policy",
        "referrer-policy",
        "cache-control",
        "strict-transport-security",
    ] {
        assert!(r.headers.contains_key(h), "missing {h}");
    }
    assert!(
        !r.headers.contains_key("access-control-allow-origin"),
        "no CORS by default"
    );
}

#[tokio::test]
async fn login_me_and_generic_failures() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (uid, email) = c.user(t, "analyst").await;
    let tok = c.token(&slug, &email).await;
    let me = c.get("/api/v1/auth/me", &tok).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.body["user_id"], uid.to_string());
    assert_eq!(me.body["role"], "analyst");

    let bad_pw = c.login(&slug, &email, "wrong password here", None).await;
    let no_user = c.login(&slug, "nobody@example.test", PW, None).await;
    let no_tenant = c.login("no-such-tenant", &email, PW, None).await;
    for r in [&bad_pw, &no_user, &no_tenant] {
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            r.body,
            json!({ "error": "invalid_credentials" }),
            "no account enumeration"
        );
    }
    assert!(
        c.security_kinds(t)
            .await
            .contains(&"auth.login_failed".to_string())
    );
    // Email lookup is case-insensitive.
    assert_eq!(
        c.login(&slug, &email.to_uppercase(), PW, None).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn tokens_are_stored_hashed() {
    use base64::Engine;
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "viewer").await;
    let r = c.login(&slug, &email, PW, None).await;
    let raw = |k: &str| {
        let v = r.body[k].as_str().unwrap();
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(v.rsplit('.').next().unwrap())
            .unwrap()
    };
    let (acc, rt) = (raw("access_token"), raw("refresh_token"));
    let mut tx = kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap();
    let stored: Vec<u8> = sqlx::query_scalar("SELECT access_hash FROM sessions")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(stored, kizuna_api::crypto::sha256(&acc).to_vec());
    assert_ne!(stored, acc);
    let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM refresh_tokens")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(stored, kizuna_api::crypto::sha256(&rt).to_vec());
}

#[tokio::test]
async fn invalid_and_malformed_tokens_rejected() {
    let c = ctx().await;
    for t in [
        "",
        "garbage",
        "kza.x.y",
        "kzr.00000000000000000000000000000000.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ] {
        let r = c.get("/api/v1/auth/me", t).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{t}");
    }
    let r = c.call(Method::GET, "/api/v1/auth/me", None, None).await;
    assert_eq!(r.body["error"], "missing_token");
}

#[tokio::test]
async fn lockout_after_repeated_failures() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "viewer").await;
    for _ in 0..5 {
        assert_eq!(
            c.login(&slug, &email, "wrong password here", None)
                .await
                .status,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        c.login(&slug, &email, PW, None).await.status,
        StatusCode::UNAUTHORIZED,
        "locked even with right password"
    );
    assert!(
        c.security_kinds(t)
            .await
            .contains(&"auth.account_locked".to_string())
    );
}

#[tokio::test]
async fn login_rate_limited_per_ip() {
    let c = ctx_with(|cfg| cfg.rate.login_per_ip_per_min = 3).await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "viewer").await;
    for _ in 0..3 {
        c.login(&slug, &email, "wrong password here", None).await;
    }
    let r = c.login(&slug, &email, PW, None).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn refresh_rotates_and_reuse_revokes_family() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "viewer").await;
    let l = c.login(&slug, &email, PW, None).await;
    let (a1, r1) = (
        l.body["access_token"].as_str().unwrap().to_owned(),
        l.body["refresh_token"].as_str().unwrap().to_owned(),
    );

    let n = c
        .post("/api/v1/auth/refresh", None, json!({ "refresh_token": r1 }))
        .await;
    assert_eq!(n.status, StatusCode::OK);
    let (a2, r2) = (
        n.body["access_token"].as_str().unwrap().to_owned(),
        n.body["refresh_token"].as_str().unwrap().to_owned(),
    );
    assert_ne!(r1, r2);
    assert_eq!(
        c.get("/api/v1/auth/me", &a1).await.status,
        StatusCode::UNAUTHORIZED,
        "old access token rotated out"
    );
    assert_eq!(c.get("/api/v1/auth/me", &a2).await.status, StatusCode::OK);

    // Replay of the used refresh token: theft assumed, family revoked.
    let replay = c
        .post("/api/v1/auth/refresh", None, json!({ "refresh_token": r1 }))
        .await;
    assert_eq!(replay.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        c.get("/api/v1/auth/me", &a2).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        c.post("/api/v1/auth/refresh", None, json!({ "refresh_token": r2 }))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert!(
        c.security_kinds(t)
            .await
            .contains(&"auth.refresh_token_reuse".to_string())
    );
}

#[tokio::test]
async fn refresh_token_with_forged_tenant_prefix_fails() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (t2, _) = c.tenant().await;
    let (_, email) = c.user(t, "viewer").await;
    let l = c.login(&slug, &email, PW, None).await;
    let r = l.body["refresh_token"].as_str().unwrap();
    let forged = r.replacen(&t.simple().to_string(), &t2.simple().to_string(), 1);
    assert_eq!(
        c.post(
            "/api/v1/auth/refresh",
            None,
            json!({ "refresh_token": forged })
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
    let a = l.body["access_token"].as_str().unwrap().replacen(
        &t.simple().to_string(),
        &t2.simple().to_string(),
        1,
    );
    assert_eq!(
        c.get("/api/v1/auth/me", &a).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn logout_revokes_server_side() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "viewer").await;
    let tok = c.token(&slug, &email).await;
    assert_eq!(
        c.post("/api/v1/auth/logout", Some(&tok), json!({}))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        c.get("/api/v1/auth/me", &tok).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn totp_mfa_flow_and_replay() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (uid, email) = c.user(t, "tenant_admin").await;
    let tok = c.token(&slug, &email).await;
    // Admin without MFA cannot use privileged permissions.
    let r = c.get("/api/v1/users", &tok).await;
    assert_eq!(r.status, StatusCode::OK, "UserRead is not privileged");
    let r = c
        .post(
            "/api/v1/users",
            Some(&tok),
            json!({"email":"x@example.test","display_name":"X","role":"viewer","password":PW}),
        )
        .await;
    assert_eq!(
        (r.status, r.body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("mfa_required"))
    );

    let secret = c.enroll_totp(&tok).await;
    let r = c
        .post(
            "/api/v1/users",
            Some(&tok),
            json!({"email":"x@example.test","display_name":"X","role":"viewer","password":PW}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);

    assert_eq!(
        c.login(&slug, &email, PW, None).await.body["error"],
        "mfa_required"
    );
    assert_eq!(
        c.login(&slug, &email, PW, Some("000000".into()))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    let code = kizuna_api::crypto::totp_now(&secret, now() + 30);
    let ok = c.login(&slug, &email, PW, Some(code.clone())).await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.body);
    assert_eq!(ok.body["mfa_verified"], true);
    assert_eq!(
        c.login(&slug, &email, PW, Some(code)).await.status,
        StatusCode::UNAUTHORIZED,
        "TOTP replay rejected"
    );

    // Secret is encrypted at rest (not the raw bytes).
    let enc: Vec<u8> = sqlx::query_scalar("SELECT totp_secret_enc FROM users WHERE id = $1")
        .bind(uid)
        .fetch_one(&mut *kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap())
        .await
        .unwrap();
    assert!(!enc.windows(secret.len()).any(|w| w == secret.as_slice()));
}

#[tokio::test]
async fn password_change_policy_and_revocation() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "viewer").await;
    let s1 = c.token(&slug, &email).await;
    let s2 = c.token(&slug, &email).await;
    let r = c
        .post(
            "/api/v1/auth/password",
            Some(&s1),
            json!({"current_password": PW, "new_password": "short"}),
        )
        .await;
    assert_eq!(r.body["error"], "password_policy");
    let r = c.post("/api/v1/auth/password", Some(&s1), json!({"current_password": "wrong password here", "new_password": "another long passphrase"})).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = c
        .post(
            "/api/v1/auth/password",
            Some(&s1),
            json!({"current_password": PW, "new_password": "another long passphrase"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(c.get("/api/v1/auth/me", &s1).await.status, StatusCode::OK);
    assert_eq!(
        c.get("/api/v1/auth/me", &s2).await.status,
        StatusCode::UNAUTHORIZED,
        "other sessions revoked"
    );
    assert_eq!(
        c.login(&slug, &email, "another long passphrase", None)
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn body_limit_and_bad_json() {
    let c = ctx().await;
    let big = "x".repeat(200 * 1024);
    let r = c
        .post(
            "/api/v1/auth/login",
            None,
            json!({ "tenant": "a", "email": "b", "password": big }),
        )
        .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    let req = Request::post("/api/v1/auth/login")
        .header("content-type", "application/json")
        .body(Body::from("{not json"))
        .unwrap();
    let r = c.raw(req).await;
    assert_eq!(
        (r.status, r.body["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_json"))
    );
}

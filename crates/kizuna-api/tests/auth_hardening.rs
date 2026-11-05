#![allow(clippy::unwrap_used)]
//! Auth hardening: WebAuthn for privileged accounts (+ recovery), password screening, data-key rotation and
//! clock-driven expiry. Real PostgreSQL; time is moved with `OffsetClock`, never by sleeping.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use common::*;
use kizuna_api::clock::OffsetClock;
use kizuna_api::keyring::{self, DataKeys};
use kizuna_api::{db, router};
use serde_json::{Value, json};
use time::Duration;
use uuid::Uuid;
use webauthn_authenticator_rs::WebauthnAuthenticator;
use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse, Url};

/// A context whose clock the test controls (and optionally fixed data keys).
async fn clocked(keys: Option<DataKeys>) -> (Ctx, Arc<OffsetClock>) {
    let mut c = ctx_with(|cfg| {
        if let Some(k) = keys {
            cfg.data_keys = k;
        }
    })
    .await;
    let clock = Arc::new(OffsetClock::default());
    c.st.clock = clock.clone();
    c.app = router(c.st.clone());
    (c, clock)
}

fn origin() -> Url {
    Url::parse("http://localhost:8080").unwrap()
}

/// Enrols a soft authenticator for the session's user; returns the finish response body.
async fn enroll_key(c: &Ctx, token: &str, key: &mut SoftPasskey) -> Value {
    let r = c
        .post(
            "/api/v1/auth/mfa/webauthn/register/start",
            Some(token),
            json!({ "password": PW }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let ccr: CreationChallengeResponse = serde_json::from_value(r.body["public_key"].clone()).unwrap();
    let cred = key.do_registration(origin(), ccr).unwrap();
    let r = c
        .post(
            "/api/v1/auth/mfa/webauthn/register/finish",
            Some(token),
            json!({ "challenge_id": r.body["challenge_id"], "credential": cred, "label": "yubikey" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body
}

async fn wa_start(c: &Ctx, slug: &str, email: &str, pw: &str) -> common::Resp {
    c.post(
        "/api/v1/auth/webauthn/login/start",
        None,
        json!({ "tenant": slug, "email": email, "password": pw }),
    )
    .await
}

async fn wa_finish(c: &Ctx, slug: &str, start: &Value, key: &mut SoftPasskey) -> common::Resp {
    let rcr: RequestChallengeResponse = serde_json::from_value(start["public_key"].clone()).unwrap();
    let cred = key.do_authentication(origin(), rcr).unwrap();
    c.post(
        "/api/v1/auth/webauthn/login/finish",
        None,
        json!({ "tenant": slug, "challenge_id": start["challenge_id"], "credential": cred }),
    )
    .await
}

/// Full WebAuthn login; returns the access token.
async fn wa_login(c: &Ctx, slug: &str, email: &str, key: &mut SoftPasskey) -> String {
    let s = wa_start(c, slug, email, PW).await;
    assert_eq!(s.status, StatusCode::OK, "{:?}", s.body);
    let r = wa_finish(c, slug, &s.body, key).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["mfa_verified"], true);
    r.body["access_token"].as_str().unwrap().to_owned()
}

async fn admin_email(c: &Ctx, uid: Uuid, t: Uuid) -> String {
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    let e = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(uid)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    e
}

/// A privileged action (needs an MFA-verified admin session).
async fn privileged(c: &Ctx, t: Uuid, token: &str) -> StatusCode {
    let (victim, _) = c.user(t, "viewer").await;
    c.post(
        &format!("/api/v1/users/{victim}/revoke-sessions"),
        Some(token),
        json!({}),
    )
    .await
    .status
}

#[tokio::test]
async fn webauthn_enrolment_makes_key_mandatory_for_admin() {
    let (c, _clock) = clocked(None).await;
    let (t, slug, uid, tok) = c.admin().await;
    let email = admin_email(&c, uid, t).await;
    let mut key = SoftPasskey::new(true);

    let bad = c
        .post(
            "/api/v1/auth/mfa/webauthn/register/start",
            Some(&tok),
            json!({ "password": "wrong password here" }),
        )
        .await;
    assert_eq!(bad.status, StatusCode::UNAUTHORIZED, "enrolment needs the password");
    let body = enroll_key(&c, &tok, &mut key).await;
    assert_eq!(body["recovery_codes"].as_array().unwrap().len(), 10);
    assert_eq!(
        c.get("/api/v1/auth/me", &tok).await.status,
        StatusCode::UNAUTHORIZED,
        "pre-enrolment sessions are revoked"
    );

    // Password + TOTP is no longer enough for this account.
    let r = c.login(&slug, &email, PW, Some(totp_for(&c, t, uid, 60).await)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.body["error"], "webauthn_required", "{:?}", r.body);

    let s = wa_start(&c, &slug, &email, "wrong password here").await;
    assert_eq!(s.status, StatusCode::UNAUTHORIZED, "password is still the first factor");
    let tok2 = wa_login(&c, &slug, &email, &mut key).await;
    assert_eq!(privileged(&c, t, &tok2).await, StatusCode::OK);
    let me = c.get("/api/v1/auth/me", &tok2).await;
    assert_eq!(me.body["session_mfa_verified"], true);
    assert!(c.security_kinds(t).await.iter().any(|k| k == "auth.mfa_enrolled"));

    // Second key: no new recovery codes; exclusion list prevents re-registering the same authenticator.
    let mut key2 = SoftPasskey::new(true);
    let body = enroll_key(&c, &tok2, &mut key2).await;
    assert!(body["recovery_codes"].is_null());
    let tok3 = wa_login(&c, &slug, &email, &mut key2).await;
    assert_eq!(privileged(&c, t, &tok3).await, StatusCode::OK);
}

/// Current TOTP code for an enrolled user with the clock `ahead` seconds forward (reads the seed via the owner).
async fn totp_for(c: &Ctx, t: Uuid, uid: Uuid, ahead: i64) -> String {
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    let enc: Vec<u8> = sqlx::query_scalar("SELECT totp_secret_enc FROM users WHERE id = $1")
        .bind(uid)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let seed =
        c.st.cfg
            .data_keys
            .open(&enc, &kizuna_api::crypto::totp_aad(t, uid))
            .unwrap();
    kizuna_api::crypto::totp_now(&seed, c.st.clock.now().unix_timestamp() + ahead)
}

#[tokio::test]
async fn webauthn_assertion_error_paths() {
    let (c, clock) = clocked(None).await;
    let (t, slug, uid, tok) = c.admin().await;
    let email = admin_email(&c, uid, t).await;
    let mut key = SoftPasskey::new(true);
    enroll_key(&c, &tok, &mut key).await;

    // Replay: a challenge is single use.
    let s = wa_start(&c, &slug, &email, PW).await;
    let rcr: RequestChallengeResponse = serde_json::from_value(s.body["public_key"].clone()).unwrap();
    let cred = key.do_authentication(origin(), rcr).unwrap();
    let finish = json!({ "tenant": slug, "challenge_id": s.body["challenge_id"], "credential": cred });
    let ok = c.post("/api/v1/auth/webauthn/login/finish", None, finish.clone()).await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.body);
    let replay = c.post("/api/v1/auth/webauthn/login/finish", None, finish).await;
    assert_eq!(replay.status, StatusCode::UNAUTHORIZED, "replayed challenge");

    // Forged assertion (signature tampered): rejected, challenge burnt, counted as a failed login.
    let s = wa_start(&c, &slug, &email, PW).await;
    let rcr: RequestChallengeResponse = serde_json::from_value(s.body["public_key"].clone()).unwrap();
    let mut cred = serde_json::to_value(key.do_authentication(origin(), rcr).unwrap()).unwrap();
    let sig = cred["response"]["signature"].as_str().unwrap().to_owned();
    let flipped = if sig.starts_with('A') { "B" } else { "A" };
    cred["response"]["signature"] = Value::String(format!("{flipped}{}", &sig[1..]));
    let forged = json!({ "tenant": slug, "challenge_id": s.body["challenge_id"], "credential": cred });
    let r = c.post("/api/v1/auth/webauthn/login/finish", None, forged.clone()).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{:?}", r.body);
    let failed: i32 = {
        let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
        let n = sqlx::query_scalar("SELECT failed_logins FROM users WHERE id = $1")
            .bind(uid)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        n
    };
    assert_eq!(failed, 1, "bad assertion counts toward lockout");

    // Expired challenge (5 min TTL), via the injected clock.
    let s = wa_start(&c, &slug, &email, PW).await;
    clock.advance(Duration::seconds(301));
    let r = wa_finish(&c, &slug, &s.body, &mut key).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "expired challenge");

    // Tenant isolation: a challenge cannot be redeemed under another tenant.
    let (_t2, slug2) = c.tenant().await;
    let s = wa_start(&c, &slug, &email, PW).await;
    let r = wa_finish(&c, &slug2, &s.body, &mut key).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "cross-tenant challenge");
    let mut tx = db::tenant_tx(&c.st.pool, _t2).await.unwrap();
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM webauthn_credentials")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(visible, 0, "RLS hides other tenants' credentials");

    // Account without a key cannot start a WebAuthn login; registration finish rejects bogus input.
    let (_, viewer) = c.user(t, "viewer").await;
    assert_eq!(wa_start(&c, &slug, &viewer, PW).await.status, StatusCode::CONFLICT);
    let vtok = c.token(&slug, &viewer).await;
    let r = c
        .post(
            "/api/v1/auth/mfa/webauthn/register/finish",
            Some(&vtok),
            json!({ "challenge_id": Uuid::new_v4(), "credential": cred_placeholder() }),
        )
        .await;
    assert!(
        r.status == StatusCode::BAD_REQUEST || r.status == StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        r.status
    );
    // Another user's registration challenge is not usable.
    let admin_tok = wa_login(&c, &slug, &email, &mut key).await;
    let st = c
        .post(
            "/api/v1/auth/mfa/webauthn/register/start",
            Some(&admin_tok),
            json!({ "password": PW }),
        )
        .await;
    let ccr: CreationChallengeResponse = serde_json::from_value(st.body["public_key"].clone()).unwrap();
    let cred = SoftPasskey::new(true).do_registration(origin(), ccr).unwrap();
    let r = c
        .post(
            "/api/v1/auth/mfa/webauthn/register/finish",
            Some(&vtok),
            json!({ "challenge_id": st.body["challenge_id"], "credential": cred, "label": "x" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.body["error"], "invalid_challenge", "{:?}", r.body);
    let r = c
        .post(
            "/api/v1/auth/mfa/webauthn/register/finish",
            Some(&admin_tok),
            json!({ "challenge_id": st.body["challenge_id"], "credential": cred, "label": "" }),
        )
        .await;
    assert_eq!(r.body["error"], "invalid_label");
}

fn cred_placeholder() -> Value {
    json!({ "id": "AA", "rawId": "AA", "type": "public-key",
            "response": { "attestationObject": "AA", "clientDataJSON": "AA" }, "extensions": {} })
}

#[tokio::test]
async fn lost_key_recovery_never_locks_out_admins() {
    let (c, clock) = clocked(None).await;
    let (t, slug, uid, tok) = c.admin().await;
    let email = admin_email(&c, uid, t).await;
    let mut key = SoftPasskey::new(true);
    let codes: Vec<String> =
        serde_json::from_value(enroll_key(&c, &tok, &mut key).await["recovery_codes"].clone()).unwrap();
    let recover = |code: String, pw: &'static str| json!({ "tenant": slug, "email": email, "password": pw, "recovery_code": code });

    // Wrong code / wrong password: rejected.
    let r = c
        .post(
            "/api/v1/auth/webauthn/recover",
            None,
            recover("AAAAAA-BBBBBB".into(), PW),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = c
        .post(
            "/api/v1/auth/webauthn/recover",
            None,
            recover(codes[0].clone(), "wrong password here"),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    // Sole admin, key lost: a recovery code (formatting-insensitive) restores access, without MFA privilege.
    let r = c
        .post(
            "/api/v1/auth/webauthn/recover",
            None,
            recover(codes[0].to_lowercase().replace('-', " "), PW),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["mfa_verified"], false);
    let rtok = r.body["access_token"].as_str().unwrap().to_owned();
    assert_eq!(
        privileged(&c, t, &rtok).await,
        StatusCode::FORBIDDEN,
        "recovery session is not MFA"
    );
    let r = c
        .post("/api/v1/auth/webauthn/recover", None, recover(codes[1].clone(), PW))
        .await;
    assert_eq!(
        r.status,
        StatusCode::CONFLICT,
        "credentials and remaining codes were cleared"
    );
    assert!(c.security_kinds(t).await.iter().any(|k| k == "auth.mfa_reset"));

    // TOTP (still enrolled) works again: full MFA admin session.
    clock.advance(Duration::seconds(60));
    let r = c.login(&slug, &email, PW, Some(totp_for(&c, t, uid, 0).await)).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let mfa_tok = r.body["access_token"].as_str().unwrap().to_owned();
    assert_eq!(privileged(&c, t, &mfa_tok).await, StatusCode::OK);

    // Peer path: a second admin enrols a key; the first resets it (clears credentials); self-reset is refused.
    let (uid2, email2) = c.user(t, "tenant_admin").await;
    let tok2 = c.token(&slug, &email2).await;
    let mut key2 = SoftPasskey::new(true);
    enroll_key(&c, &tok2, &mut key2).await;
    let r = c
        .post(&format!("/api/v1/users/{uid}/reset-mfa"), Some(&mfa_tok), json!({}))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "self reset refused");
    let r = c
        .post(&format!("/api/v1/users/{uid2}/reset-mfa"), Some(&mfa_tok), json!({}))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let r = c.login(&slug, &email2, PW, None).await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "after peer reset the admin can sign in and re-enrol"
    );
}

#[tokio::test]
async fn common_and_context_passwords_are_refused() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let r = c
        .post(
            "/api/v1/users",
            Some(&admin),
            json!({ "email": "new.person@example.test", "display_name": "N", "role": "viewer", "password": (["0987654321", "qwerty"].concat()) }),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.body["error"], "password_common", "{:?}", r.body);
    let r = c
        .post(
            "/api/v1/users",
            Some(&admin),
            json!({ "email": "new.person@example.test", "display_name": "N", "role": "viewer", "password": "i am new.person, ok?" }),
        )
        .await;
    assert_eq!(r.body["error"], "password_contains_context", "{:?}", r.body);

    let (_, email) = c.user(t, "viewer").await;
    let tok = c.token(&slug, &email).await;
    let r = c
        .post(
            "/api/v1/auth/password",
            Some(&tok),
            json!({ "current_password": PW, "new_password": "QWERTYUIOPASDFGHJKL" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.body["error"], "password_common", "{:?}", r.body);
    // Rejected before re-authentication: no failure counted, old password still valid.
    let r = c.login(&slug, &email, PW, None).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn data_key_rotation_reencrypts_per_tenant() {
    let (k1, k2) = ([11u8; 32], [22u8; 32]);
    let (c1, _) = clocked(Some(DataKeys::new(1, k1))).await;
    let (ta, slug_a, ua, _) = c1.admin().await;
    let (tb, slug_b, ub, _) = c1.admin().await;
    let email_a = admin_email(&c1, ua, ta).await;
    let email_b = admin_email(&c1, ub, tb).await;
    let (_, ukey_email) = c1.user(ta, "tenant_admin").await;
    let mut key = SoftPasskey::new(true);
    let tok = c1.token(&slug_a, &ukey_email).await;
    enroll_key(&c1, &tok, &mut key).await;
    // Pre-versioning (M1) format for the admin's TOTP seed: bare nonce||ct under key 1.
    let mut tx = db::tenant_tx(&c1.owner, ta).await.unwrap();
    let enc: Vec<u8> = sqlx::query_scalar("SELECT totp_secret_enc FROM users WHERE id = $1")
        .bind(ua)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let aad = kizuna_api::crypto::totp_aad(ta, ua);
    let seed = c1.st.cfg.data_keys.open(&enc, &aad).unwrap();
    sqlx::query("UPDATE users SET totp_secret_enc = $2 WHERE id = $1")
        .bind(ua)
        .bind(kizuna_api::crypto::seal(&k1, &seed, &aad).unwrap())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    // A login ceremony in flight across the rotation (state sealed under key 1).
    let pending = wa_start(&c1, &slug_a, &ukey_email, PW).await;
    assert_eq!(pending.status, StatusCode::OK);

    // New current key 2, key 1 still readable: everything keeps working before rotation.
    let (c2, clock2) = clocked(Some(DataKeys::new(2, k2).with_previous(1, k1))).await;
    clock2.advance(Duration::seconds(60));
    let r = c2
        .login(&slug_a, &email_a, PW, Some(totp_for(&c2, ta, ua, 0).await))
        .await;
    assert_eq!(r.status, StatusCode::OK, "old key readable: {:?}", r.body);

    let rotate = |t: Uuid| {
        let owner = c2.owner.clone();
        let keys = c2.st.cfg.data_keys.clone();
        let now = c2.st.clock.now();
        async move {
            let mut tx = db::tenant_tx(&owner, t).await.unwrap();
            let s = keyring::reencrypt_tenant(&mut tx, &keys, t, now).await.unwrap();
            let stale = keyring::stale_count(&mut tx, &keys).await.unwrap();
            tx.commit().await.unwrap();
            (s, stale)
        }
    };
    let (s, stale) = rotate(ta).await;
    assert_eq!(s.rewritten, 3, "legacy TOTP seed + passkey + pending challenge: {s:?}");
    assert_eq!((s.unreadable, stale), (0, 0));
    let (again, _) = rotate(ta).await;
    assert_eq!(again.rewritten, 0, "idempotent");
    let mut tx = db::tenant_tx(&c2.owner, tb).await.unwrap();
    assert!(
        keyring::stale_count(&mut tx, &c2.st.cfg.data_keys).await.unwrap() > 0,
        "tenant B untouched by tenant A's rotation"
    );
    tx.commit().await.unwrap();

    // Old key removed: rotated tenant A works (TOTP and WebAuthn); un-rotated tenant B cannot decrypt.
    let (c3, clock3) = clocked(Some(DataKeys::new(2, k2))).await;
    clock3.advance(Duration::seconds(120));
    let r = c3
        .login(&slug_a, &email_a, PW, Some(totp_for(&c3, ta, ua, 0).await))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let r = wa_finish(&c3, &slug_a, &pending.body, &mut key).await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "in-flight challenge survives rotation: {:?}",
        r.body
    );
    wa_login(&c3, &slug_a, &ukey_email, &mut key).await;
    let r = c3.login(&slug_b, &email_b, PW, Some("000000".into())).await;
    assert_eq!(
        r.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "B still needs key 1 until rotated"
    );

    // Corrupt blob: reported as unreadable, left in place.
    let mut tx = db::tenant_tx(&c2.owner, tb).await.unwrap();
    sqlx::query("UPDATE users SET totp_pending_enc = '\\x00112233'::bytea WHERE id = $1")
        .bind(ub)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (s, stale) = rotate(tb).await;
    assert_eq!((s.unreadable, stale), (1, 1), "{s:?}");
}

#[tokio::test]
async fn session_and_refresh_expiry_follow_the_clock() {
    let (c, clock) = clocked(None).await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "viewer").await;
    let r = c.login(&slug, &email, PW, None).await;
    let (acc, rt) = (
        r.body["access_token"].as_str().unwrap().to_owned(),
        r.body["refresh_token"].as_str().unwrap().to_owned(),
    );
    clock.advance(Duration::seconds(899));
    assert_eq!(
        c.get("/api/v1/auth/me", &acc).await.status,
        StatusCode::OK,
        "inside access TTL"
    );
    clock.advance(Duration::seconds(2));
    assert_eq!(
        c.get("/api/v1/auth/me", &acc).await.status,
        StatusCode::UNAUTHORIZED,
        "access token expired"
    );

    let r = c
        .post("/api/v1/auth/refresh", None, json!({ "refresh_token": rt }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let acc2 = r.body["access_token"].as_str().unwrap().to_owned();
    let rt2 = r.body["refresh_token"].as_str().unwrap().to_owned();
    assert_eq!(c.get("/api/v1/auth/me", &acc2).await.status, StatusCode::OK);

    // Near the 12 h absolute lifetime the access token is capped to the session end.
    clock.advance(Duration::seconds(12 * 3600 - 901 - 100));
    let r = c
        .post("/api/v1/auth/refresh", None, json!({ "refresh_token": rt2 }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let exp = r.body["expires_in"].as_i64().unwrap();
    assert!((1..=100).contains(&exp), "capped expires_in {exp}");
    let rt3 = r.body["refresh_token"].as_str().unwrap().to_owned();

    // A rotated-out token presented later is still treated as theft.
    let r = c
        .post("/api/v1/auth/refresh", None, json!({ "refresh_token": rt2 }))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(
        c.security_kinds(t)
            .await
            .iter()
            .any(|k| k == "auth.refresh_token_reuse")
    );
    let r = c
        .post("/api/v1/auth/refresh", None, json!({ "refresh_token": rt3 }))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "family revoked");

    // Past the absolute lifetime a fresh session's refresh fails too.
    let r = c.login(&slug, &email, PW, None).await;
    let rt4 = r.body["refresh_token"].as_str().unwrap().to_owned();
    clock.advance(Duration::seconds(12 * 3600 + 1));
    let r = c
        .post("/api/v1/auth/refresh", None, json!({ "refresh_token": rt4 }))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "session absolute expiry");
}

#[tokio::test]
async fn lockout_expires_with_the_clock() {
    let (c, clock) = clocked(None).await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "viewer").await;
    for _ in 0..5 {
        c.login(&slug, &email, "wrong password here", None).await;
    }
    assert_eq!(
        c.login(&slug, &email, PW, None).await.status,
        StatusCode::UNAUTHORIZED,
        "locked"
    );
    clock.advance(Duration::seconds(899));
    assert_eq!(
        c.login(&slug, &email, PW, None).await.status,
        StatusCode::UNAUTHORIZED,
        "still locked"
    );
    clock.advance(Duration::seconds(2));
    assert_eq!(
        c.login(&slug, &email, PW, None).await.status,
        StatusCode::OK,
        "lock expired"
    );
}

#[tokio::test]
async fn bad_assertions_lock_out_and_challenges_stay_bounded() {
    let (c, _clock) = clocked(None).await;
    let (t, slug) = c.tenant().await;
    let (ua, email_a) = c.user(t, "tenant_admin").await;
    let (_, email_b) = c.user(t, "tenant_admin").await;
    let (mut ka, mut kb) = (SoftPasskey::new(true), SoftPasskey::new(true));
    let tok_a = c.token(&slug, &email_a).await;
    enroll_key(&c, &tok_a, &mut ka).await;
    let tok_b = c.token(&slug, &email_b).await;
    enroll_key(&c, &tok_b, &mut kb).await;

    // Cross-user: B's valid assertion cannot redeem A's challenge.
    let sa = wa_start(&c, &slug, &email_a, PW).await;
    let sb = wa_start(&c, &slug, &email_b, PW).await;
    let rcr: RequestChallengeResponse = serde_json::from_value(sb.body["public_key"].clone()).unwrap();
    let b_cred = kb.do_authentication(origin(), rcr).unwrap();
    let r = c
        .post(
            "/api/v1/auth/webauthn/login/finish",
            None,
            json!({ "tenant": slug, "challenge_id": sa.body["challenge_id"], "credential": b_cred }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "cross-user assertion");

    // Repeated starts keep one live challenge per user and purpose.
    for _ in 0..2 {
        assert_eq!(wa_start(&c, &slug, &email_a, PW).await.status, StatusCode::OK);
    }
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    let live: i64 = sqlx::query_scalar("SELECT count(*) FROM webauthn_challenges WHERE user_id = $1")
        .bind(ua)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(live, 1);

    // Five forged assertions lock the account; the real key then cannot start a login.
    for _ in 0..4 {
        let s = wa_start(&c, &slug, &email_a, PW).await;
        assert_eq!(s.status, StatusCode::OK, "{:?}", s.body);
        let rcr: RequestChallengeResponse = serde_json::from_value(s.body["public_key"].clone()).unwrap();
        let mut cred = serde_json::to_value(ka.do_authentication(origin(), rcr).unwrap()).unwrap();
        let sig = cred["response"]["signature"].as_str().unwrap().to_owned();
        cred["response"]["signature"] =
            Value::String(format!("{}{}", if sig.starts_with('A') { "B" } else { "A" }, &sig[1..]));
        let r = c
            .post(
                "/api/v1/auth/webauthn/login/finish",
                None,
                json!({ "tenant": slug, "challenge_id": s.body["challenge_id"], "credential": cred }),
            )
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        wa_start(&c, &slug, &email_a, PW).await.status,
        StatusCode::UNAUTHORIZED,
        "locked"
    );
    assert!(c.security_kinds(t).await.iter().any(|k| k == "auth.account_locked"));
}

#[tokio::test]
async fn adding_a_key_to_a_protected_account_needs_a_key_session() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (uid, email) = c.user(t, "tenant_admin").await;
    let tok = c.token(&slug, &email).await;
    // Simulates the enrolment-vs-login race: the account became key-protected after this session started.
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    sqlx::query("UPDATE users SET webauthn_enabled = true WHERE id = $1")
        .bind(uid)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let r = c
        .post(
            "/api/v1/auth/mfa/webauthn/register/start",
            Some(&tok),
            json!({ "password": PW }),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.body["error"], "mfa_required", "{:?}", r.body);
    let r = c.login(&slug, &email, PW, None).await;
    assert_eq!(r.body["error"], "webauthn_required");
}

#[tokio::test]
async fn own_security_keys_and_recovery_codes_need_a_key_session_and_password() {
    let (c, _clock) = clocked(None).await;
    let (t, slug, uid, tok) = c.admin().await;
    let email = admin_email(&c, uid, t).await;
    let st = c.get("/api/v1/auth/mfa/status", &tok).await;
    assert_eq!(st.body["webauthn_enabled"], false, "{:?}", st.body);
    let mut key = SoftPasskey::new(true);
    let codes = enroll_key(&c, &tok, &mut key).await["recovery_codes"].clone();
    let tok = wa_login(&c, &slug, &email, &mut key).await;
    let mut key2 = SoftPasskey::new(true);
    enroll_key(&c, &tok, &mut key2).await;
    let tok = wa_login(&c, &slug, &email, &mut key2).await;
    let st = c.get("/api/v1/auth/mfa/status", &tok).await;
    assert_eq!(st.body["webauthn_enabled"], true);
    assert_eq!(st.body["recovery_codes_remaining"], 10);
    let creds = st.body["credentials"].as_array().unwrap().clone();
    assert_eq!(creds.len(), 2);
    assert_eq!(creds[0]["label"], "yubikey");
    assert!(
        st.body.to_string().find("passkey").is_none(),
        "no key material leaves the server"
    );

    // Regenerate: wrong password refused; old codes stop working, new ones are returned once.
    let bad = c
        .post(
            "/api/v1/auth/mfa/recovery-codes",
            Some(&tok),
            json!({ "password": "wrong password here" }),
        )
        .await;
    assert_eq!(bad.status, StatusCode::UNAUTHORIZED);
    let r = c
        .post("/api/v1/auth/mfa/recovery-codes", Some(&tok), json!({ "password": PW }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let fresh = r.body["recovery_codes"].as_array().unwrap().clone();
    assert_eq!(fresh.len(), 10);
    let old = c
        .post(
            "/api/v1/auth/webauthn/recover",
            None,
            json!({ "tenant": slug, "email": email, "password": PW, "recovery_code": codes[0] }),
        )
        .await;
    assert_eq!(
        old.status,
        StatusCode::UNAUTHORIZED,
        "regenerated codes replace the old set"
    );

    // A session without a key (recovery) cannot manage keys.
    let rec = c
        .post(
            "/api/v1/auth/webauthn/recover",
            None,
            json!({ "tenant": slug, "email": email, "password": PW, "recovery_code": fresh[0] }),
        )
        .await;
    assert_eq!(rec.status, StatusCode::OK, "{:?}", rec.body);
    let weak = rec.body["access_token"].as_str().unwrap().to_owned();
    let st = c.get("/api/v1/auth/mfa/status", &weak).await;
    assert_eq!(st.body["webauthn_enabled"], false, "recovery cleared the keys");
    let r = c
        .post(
            "/api/v1/auth/mfa/recovery-codes",
            Some(&weak),
            json!({ "password": PW }),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // Re-enrol two keys, then remove them one by one.
    let mut k3 = SoftPasskey::new(true);
    enroll_key(&c, &weak, &mut k3).await;
    let tok = wa_login(&c, &slug, &email, &mut k3).await;
    let mut k4 = SoftPasskey::new(true);
    enroll_key(&c, &tok, &mut k4).await;
    let tok = wa_login(&c, &slug, &email, &mut k4).await;
    let other = wa_login(&c, &slug, &email, &mut k3).await;
    let creds = c.get("/api/v1/auth/mfa/status", &tok).await.body["credentials"].clone();
    let (first, second) = (creds[0]["id"].as_str().unwrap(), creds[1]["id"].as_str().unwrap());
    let r = c
        .post(
            &format!("/api/v1/auth/mfa/webauthn/credentials/{first}/remove"),
            Some(&tok),
            json!({ "password": "wrong password here" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = c
        .post(
            &format!("/api/v1/auth/mfa/webauthn/credentials/{first}/remove"),
            Some(&tok),
            json!({ "password": PW }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["remaining"], 1);
    assert_eq!(
        c.get("/api/v1/auth/me", &other).await.status,
        StatusCode::OK,
        "removing a spare key keeps sessions"
    );
    let s = wa_start(&c, &slug, &email, PW).await;
    let rcr: RequestChallengeResponse = serde_json::from_value(s.body["public_key"].clone()).unwrap();
    assert!(
        k3.do_authentication(origin(), rcr).is_err(),
        "the removed key is no longer offered"
    );

    // Another user cannot remove this user's key (404, not 403: no existence oracle).
    let (_, e2) = c.user(t, "tenant_admin").await;
    let t2 = c.mfa_token(&slug, &e2).await;
    let r = c
        .post(
            &format!("/api/v1/auth/mfa/webauthn/credentials/{second}/remove"),
            Some(&t2),
            json!({ "password": PW }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    let r = c
        .post(
            &format!("/api/v1/auth/mfa/webauthn/credentials/{second}/remove"),
            Some(&tok),
            json!({ "password": PW }),
        )
        .await;
    assert_eq!(r.body["webauthn_enabled"], false, "{:?}", r.body);
    assert_eq!(
        c.get("/api/v1/auth/me", &other).await.status,
        StatusCode::UNAUTHORIZED,
        "other sessions end"
    );
    assert_eq!(
        c.get("/api/v1/auth/me", &tok).await.status,
        StatusCode::OK,
        "the acting session stays"
    );
    let st = c.get("/api/v1/auth/mfa/status", &tok).await;
    assert_eq!(st.body["recovery_codes_remaining"], 0);
    assert!(c.security_kinds(t).await.iter().any(|k| k == "auth.mfa_reset"));
}

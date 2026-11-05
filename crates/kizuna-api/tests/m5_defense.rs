#![allow(clippy::unwrap_used)]
mod common;

use axum::http::StatusCode;
use common::*;
use kizuna_api::db;
use serde_json::{Value, json};

async fn incident(c: &Ctx, tok: &str, rule: &str) -> Value {
    let list = c.get("/api/v1/security/incidents", tok).await;
    let i = list
        .body
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["rule"] == rule)
        .unwrap_or_else(|| panic!("no {rule}: {:?}", list.body))
        .clone();
    c.get(
        &format!("/api/v1/security/incidents/{}", i["id"].as_str().unwrap()),
        tok,
    )
    .await
    .body
}

fn action<'a>(d: &'a Value, kind: &str) -> &'a Value {
    d["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == kind)
        .unwrap()
}

#[tokio::test]
async fn token_theft_contained_and_lock_needs_approval() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let (uid, email) = c.user(t, "analyst").await;
    let l = c.login(&slug, &email, PW, None).await;
    let r1 = l.body["refresh_token"].as_str().unwrap().to_owned();
    c.post("/api/v1/auth/refresh", None, json!({ "refresh_token": r1 }))
        .await;
    c.post("/api/v1/auth/refresh", None, json!({ "refresh_token": r1 }))
        .await; // replay
    let d = incident(&c, &admin, "token_theft").await;
    assert_eq!(d["incident"]["severity"], "critical");
    assert_eq!(d["incident"]["status"], "contained");
    assert_eq!(action(&d, "revoke_user_sessions")["status"], "executed");
    let lock = action(&d, "lock_account");
    assert_eq!(
        (lock["status"].as_str(), lock["policy"].as_str()),
        (Some("proposed"), Some("requires_approval"))
    );
    assert!(!d["events"].as_array().unwrap().is_empty(), "forensic events linked");

    let iid = d["incident"]["id"].as_str().unwrap();
    let aid = lock["id"].as_str().unwrap();
    let path = format!("/api/v1/security/incidents/{iid}/actions/{aid}");
    let (_, rev) = c.user(t, "reviewer").await;
    let rtok = c.token(&slug, &rev).await;
    assert_eq!(
        c.post(&format!("{path}/approve"), Some(&rtok), json!({})).await.status,
        StatusCode::FORBIDDEN
    );
    let r = c
        .post(
            &format!("{path}/approve"),
            Some(&admin),
            json!({"comment":"confirmed with user"}),
        )
        .await;
    assert_eq!(r.body["status"], "executed", "{:?}", r.body);
    assert_eq!(
        c.login(&slug, &email, PW, None).await.status,
        StatusCode::UNAUTHORIZED,
        "credentials disabled"
    );
    let r = c
        .post(
            &format!("{path}/revert"),
            Some(&admin),
            json!({"comment":"new device enrolled"}),
        )
        .await;
    assert_eq!(r.body["status"], "reverted");
    assert_eq!(c.login(&slug, &email, PW, None).await.status, StatusCode::OK);
    let _ = uid;
    let res = c
        .post(
            &format!("/api/v1/security/incidents/{iid}/resolve"),
            Some(&admin),
            json!({"status":"resolved","comment":"token rotated"}),
        )
        .await;
    assert_eq!(res.body["status"], "resolved");
}

#[tokio::test]
async fn password_spraying_blocks_the_source_address_only() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let mut users = Vec::new();
    for _ in 0..3 {
        users.push(c.user(t, "viewer").await.1);
    }
    let attacker = [203, 0, 113, 66];
    for i in 0..9 {
        c.login_from(attacker, &slug, &users[i % 3], "wrong password here")
            .await;
    }
    let d = incident(&c, &admin, "password_spraying").await;
    assert_eq!(action(&d, "block_ip")["status"], "executed", "{d:?}");
    let r = c.login_from(attacker, &slug, &users[0], PW).await;
    assert_eq!(
        (r.status, r.body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("blocked")),
        "even with the right password"
    );
    assert_eq!(
        c.login_from([192, 0, 2, 77], &slug, &users[1], PW).await.status,
        StatusCode::OK,
        "other addresses unaffected"
    );
    let s = c.get("/api/v1/security/summary", &admin).await;
    assert_eq!(s.body["active_ip_blocks"].as_array().unwrap().len(), 1);
    let path = format!(
        "/api/v1/security/incidents/{}/actions/{}/revert",
        d["incident"]["id"].as_str().unwrap(),
        action(&d, "block_ip")["id"].as_str().unwrap()
    );
    assert_eq!(c.post(&path, Some(&admin), json!({})).await.body["status"], "reverted");
    assert_eq!(
        c.login_from(attacker, &slug, &users[0], PW).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn admin_address_is_never_auto_blocked_and_last_admin_never_locked() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await; // admin logged in from c.ip
    let mut users = Vec::new();
    for _ in 0..3 {
        users.push(c.user(t, "viewer").await.1);
    }
    for i in 0..9 {
        c.login(&slug, &users[i % 3], "wrong password here", None).await;
    }
    // The admin's own address was in play: no automatic block; escalated instead.
    let d = incident(&c, &admin, "password_spraying").await;
    let b = action(&d, "block_ip");
    assert_eq!(
        (b["status"].as_str(), b["policy"].as_str()),
        (Some("proposed"), Some("requires_approval"))
    );
    assert_eq!(c.get("/api/v1/auth/me", &admin).await.status, StatusCode::OK);
}

#[tokio::test]
async fn last_admin_is_never_locked_and_emergency_unlock_restores_access() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (admin_id, email) = c.user(t, "tenant_admin").await;
    let tok = c.token(&slug, &email).await;
    let secret = c.enroll_totp(&tok).await;
    // A stolen-token signal on the only admin: sessions revoked automatically, lock only proposed.
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    sqlx::query("INSERT INTO security_events (id, tenant_id, kind, severity, user_id) VALUES ($1,$2,'auth.refresh_token_reuse','high',$3)")
        .bind(uuid::Uuid::now_v7())
        .bind(t)
        .bind(admin_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    kizuna_api::incidents::run_detection(&c.st.pool, t).await.unwrap();
    assert_eq!(
        c.get("/api/v1/auth/me", &tok).await.status,
        StatusCode::UNAUTHORIZED,
        "sessions revoked"
    );
    let r = c
        .login(
            &slug,
            &email,
            PW,
            Some(kizuna_api::crypto::totp_now(&secret, now() + 30)),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "admin can sign in again: {:?}", r.body);
    let admin = r.body["access_token"].as_str().unwrap().to_owned();
    let d = incident(&c, &admin, "token_theft").await;
    let lock = action(&d, "lock_account");
    assert_eq!(lock["status"], "proposed");
    let path = format!(
        "/api/v1/security/incidents/{}/actions/{}/approve",
        d["incident"]["id"].as_str().unwrap(),
        lock["id"].as_str().unwrap()
    );
    // The only admin cannot approve containment against themselves (four-eyes, final review M3); with no second
    // admin the lock can never execute, so the last administrator is never disabled by automation.
    let r = c.post(&path, Some(&admin), json!({"comment":"test"})).await;
    assert_eq!(r.body["error"], "no_self_review", "{:?}", r.body);
    // Emergency procedure (operator CLI) restores admins disabled by containment.
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    sqlx::query("UPDATE users SET status = 'disabled', disabled_by_action = $2 WHERE id = $1")
        .bind(admin_id)
        .bind(uuid::Uuid::now_v7())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let out = kizuna_api::incidents::emergency_unlock(&c.owner, t).await.unwrap();
    assert_eq!(out["admins_reenabled"], 1);
}

#[tokio::test]
async fn integrity_failure_quarantines_and_reupload_recovers() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let (_, e) = c.user(t, "analyst").await;
    let tok = c.token(&slug, &e).await;
    let data = b"Part Number,Rev\nBRK-1,C\n";
    let id = c.upload(&tok, "s.csv", data, None).await.body["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let key = kizuna_api::storage::ObjectKey {
        tenant: t,
        sha256: kizuna_api::crypto::sha256(data),
    };
    std::fs::write(
        kizuna_api::storage::FsStore::new(c.st.cfg.storage_dir.clone()).path(&key),
        b"tampered",
    )
    .unwrap();
    assert_eq!(
        c.post(&format!("/api/v1/artifacts/{id}/verify"), Some(&tok), json!({}))
            .await
            .body["intact"],
        false
    );
    let d = incident(&c, &admin, "artifact_integrity_failure").await;
    assert_eq!(action(&d, "quarantine_artifact")["status"], "executed");
    assert_eq!(
        c.get(&format!("/api/v1/artifacts/{id}"), &tok).await.body["artifact"]["status"],
        "quarantined"
    );
    // Recovery: re-upload the original bytes, verify, release.
    c.upload(&tok, "s.csv", data, None).await;
    assert_eq!(
        c.post(&format!("/api/v1/artifacts/{id}/verify"), Some(&tok), json!({}))
            .await
            .body["intact"],
        true
    );
    assert_eq!(
        c.post(
            &format!("/api/v1/artifacts/{id}/release"),
            Some(&admin),
            json!({ "reason": "reviewed the scan findings" })
        )
        .await
        .body["status"],
        "accepted"
    );
}

#[tokio::test]
async fn upload_abuse_restricts_uploads_and_authz_probing_revokes_sessions() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let (_, e) = c.user(t, "analyst").await;
    let tok = c.token(&slug, &e).await;
    for _ in 0..5 {
        c.upload(&tok, "x.exe", b"MZ\x90\x00\x03\x00", None).await;
    }
    let r = c.upload(&tok, "ok.csv", b"a,b\n1,2\n", None).await;
    assert_eq!(r.body["error"], "uploads_restricted");
    let d = incident(&c, &admin, "upload_abuse").await;
    let a = action(&d, "restrict_uploads");
    let path = format!(
        "/api/v1/security/incidents/{}/actions/{}/revert",
        d["incident"]["id"].as_str().unwrap(),
        a["id"].as_str().unwrap()
    );
    assert_eq!(c.post(&path, Some(&admin), json!({})).await.body["status"], "reverted");
    assert_eq!(
        c.upload(&tok, "ok.csv", b"a,b\n1,2\n", None).await.status,
        StatusCode::OK
    );

    for _ in 0..10 {
        c.get("/api/v1/users", &tok).await;
    }
    assert_eq!(
        c.get("/api/v1/auth/me", &tok).await.status,
        StatusCode::UNAUTHORIZED,
        "sessions revoked after probing"
    );
}

#[tokio::test]
async fn signed_checkpoints_detect_full_rechain_by_db_owner() {
    let c = ctx().await;
    let (t, _, _, admin) = c.admin().await;
    let cp = c.post("/api/v1/audit/checkpoints", Some(&admin), json!({})).await;
    assert_eq!(cp.status, StatusCode::OK, "{:?}", cp.body);
    assert_eq!(
        c.get("/api/v1/audit/checkpoints/verify", &admin).await.body["intact"],
        true
    );

    // Malicious owner rewrites event 1 and re-chains everything consistently (plain verification is fooled).
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    sqlx::query("ALTER TABLE audit_events DISABLE TRIGGER audit_events_append_only")
        .execute(&mut *tx)
        .await
        .unwrap();
    type Row = (
        i64,
        time::OffsetDateTime,
        Option<uuid::Uuid>,
        String,
        String,
        String,
        Option<String>,
        Value,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT seq, occurred_at, actor_user_id, actor_kind, action, target_type, target_id, details FROM audit_events WHERE tenant_id = $1 ORDER BY seq",
    )
    .bind(t)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    let mut prev = audit_engine::GENESIS.to_vec();
    for (seq, at, actor, ak, action, tt, tid, details) in rows {
        let details = if seq == 1 { json!({"forged": true}) } else { details };
        let ev = audit_engine::Event {
            tenant_id: t,
            seq,
            occurred_at_us: (at.unix_timestamp_nanos() / 1000) as i64,
            actor_user_id: actor,
            actor_kind: ak,
            action,
            target_type: tt,
            target_id: tid,
            details: details.clone(),
        };
        let h = audit_engine::chain_hash(&ev, &prev).unwrap().to_vec();
        sqlx::query(
            "UPDATE audit_events SET details = $3, prev_hash = $4, hash = $5 WHERE tenant_id = $1 AND seq = $2",
        )
        .bind(t)
        .bind(seq)
        .bind(&details)
        .bind(&prev)
        .bind(&h)
        .execute(&mut *tx)
        .await
        .unwrap();
        prev = h;
    }
    sqlx::query("UPDATE audit_heads SET last_hash = $2 WHERE tenant_id = $1")
        .bind(t)
        .bind(&prev)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE audit_events ENABLE TRIGGER audit_events_append_only")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        c.get("/api/v1/audit/verify", &admin).await.body["intact"],
        true,
        "re-chained log passes plain verification"
    );
    let v = c.get("/api/v1/audit/checkpoints/verify", &admin).await;
    assert_eq!(v.body["intact"], false, "{:?}", v.body);
    let ext = c
        .post(
            "/api/v1/audit/checkpoints/verify-external",
            Some(&admin),
            cp.body.clone(),
        )
        .await;
    assert_eq!(ext.body["intact"], false);
    let mut forged = cp.body.clone();
    forged["seq"] = json!(1);
    assert_eq!(
        c.post("/api/v1/audit/checkpoints/verify-external", Some(&admin), forged)
            .await
            .body["error"],
        "invalid_checkpoint_signature"
    );
    // KZ-C1: a new checkpoint must not re-anchor on the re-chained log.
    let n = |c: &Ctx| {
        let owner = c.owner.clone();
        async move {
            let mut tx = db::tenant_tx(&owner, t).await.unwrap();
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_checkpoints")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            n
        }
    };
    let before = n(&c).await;
    // (a fresh auditor: the admin has used up the per-user rate limit of heavy audit calls)
    let (_, aud) = c.user(t, "auditor").await;
    let slug: String = sqlx::query_scalar("SELECT slug FROM tenants WHERE id = $1")
        .bind(t)
        .fetch_one(&c.owner)
        .await
        .unwrap();
    let at = c.token(&slug, &aud).await;
    let r = c.post("/api/v1/audit/checkpoints", Some(&at), json!({})).await;
    assert_eq!(
        (r.status, r.body["error"].clone()),
        (StatusCode::CONFLICT, json!("audit_chain_not_intact"))
    );
    assert_eq!(n(&c).await, before, "no checkpoint signed over the forged head");
}

#[tokio::test]
async fn incidents_rbac_and_isolation() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let (_, v) = c.user(t, "viewer").await;
    let vt = c.token(&slug, &v).await;
    assert_eq!(
        c.get("/api/v1/security/incidents", &vt).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        c.get("/api/v1/security/summary", &vt).await.status,
        StatusCode::FORBIDDEN
    );
    let (tb, sb, _, admin_b) = c.admin().await;
    let _ = (tb, sb);
    for _ in 0..12 {
        c.get("/api/v1/users", &vt).await;
    }
    let ia = c.get("/api/v1/security/incidents", &admin).await;
    assert!(!ia.body.as_array().unwrap().is_empty());
    let id = ia.body[0]["id"].as_str().unwrap();
    assert_eq!(
        c.get(&format!("/api/v1/security/incidents/{id}"), &admin_b)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(c.get("/api/v1/security/incidents", &admin_b).await.body, json!([]));
}

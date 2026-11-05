#![allow(clippy::unwrap_used)]
//! Regression tests for the final independent security review (BUILD_STATUS.md, "Final security review").
mod common;

use axum::http::{Method, StatusCode};
use common::*;
use kizuna_api::db;
use serde_json::{Value, json};

async fn kinds_count(c: &Ctx, t: uuid::Uuid, kind: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM security_events WHERE tenant_id = $1 AND kind = $2")
        .bind(t)
        .bind(kind)
        .fetch_one(&c.owner)
        .await
        .unwrap()
}

async fn incidents(c: &Ctx, tok: &str) -> Vec<Value> {
    c.get("/api/v1/security/incidents", tok)
        .await
        .body
        .as_array()
        .unwrap()
        .clone()
}

#[tokio::test]
async fn h1_denial_floods_are_bounded_and_old_events_cannot_hide_new_ones() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, v) = c.user(t, "viewer").await;
    let vt = c.token(&slug, &v).await;
    for _ in 0..80 {
        c.get("/api/v1/users", &vt).await;
    }
    assert!(
        kinds_count(&c, t, "authz.denied").await <= 30,
        "denial events are rate-limited per user"
    );

    let (t2, slug2) = c.tenant().await;
    let (u2, e2) = c.user(t2, "viewer").await;
    let mut tx = db::tenant_tx(&c.owner, t2).await.unwrap();
    sqlx::query(
        "INSERT INTO security_events (id, tenant_id, occurred_at, kind, severity) \
         SELECT gen_random_uuid(), $1, now() - interval '50 minutes', 'export.report', 'info' FROM generate_series(1, 20001)",
    )
    .bind(t2)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    for _ in 0..5 {
        c.login(&slug2, &e2, "wrong password here", None).await;
    }
    let ran = kizuna_api::incidents::run_detection(&c.st.pool, t2).await.unwrap();
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM incidents WHERE rule = 'brute_force_account' AND subject = $1")
            .bind(format!("user:{u2}"))
            .fetch_one(&mut *db::tenant_tx(&c.owner, t2).await.unwrap())
            .await
            .unwrap();
    assert_eq!(
        n, 1,
        "fresh brute force detected despite 20,001 older events (ran={ran})"
    );
}

async fn spray(c: &Ctx, slug: &str, users: &[String], n: usize) {
    for i in 0..n {
        c.login_from([203, 0, 113, 9], slug, &users[i % 3], "wrong password here")
            .await;
    }
}

#[tokio::test]
async fn m1_false_positive_stays_resolved_and_expired_blocks_are_reapplied() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let mut users = Vec::new();
    for _ in 0..3 {
        users.push(c.user(t, "viewer").await.1);
    }
    spray(&c, &slug, &users, 9).await;
    let inc = incidents(&c, &admin)
        .await
        .into_iter()
        .find(|i| i["rule"] == "password_spraying")
        .unwrap();
    let id = inc["id"].as_str().unwrap().to_owned();
    // Expire the block, then a returning attacker is blocked again.
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    sqlx::query("UPDATE ip_blocks SET until = now() - interval '1 second'")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        c.login_from([203, 0, 113, 9], &slug, &users[0], PW).await.status,
        StatusCode::OK,
        "block expired"
    );
    spray(&c, &slug, &users, 2).await;
    assert_eq!(
        c.login_from([203, 0, 113, 9], &slug, &users[0], PW).await.body["error"],
        "blocked",
        "re-applied"
    );

    // Admin judges it a false positive and lifts the block: the same events do not re-trigger it.
    let d = c.get(&format!("/api/v1/security/incidents/{id}"), &admin).await;
    let block = d.body["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "block_ip")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    c.post(
        &format!("/api/v1/security/incidents/{id}/actions/{block}/revert"),
        Some(&admin),
        json!({}),
    )
    .await;
    let r = c
        .post(
            &format!("/api/v1/security/incidents/{id}/resolve"),
            Some(&admin),
            json!({"status":"false_positive","comment":"pen test"}),
        )
        .await;
    assert_eq!(r.body["status"], "false_positive", "{:?}", r.body);
    kizuna_api::incidents::run_detection(&c.st.pool, t).await.unwrap();
    assert_eq!(
        c.login_from([203, 0, 113, 9], &slug, &users[0], PW).await.status,
        StatusCode::OK,
        "no re-block after false positive"
    );
}

#[tokio::test]
async fn m2_one_analyst_cannot_suspend_tenant_extraction() {
    let c = ctx_with(|cfg| cfg.extraction_python = "/nonexistent/python".into()).await;
    let (t, slug) = c.tenant().await;
    let (_, e) = c.user(t, "analyst").await;
    let tok = c.token(&slug, &e).await;
    let mut ids = Vec::new();
    for i in 0..3 {
        ids.push(
            c.upload(&tok, "s.csv", format!("Part Number,Rev\nA-{i},B\n").as_bytes(), None)
                .await
                .body["artifact"]["id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    let mut statuses = Vec::new();
    for _ in 0..4 {
        statuses.push(
            c.post(&format!("/api/v1/artifacts/{}/extract", ids[0]), Some(&tok), json!({}))
                .await
                .status,
        );
    }
    assert_eq!(statuses[3], StatusCode::TOO_MANY_REQUESTS, "{statuses:?}");
    for id in &ids[1..] {
        c.post(&format!("/api/v1/artifacts/{id}/extract"), Some(&tok), json!({}))
            .await;
    }
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    let suspended: Option<bool> = sqlx::query_scalar("SELECT extraction_suspended_until > now() FROM tenant_controls")
        .fetch_optional(&mut *tx)
        .await
        .unwrap()
        .flatten();
    assert_ne!(suspended, Some(true), "tenant-wide suspension needs approval");
}

#[tokio::test]
async fn m3_m4_admin_cannot_judge_own_incident_and_emergency_unlock_keeps_deliberate_disables() {
    let c = ctx().await;
    let (t, slug, a_id, a) = c.admin().await;
    let (b_id, eb) = c.user(t, "tenant_admin").await;
    let b = c.mfa_token(&slug, &eb).await;
    for _ in 0..5 {
        let (u, _) = c.user(t, "viewer").await;
        c.call(
            Method::PATCH,
            &format!("/api/v1/users/{u}"),
            Some(&a),
            Some(json!({"role":"analyst"})),
        )
        .await;
    }
    let inc = incidents(&c, &b)
        .await
        .into_iter()
        .find(|i| i["rule"] == "privileged_operation_burst" && i["subject"] == format!("user:{a_id}"))
        .unwrap();
    let id = inc["id"].as_str().unwrap();
    let own = c
        .post(
            &format!("/api/v1/security/incidents/{id}/resolve"),
            Some(&a),
            json!({"status":"false_positive","comment":"me"}),
        )
        .await;
    assert_eq!(own.body["error"], "no_self_review");
    let other = c
        .post(
            &format!("/api/v1/security/incidents/{id}/resolve"),
            Some(&b),
            json!({"status":"resolved","comment":"reviewed"}),
        )
        .await;
    assert_eq!(other.body["status"], "resolved");

    // B is disabled deliberately (offboarding); emergency unlock must not bring B back.
    c.call(
        Method::PATCH,
        &format!("/api/v1/users/{b_id}"),
        Some(&a),
        Some(json!({"status":"disabled"})),
    )
    .await;
    let out = kizuna_api::incidents::emergency_unlock(&c.owner, t).await.unwrap();
    assert_eq!(out["admins_reenabled"], 0);
    let st: String = sqlx::query_scalar("SELECT status FROM users WHERE id = $1")
        .bind(b_id)
        .fetch_one(&mut *db::tenant_tx(&c.owner, t).await.unwrap())
        .await
        .unwrap();
    assert_eq!(st, "disabled");
}

#[tokio::test]
async fn l1_junk_checkpoint_is_rejected_without_alert() {
    let c = ctx().await;
    let (t, _, _, admin) = c.admin().await;
    let junk = json!({"tenant_id": t, "seq": 1, "head_hash": "00", "signed_at_us": 0, "key_id": "x", "public_key": "00", "signature": "00"});
    let r = c
        .post("/api/v1/audit/checkpoints/verify-external", Some(&admin), junk)
        .await;
    assert_eq!(r.body["error"], "invalid_checkpoint_signature");
    assert_eq!(kinds_count(&c, t, "integrity.audit_chain_broken").await, 0);
}

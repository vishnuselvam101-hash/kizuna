#![allow(clippy::unwrap_used)]
//! Regression tests for docs/security/REVIEW_M5_M7.md (one or more per finding).
mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{Method, StatusCode};
use axum::routing::post;
use common::*;
use kizuna_api::config::{AiConfig, Secret};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone)]
struct Mock {
    reply: String,
    sleep_ms: u64,
    calls: Arc<Mutex<usize>>,
}

async fn chat(State(m): State<Mock>) -> String {
    *m.calls.lock().unwrap() += 1;
    tokio::time::sleep(Duration::from_millis(m.sleep_ms)).await;
    json!({ "choices": [ { "message": { "role": "assistant", "content": m.reply } } ] }).to_string()
}

async fn mock(reply: &str, sleep_ms: u64) -> (Mock, String) {
    let m = Mock {
        reply: reply.into(),
        sleep_ms,
        calls: Arc::default(),
    };
    let app = axum::Router::new()
        .route("/v1/chat/completions", post(chat))
        .with_state(m.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (m, format!("http://{addr}/v1"))
}

fn with_ai(base: String, timeout_secs: u64) -> impl FnOnce(&mut kizuna_api::config::Config) {
    move |c| {
        c.ai = Some(AiConfig {
            base_url: base,
            model: "mock".into(),
            api_key: None,
            timeout_secs,
            daily_calls: 100,
            daily_chars: 1_000_000,
            calls_per_min: 100,
        })
    }
}

async fn opt_in(c: &Ctx, admin: &str) {
    let r = c
        .call(
            Method::PUT,
            "/api/v1/integrations/ai-extraction",
            Some(admin),
            Some(json!({ "expected_version": 0, "enabled": true, "accept_notice": "ai-notice@1" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
}

async fn extract(c: &Ctx, tok: &str, name: &str, data: &[u8]) -> Value {
    let r = c.upload(tok, name, data, Some("spec_document")).await;
    let id = r.body["artifact"]["id"].as_str().unwrap().to_owned();
    c.post(&format!("/api/v1/artifacts/{id}/extract"), Some(tok), json!({}))
        .await
        .body
}

async fn owner_count(c: &Ctx, t: Uuid, sql: &str) -> i64 {
    let mut tx = kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap();
    let n: i64 = sqlx::query_scalar(sql).fetch_one(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    n
}

const MAP: &str = r#"{"mappings":[{"id":0,"field":"material","confidence":0.8}]}"#;

fn decision(claim: &str, key: &str) -> Value {
    json!({ "kind": "supplier_part_release", "title": "t", "subject_type": "part", "subject_key": key,
            "outcome": "approved", "rationale": "r", "evidence": [ { "claim_id": claim, "role": "basis" } ], "depends_on": [] })
}

/// M1: "request evidence" no longer launders an AI suggestion into usable evidence.
#[tokio::test]
async fn m7_ai_request_evidence_does_not_launder_suggestion() {
    let (_m, base) = mock(MAP, 0).await;
    let c = ctx_with(with_ai(base, 2)).await;
    let (t, slug, _, admin) = c.admin().await;
    opt_in(&c, &admin).await;
    let (_, e1) = c.user(t, "reviewer").await;
    let (_, e2) = c.user(t, "reviewer").await;
    let (r1, r2) = (c.mfa_token(&slug, &e1).await, c.mfa_token(&slug, &e2).await);
    let x = extract(&c, &r1, "a.csv", b"Part Number,Werkstoff\nLD-1,SUS304\n").await;
    assert_eq!(x["ai_claims_suggested"], 1, "{x:?}");
    let claims = c.get("/api/v1/claims", &r1).await.body;
    let id = claims
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["review_status"] == "ai_suggested")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let review = |tok: String, d: &'static str| {
        let c = &c;
        let id = id.clone();
        async move {
            c.post(
                &format!("/api/v1/claims/{id}/review"),
                Some(&tok),
                json!({ "decision": d, "comment": "x" }),
            )
            .await
        }
    };
    let r = review(r2.clone(), "request_evidence").await;
    assert_eq!(r.body["review_status"], "ai_suggested", "{:?}", r.body);
    let d = c.post("/api/v1/decisions", Some(&r1), decision(&id, "LD-1")).await;
    assert_eq!(d.body["error"], "claim_ai_suggested_unreviewed");
    // The database refuses the laundering transition even for a direct update.
    let mut tx = kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap();
    let e = sqlx::query("UPDATE claims SET review_status = 'needs_evidence' WHERE id = $1::uuid")
        .bind(&id)
        .execute(&mut *tx)
        .await;
    assert!(e.is_err());
    drop(tx);
    // Accepted by a second reviewer → usable; then "request evidence" withdraws it and flags the decision.
    assert_eq!(review(r2.clone(), "approve").await.body["review_status"], "accepted");
    let d = c.post("/api/v1/decisions", Some(&r1), decision(&id, "LD-1")).await;
    assert_eq!(d.status, StatusCode::OK, "{:?}", d.body);
    let dec = d.body["id"].as_str().unwrap().to_owned();
    assert_eq!(
        review(r2.clone(), "request_evidence").await.body["review_status"],
        "needs_evidence"
    );
    let st = c.get(&format!("/api/v1/decisions/{dec}"), &r1).await.body["decision"]["status"].clone();
    assert_eq!(st, "potentially_affected");
    assert_eq!(
        c.post("/api/v1/decisions", Some(&r1), decision(&id, "LD-1")).await.body["error"],
        "claim_ai_suggested_unreviewed"
    );
}

/// M2: checkpoints verify incrementally without the audit-head lock; heavy audit endpoints are rate limited.
#[tokio::test]
async fn m5_checkpoints_incremental_and_audit_endpoints_rate_limited() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let (_, aud) = c.user(t, "auditor").await;
    let at = c.token(&slug, &aud).await;
    let first = c.post("/api/v1/audit/checkpoints", Some(&at), json!({})).await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.body);
    // Audited writes while another session holds a REPEATABLE READ snapshot do not wait.
    let snap = kizuna_api::db::tenant_snapshot_tx(&c.owner, t).await.unwrap();
    let t0 = Instant::now();
    c.call(
        Method::PUT,
        "/api/v1/integrations/import-settings",
        Some(&admin),
        Some(json!({ "expected_version": 0, "settings": { "default_kind": "drawing" } })),
    )
    .await;
    assert!(t0.elapsed() < Duration::from_secs(2));
    drop(snap);
    let second = c.post("/api/v1/audit/checkpoints", Some(&at), json!({})).await;
    assert_eq!(second.status, StatusCode::OK);
    let detail: i64 = owner_count(
        &c,
        t,
        "SELECT (details->>'incremental_from')::bigint FROM audit_events WHERE action = 'audit.checkpoint' ORDER BY seq DESC LIMIT 1",
    )
    .await;
    assert_eq!(
        detail,
        first.body["seq"].as_i64().unwrap(),
        "verification resumed from the last checkpoint"
    );
    let mut codes = Vec::new();
    for _ in 0..6 {
        codes.push(c.post("/api/v1/audit/checkpoints", Some(&at), json!({})).await.status);
    }
    assert!(codes.contains(&StatusCode::TOO_MANY_REQUESTS), "{codes:?}");
    assert_eq!(
        c.get("/api/v1/audit/verify", &at).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
}

/// L1 + L3: R4 bound below the recording cap; settings changes raise security events.
#[tokio::test]
async fn thresholds_bounded_and_settings_changes_emit_security_events() {
    let c = ctx().await;
    let (t, _, _, admin) = c.admin().await;
    let put = |body: Value| {
        let c = &c;
        let a = admin.clone();
        async move {
            c.call(Method::PUT, "/api/v1/security/detection-settings", Some(&a), Some(body))
                .await
        }
    };
    let r = put(json!({ "expected_version": 0, "settings": { "thresholds": { "authz_denials": 151 } } })).await;
    assert_eq!(r.body["error"], "invalid_thresholds");
    let r = put(json!({ "expected_version": 0, "settings": { "thresholds": { "authz_denials": 150 } } })).await;
    assert_eq!(r.status, StatusCode::OK);
    c.call(
        Method::PUT,
        "/api/v1/integrations/import-settings",
        Some(&admin),
        Some(json!({ "expected_version": 0, "settings": { "default_kind": "drawing" } })),
    )
    .await;
    let weak = owner_count(
        &c,
        t,
        &format!("SELECT count(*) FROM security_events WHERE tenant_id = '{t}' AND kind = 'admin.settings_changed' AND details->>'weakening' = 'true'"),
    )
    .await;
    let all = owner_count(
        &c,
        t,
        &format!("SELECT count(*) FROM security_events WHERE tenant_id = '{t}' AND kind = 'admin.settings_changed'"),
    )
    .await;
    assert_eq!((weak, all), (1, 2));
    let diff = owner_count(
        &c,
        t,
        "SELECT count(*) FROM audit_events WHERE action = 'settings.import_updated' AND details ? 'previous' AND details ? 'settings'",
    )
    .await;
    assert_eq!(diff, 1);
}

/// L2: events that opened an incident do not raise the adaptive baseline.
#[tokio::test]
async fn adaptive_baseline_ignores_incident_events() {
    let c = ctx().await;
    let (t, _, uid, admin) = c.admin().await;
    let mut tx = kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap();
    let mut ids = Vec::new();
    for i in 0..25 {
        let id = Uuid::now_v7();
        ids.push(id.to_string());
        sqlx::query(
            "INSERT INTO security_events (id, tenant_id, kind, severity, user_id, occurred_at, details) \
             VALUES ($1, $2, 'authz.denied', 'medium', $3, now() - interval '2 days' + make_interval(secs => $4), '{}')",
        )
        .bind(id)
        .bind(t)
        .bind(uid)
        .bind(f64::from(i))
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO incidents (id, tenant_id, rule, severity, subject, summary, status, event_ids) \
         VALUES ($1, $2, 'authorization_probing', 'medium', $3, 's', 'resolved', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(t)
    .bind(format!("user:{uid}"))
    .bind(json!(ids))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let r = c
        .call(
            Method::PUT,
            "/api/v1/security/detection-settings",
            Some(&admin),
            Some(json!({ "expected_version": 0, "settings": { "mode": "adaptive" } })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let g = c.get("/api/v1/security/detection-settings", &admin).await;
    assert_eq!(g.body["active_thresholds"]["authz_denials"], 10, "{:?}", g.body);
}

/// L4: checkpoints stay verifiable after a signing-key rotation; an unknown key is not reported as tampering.
#[tokio::test]
async fn checkpoints_survive_signing_key_rotation() {
    let c = ctx().await;
    let (t, slug, _, _) = c.admin().await;
    let (_, aud) = c.user(t, "auditor").await;
    let at = c.token(&slug, &aud).await;
    assert_eq!(
        c.post("/api/v1/audit/checkpoints", Some(&at), json!({})).await.status,
        StatusCode::OK
    );
    let old = c.st.cfg.audit_signing_key.clone().unwrap();
    let rotated = ctx_with(|cfg| {
        cfg.audit_signing_key = Some(Secret::new([9u8; 32]));
        cfg.audit_signing_keys_previous = vec![old];
    })
    .await;
    let v = rotated.get("/api/v1/audit/checkpoints/verify", &at).await;
    assert_eq!(v.body["intact"], true, "{:?}", v.body);
    assert_eq!(v.body["checkpoints"][0]["key_status"], "retired");
    assert_eq!(
        rotated
            .post("/api/v1/audit/checkpoints", Some(&at), json!({}))
            .await
            .status,
        StatusCode::OK
    );
    let dropped = ctx_with(|cfg| cfg.audit_signing_key = Some(Secret::new([9u8; 32]))).await;
    let v = dropped.get("/api/v1/audit/checkpoints/verify", &at).await;
    assert_eq!(
        (v.body["intact"].clone(), v.body["unverifiable_unknown_key"].clone()),
        (json!(true), json!(1)),
        "{:?}",
        v.body
    );
    assert!(
        !c.security_kinds(t)
            .await
            .contains(&"integrity.audit_chain_broken".to_string())
    );
}

async fn seed_action(c: &Ctx, t: Uuid, kind: &str, target: &str, inc_status: &str, act_status: &str) -> (Uuid, Uuid) {
    let (inc, act) = (Uuid::now_v7(), Uuid::now_v7());
    let mut tx = kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap();
    sqlx::query(
        "INSERT INTO incidents (id, tenant_id, rule, severity, subject, summary, status) VALUES ($1,$2,'password_spraying','high',$3,'s',$4)",
    )
    .bind(inc)
    .bind(t)
    .bind(format!("x:{inc}"))
    .bind(inc_status)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO response_actions (id, tenant_id, incident_id, kind, target, policy, status) VALUES ($1,$2,$3,$4,$5,'requires_approval',$6)",
    )
    .bind(act)
    .bind(t)
    .bind(inc)
    .bind(kind)
    .bind(target)
    .bind(act_status)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (inc, act)
}

/// L5: a revert only lifts its own block; actions of closed incidents are not approvable; resolve closes proposals.
#[tokio::test]
async fn revert_only_lifts_own_block_and_closed_incident_actions_not_approvable() {
    let c = ctx().await;
    let (t, _, _, admin) = c.admin().await;
    let ip = "203.0.113.77";
    let (inc_old, old) = seed_action(&c, t, "block_ip", ip, "contained", "executed").await;
    let (_, newer) = seed_action(&c, t, "block_ip", ip, "contained", "executed").await;
    let mut tx = kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap();
    sqlx::query(
        "INSERT INTO ip_blocks (tenant_id, ip, until, action_id) VALUES ($1,$2, now() + interval '1 hour', $3)",
    )
    .bind(t)
    .bind(ip)
    .bind(newer)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let r = c
        .post(
            &format!("/api/v1/security/incidents/{inc_old}/actions/{old}/revert"),
            Some(&admin),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let still = owner_count(&c, t, &format!("SELECT count(*) FROM ip_blocks WHERE ip = '{ip}'")).await;
    assert_eq!(still, 1, "the newer block stays");
    let (_, uid_victim) = c.user(t, "viewer").await;
    let _ = uid_victim;
    let (closed, pending) = seed_action(
        &c,
        t,
        "lock_account",
        &Uuid::now_v7().to_string(),
        "false_positive",
        "proposed",
    )
    .await;
    let r = c
        .post(
            &format!("/api/v1/security/incidents/{closed}/actions/{pending}/approve"),
            Some(&admin),
            json!({}),
        )
        .await;
    assert_eq!(r.body["error"], "incident_closed");
    let (open, p2) = seed_action(&c, t, "lock_account", &Uuid::now_v7().to_string(), "open", "proposed").await;
    let r = c
        .post(
            &format!("/api/v1/security/incidents/{open}/resolve"),
            Some(&admin),
            json!({ "status": "false_positive", "comment": "test" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let st = owner_count(
        &c,
        t,
        &format!("SELECT count(*) FROM response_actions WHERE id = '{p2}' AND status = 'rejected'"),
    )
    .await;
    assert_eq!(st, 1);
}

/// L6: a slow provider used by one tenant neither blocks sandbox slots nor other tenants.
#[tokio::test]
async fn slow_ai_provider_does_not_block_other_tenants() {
    let (_m, base) = mock(MAP, 2500).await;
    let c = ctx_with(with_ai(base, 5)).await;
    let (_, _, _, a) = c.admin().await;
    let (_, _, _, b) = c.admin().await;
    opt_in(&c, &a).await;
    let a1 = extract(&c, &a, "a1.csv", b"Part Number,Werkstoff\nS-1,SUS304\n");
    let a2 = async {
        tokio::time::sleep(Duration::from_millis(600)).await;
        extract(&c, &a, "a2.csv", b"Part Number,Werkstoff\nS-2,SUS304\n").await
    };
    let bx = async {
        tokio::time::sleep(Duration::from_millis(900)).await;
        let t0 = Instant::now();
        let x = extract(&c, &b, "b.csv", b"Part Number,Material\nS-3,S45C\n").await;
        (x, t0.elapsed())
    };
    let (x1, x2, (xb, tb)) = tokio::join!(a1, a2, bx);
    assert_eq!(x1["ai_claims_suggested"], 1, "{x1:?}");
    assert!(
        x2["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!("ai_fallback:ai_busy")),
        "{x2:?}"
    );
    assert_eq!(xb["claims_created"], 1);
    assert!(tb < Duration::from_millis(2000), "other tenant waited {tb:?}");
}

/// L8 + I1: no self-release of an own quarantined upload; org reports are in the audit chain.
#[tokio::test]
async fn uploader_cannot_release_own_quarantine_and_reports_are_audited() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let pdf = b"%PDF-1.7\n1 0 obj << /OpenAction << /S /JavaScript /JS (app.alert(1)) >> >> endobj\n%%EOF\n";
    let r = c.upload(&admin, "evil.pdf", pdf, None).await;
    assert_eq!(r.body["artifact"]["status"], "quarantined");
    let id = r.body["artifact"]["id"].as_str().unwrap().to_owned();
    let own = c
        .post(
            &format!("/api/v1/artifacts/{id}/release"),
            Some(&admin),
            json!({ "reason": "reviewed the scan findings" }),
        )
        .await;
    assert_eq!(own.status, StatusCode::FORBIDDEN, "{:?}", own.body);
    let (_, e2) = c.user(t, "tenant_admin").await;
    let a2 = c.mfa_token(&slug, &e2).await;
    let ok = c
        .post(
            &format!("/api/v1/artifacts/{id}/release"),
            Some(&a2),
            json!({ "reason": "reviewed the scan findings" }),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.body);
    let rep = c.post("/api/v1/reports", Some(&admin), json!({ "scope": "all" })).await;
    assert_eq!(rep.status, StatusCode::OK);
    assert_eq!(
        owner_count(
            &c,
            t,
            "SELECT count(*) FROM audit_events WHERE action = 'export.report'"
        )
        .await,
        1
    );
}

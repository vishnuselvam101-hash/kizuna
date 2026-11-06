#![allow(clippy::unwrap_used)]
//! Invalidations above the inline bound: the change commits, a review batch applies the impact in resumable chunks,
//! and the subject is locked for new decisions until the batch is done. Own test binary: the inline bound is
//! process-wide (set from the config), here 3 decisions.
mod common;

use axum::http::StatusCode;
use common::*;
use kizuna_api::db;
use kizuna_api::review_batches::{Step, run_pending, step};
use serde_json::{Value, json};

async fn decide(c: &Ctx, tok: &str, claim: &str) -> Value {
    c.post("/api/v1/decisions", Some(tok), json!({
        "kind": "supplier_part_release", "title": "Release", "subject_type": "part", "subject_key": "BRK-100",
        "outcome": "approved", "rationale": "dimensions verified", "evidence": [{"claim_id": claim}], "depends_on": []
    }))
    .await
    .body
}

async fn status(c: &Ctx, tok: &str, id: &str) -> String {
    c.get(&format!("/api/v1/decisions/{id}"), tok).await.body["decision"]["status"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn large_invalidation_commits_and_is_applied_by_a_resumable_batch() {
    let c = ctx_with(|cfg| {
        cfg.invalidation_inline_limit = 3;
        cfg.review_batch_chunk = 2;
    })
    .await;
    let (t, slug) = c.tenant().await;
    let (_, e) = c.user(t, "reviewer").await;
    let tok = c.mfa_token(&slug, &e).await;
    let r = c
        .post(
            "/api/v1/claims",
            Some(&tok),
            json!({"subject_type":"part","subject_key":"BRK-100",
        "predicate":"mfg.dimension.length","value_num":"120","value_unit":"mm","revision":"C"}),
        )
        .await;
    let claim = r.body["id"].as_str().unwrap().to_owned();
    let mut ids = Vec::new();
    for _ in 0..7 {
        ids.push(decide(&c, &tok, &claim).await["id"].as_str().unwrap().to_owned());
    }

    let r = c
        .post(
            "/api/v1/claims",
            Some(&tok),
            json!({"subject_type":"part","subject_key":"BRK-100",
        "predicate":"mfg.material","value_text":"C45"}),
        )
        .await;
    let other = r.body["id"].as_str().unwrap().to_owned();

    // 1. The correction affects 7 > 3 decisions: it commits anyway, and a review batch is queued.
    let r = c
        .post(
            &format!("/api/v1/claims/{claim}/supersede"),
            Some(&tok),
            json!({"subject_type":"part",
        "subject_key":"BRK-100","predicate":"mfg.dimension.length","value_num":"125","value_unit":"mm","revision":"C",
        "reason":"corrected"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let batches = c.get("/api/v1/review-batches", &tok).await.body;
    assert_eq!(batches.as_array().unwrap().len(), 1);
    let bid = batches[0]["id"].as_str().unwrap().to_owned();
    assert_eq!(batches[0]["status"], "pending");
    assert_eq!(status(&c, &tok, &ids[0]).await, "valid", "not applied inline");

    // 2. Until the batch is done, no new decision may rely on the subject.
    let r = c
        .post(
            "/api/v1/decisions",
            Some(&tok),
            json!({
                "kind": "supplier_part_release", "title": "Release", "subject_type": "part", "subject_key": "brk-100",
                "outcome": "approved", "rationale": "dimensions verified", "evidence": [{"claim_id": other}], "depends_on": []
            }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{:?}", r.body);
    assert_eq!(r.body["error"], "impact_analysis_in_progress");

    // 3. Materialise, then one chunk of 2.
    let mut tx = db::tenant_tx(&c.st.pool, t).await.unwrap();
    assert!(matches!(
        step(&mut tx, t, 2).await.unwrap(),
        Step::Materialised { total: 7, .. }
    ));
    tx.commit().await.unwrap();
    let mut tx = db::tenant_tx(&c.st.pool, t).await.unwrap();
    assert!(matches!(
        step(&mut tx, t, 2).await.unwrap(),
        Step::Applied {
            items: 2,
            done: false,
            ..
        }
    ));
    tx.commit().await.unwrap();

    // 4. Crash mid-batch: a chunk runs but its transaction never commits.
    let mut tx = db::tenant_tx(&c.st.pool, t).await.unwrap();
    assert!(matches!(
        step(&mut tx, t, 2).await.unwrap(),
        Step::Applied { items: 2, .. }
    ));
    drop(tx);
    let b = c.get(&format!("/api/v1/review-batches/{bid}"), &tok).await.body;
    assert_eq!(
        (b["status"].as_str(), b["total"].as_i64(), b["processed"].as_i64()),
        (Some("running"), Some(7), Some(2))
    );
    let flagged = |n: usize| {
        let ids = ids.clone();
        let c = &c;
        let tok = &tok;
        async move {
            let mut k = 0;
            for id in &ids {
                k += usize::from(status(c, tok, id).await == "potentially_affected");
            }
            assert_eq!(k, n);
        }
    };
    flagged(2).await;

    // 5. Restart: the batch's job (queued in the creating transaction: outbox) resumes and finishes it; a second
    // run is a no-op (idempotent).
    let jc = kizuna_api::jobs::JobCtx {
        pool: c.st.pool.clone(),
        alerts: std::sync::Arc::clone(&c.st.alerts),
        metrics: std::sync::Arc::clone(&c.st.metrics),
        review_chunk: 2,
        statement_timeout_ms: 60_000,
        lease_secs: 60,
    };
    let (_, out) = kizuna_api::jobs::run_one(&jc, t, "w").await.unwrap().unwrap();
    assert_eq!(out, kizuna_api::jobs::Outcome::Done);
    assert!(kizuna_api::jobs::run_one(&jc, t, "w").await.unwrap().is_none());
    assert_eq!(run_pending(&c.st.pool, 2).await.unwrap(), 0);
    flagged(7).await;
    let b = c.get(&format!("/api/v1/review-batches/{bid}"), &tok).await.body;
    assert_eq!(
        (b["status"].as_str(), b["processed"].as_i64(), b["flagged"].as_i64()),
        (Some("done"), Some(7), Some(7))
    );
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM invalidation_events WHERE cause_claim = $1::uuid")
        .bind(&claim)
        .fetch_one(&mut *db::tenant_tx(&c.owner, t).await.unwrap())
        .await
        .unwrap();
    assert_eq!(events, 7, "each impact applied exactly once");
    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE target_id = $1 AND action LIKE 'invalidation.review_batch_%'",
    )
    .bind(&bid)
    .fetch_one(&mut *db::tenant_tx(&c.owner, t).await.unwrap())
    .await
    .unwrap();
    assert_eq!(audit, 2, "created + completed are audited");

    // 6. The subject is free again.
    assert!(decide(&c, &tok, &other).await["id"].is_string());
}

#[tokio::test]
async fn small_invalidation_stays_inline() {
    let c = ctx_with(|cfg| cfg.invalidation_inline_limit = 3).await;
    let (t, slug) = c.tenant().await;
    let (_, e) = c.user(t, "reviewer").await;
    let tok = c.mfa_token(&slug, &e).await;
    let r = c
        .post(
            "/api/v1/claims",
            Some(&tok),
            json!({"subject_type":"part","subject_key":"SHF-1",
        "predicate":"mfg.material","value_text":"C45"}),
        )
        .await;
    let claim = r.body["id"].as_str().unwrap().to_owned();
    let r = c.post("/api/v1/decisions", Some(&tok), json!({
        "kind": "supplier_part_release", "title": "Release", "subject_type": "part", "subject_key": "SHF-1",
        "outcome": "approved", "rationale": "material verified", "evidence": [{"claim_id": claim}], "depends_on": []
    })).await;
    let d = r.body["id"].as_str().unwrap().to_owned();
    let r = c
        .post(
            &format!("/api/v1/claims/{claim}/supersede"),
            Some(&tok),
            json!({"subject_type":"part",
        "subject_key":"SHF-1","predicate":"mfg.material","value_text":"S355","reason":"corrected"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(status(&c, &tok, &d).await, "potentially_affected");
    assert!(
        c.get("/api/v1/review-batches", &tok)
            .await
            .body
            .as_array()
            .unwrap()
            .is_empty()
    );
}

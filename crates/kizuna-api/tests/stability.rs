#![allow(clippy::unwrap_used)]
//! STABILITY_2026-11-06: Idempotency-Key, retry-safe jobs (leases, backoff, quarantine + incident), admission
//! queues (429 + Retry-After), per-class statement timeouts, metrics and readiness.
mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::*;
use kizuna_api::admission::{self, RouteClass, RouteCtx};
use kizuna_api::jobs::{self, JobCtx, Outcome};
use kizuna_api::{db, error::ApiError};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

fn claim_body(key: &str) -> Value {
    json!({"subject_type":"part","subject_key":key,"predicate":"mfg.material","value_text":"C45"})
}

async fn post_key(c: &Ctx, tok: &str, path: &str, key: &str, body: &Value) -> Resp {
    let req = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("authorization", format!("Bearer {tok}"))
        .header("content-type", "application/json")
        .header("idempotency-key", key)
        .body(Body::from(body.to_string()))
        .unwrap();
    c.raw(req).await
}

/// Owner-role query in the tenant's RLS scope (tables FORCE row-level security, even for the owner).
async fn count(c: &Ctx, sql: &str, t: Uuid) -> i64 {
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    let n = sqlx::query_scalar(sql).bind(t).fetch_one(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    n
}

async fn exec(c: &Ctx, t: Uuid, sql: &str, binds: &[Uuid]) {
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    let mut q = sqlx::query(sql);
    for b in binds {
        q = q.bind(*b);
    }
    q.execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn idempotency_key_replays_the_first_response_and_rejects_a_different_request() {
    let c = ctx().await;
    let (t, slug, _, tok) = c.admin().await;
    let part = format!("P-{}", Uuid::new_v4().simple());
    let claims = "SELECT count(*) FROM claims WHERE tenant_id = $1";

    let a = post_key(&c, &tok, "/api/v1/claims", "k-1", &claim_body(&part)).await;
    assert_eq!(a.status, StatusCode::OK, "{:?}", a.body);
    assert!(a.headers.get("idempotent-replayed").is_none());
    // Retry (e.g. the response was lost): same answer, no second claim.
    let b = post_key(&c, &tok, "/api/v1/claims", "k-1", &claim_body(&part)).await;
    assert_eq!(b.status, StatusCode::OK);
    assert_eq!(b.body, a.body);
    assert_eq!(b.headers["idempotent-replayed"], "true");
    assert_eq!(b.headers["content-type"], "application/json");
    assert_eq!(count(&c, claims, t).await, 1);
    // Same key, different body: refused.
    let r = post_key(&c, &tok, "/api/v1/claims", "k-1", &claim_body("OTHER")).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(r.body["error"], "idempotency_key_reused");
    // Same key on another path is a different request too.
    let r = post_key(&c, &tok, "/api/v1/verification/run", "k-1", &json!({})).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(count(&c, claims, t).await, 1);
    // Without a key nothing changes: two requests, two claims. A new key is a new request.
    assert_eq!(
        c.post("/api/v1/claims", Some(&tok), claim_body(&part)).await.status,
        StatusCode::OK
    );
    assert_eq!(
        post_key(&c, &tok, "/api/v1/claims", "k-2", &claim_body(&part))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(count(&c, claims, t).await, 3);
    // Keys are per user: another user of the tenant with the same key runs its own request.
    let (_, e2) = c.user(t, "tenant_admin").await;
    let tok2 = c.mfa_token(&slug, &e2).await;
    let r = post_key(&c, &tok2, "/api/v1/claims", "k-1", &claim_body(&part)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.headers.get("idempotent-replayed").is_none());
    assert_eq!(count(&c, claims, t).await, 4);
    // The stored body is sealed: the claim id never sits in the table in clear.
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    let bodies: Vec<Vec<u8>> =
        sqlx::query_scalar("SELECT response_body FROM idempotency_keys WHERE tenant_id = $1 AND key = 'k-1'")
            .bind(t)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    let id = a.body["id"].as_str().unwrap();
    assert_eq!(bodies.len(), 2);
    assert!(bodies.iter().all(|b| !String::from_utf8_lossy(b).contains(id)));
    // Invalid keys are refused before anything runs.
    let r = post_key(&c, &tok, "/api/v1/claims", &"x".repeat(201), &claim_body(&part)).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.body["error"], "invalid_idempotency_key");
    assert_eq!(count(&c, claims, t).await, 4);
}

#[tokio::test]
async fn idempotency_in_progress_takeover_retryable_answers_and_expiry() {
    let c = ctx().await;
    let (t, slug, uid, tok) = c.admin().await;
    let part = format!("P-{}", Uuid::new_v4().simple());
    let body = claim_body(&part);
    // A concurrent holder (row in progress, lock live): 409 + Retry-After, handler not run.
    let hash = kizuna_api::idempotency::request_hash(&Method::POST, "/api/v1/claims", body.to_string().as_bytes());
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    sqlx::query(
        "INSERT INTO idempotency_keys (tenant_id, principal_kind, principal_id, key, request_hash, state, locked_until, \
         expires_at) VALUES ($1, 'user', $2, 'busy', $3, 'in_progress', now() + interval '1 minute', now() + interval '1 day')",
    )
    .bind(t)
    .bind(uid)
    .bind(hash.to_vec())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let r = post_key(&c, &tok, "/api/v1/claims", "busy", &body).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.body["error"], "idempotency_in_progress");
    assert_eq!(r.headers["retry-after"], "1");
    assert_eq!(
        count(&c, "SELECT count(*) FROM claims WHERE tenant_id = $1", t).await,
        0
    );
    // The holder died: once its lock lapses, the same request takes the key over and runs.
    exec(
        &c,
        t,
        "UPDATE idempotency_keys SET locked_until = now() - interval '1 second' WHERE key = 'busy' AND tenant_id = $1",
        &[t],
    )
    .await;
    let r = post_key(&c, &tok, "/api/v1/claims", "busy", &body).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(
        count(&c, "SELECT count(*) FROM claims WHERE tenant_id = $1", t).await,
        1
    );

    // "Not executed" answers (here 403) release the key: after the cause is fixed, the retry runs.
    let (_, ve) = c.user(t, "viewer").await;
    let vt = c.token(&slug, &ve).await;
    let r = post_key(&c, &vt, "/api/v1/claims", "v-1", &body).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM idempotency_keys WHERE tenant_id = $1 AND key = 'v-1'",
            t
        )
        .await,
        0
    );
    // A deterministic client error is a result: stored and replayed.
    let bad = json!({"subject_type":"part"});
    let r1 = post_key(&c, &tok, "/api/v1/claims", "bad-1", &bad).await;
    assert_eq!(r1.status, StatusCode::BAD_REQUEST);
    let r2 = post_key(&c, &tok, "/api/v1/claims", "bad-1", &bad).await;
    assert_eq!(
        (r2.status, r2.headers["idempotent-replayed"].to_str().unwrap()),
        (StatusCode::BAD_REQUEST, "true")
    );

    // After 24 h the key is free again, and GC removes expired keys.
    exec(
        &c,
        t,
        "UPDATE idempotency_keys SET expires_at = now() - interval '1 second' WHERE tenant_id = $1",
        &[t],
    )
    .await;
    let r = post_key(&c, &tok, "/api/v1/claims", "busy", &claim_body("NEW")).await;
    assert_eq!(r.status, StatusCode::OK);
    let mut tx = db::tenant_tx(&c.st.pool, t).await.unwrap();
    let n = kizuna_api::idempotency::purge_expired(&mut tx, OffsetDateTime::now_utc())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(n, 1, "bad-1 expired; busy was re-claimed with a fresh expiry");
}

#[tokio::test]
async fn idempotent_upload_is_stored_once() {
    let c = ctx().await;
    let (t, _, _, tok) = c.admin().await;
    let up = |key: &'static str| {
        let boundary = "kizunaidem";
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.pdf\"\r\nContent-Type: application/pdf\r\n\r\n").as_bytes());
        body.extend_from_slice(CLEAN_PDF);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/artifacts")
            .header("authorization", format!("Bearer {tok}"))
            .header("content-type", format!("multipart/form-data; boundary={boundary}"))
            .header("idempotency-key", key)
            .body(Body::from(body))
            .unwrap()
    };
    let a = c.raw(up("u-1")).await;
    assert_eq!(a.status, StatusCode::OK, "{:?}", a.body);
    let b = c.raw(up("u-1")).await;
    assert_eq!(b.body, a.body);
    assert_eq!(b.headers["idempotent-replayed"], "true");
    let audits = count(
        &c,
        "SELECT count(*) FROM audit_events WHERE tenant_id = $1 AND action LIKE 'artifact.%'",
        t,
    )
    .await;
    let again = c.raw(up("u-1")).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM audit_events WHERE tenant_id = $1 AND action LIKE 'artifact.%'",
            t
        )
        .await,
        audits,
        "a replay writes nothing"
    );
}

fn job_ctx(c: &Ctx) -> JobCtx {
    JobCtx {
        pool: c.st.pool.clone(),
        alerts: Arc::clone(&c.st.alerts),
        metrics: Arc::clone(&c.st.metrics),
        review_chunk: 100,
        statement_timeout_ms: 60_000,
        lease_secs: 60,
    }
}

async fn enqueue(c: &Ctx, t: Uuid, kind: &str) -> i64 {
    let mut tx = db::tenant_tx(&c.st.pool, t).await.unwrap();
    let id = jobs::enqueue(&mut tx, t, kind, "", json!({}), None).await.unwrap();
    tx.commit().await.unwrap();
    id
}

#[tokio::test]
async fn failing_job_backs_off_then_is_quarantined_with_an_incident_and_can_be_requeued() {
    let c = ctx().await;
    let (t, _, _, tok) = c.admin().await;
    let jc = job_ctx(&c);
    let id = enqueue(&c, t, "test.poison").await;
    assert_eq!(
        enqueue(&c, t, "test.poison").await,
        id,
        "enqueue is deduplicated while queued"
    );
    for attempt in 1..=5 {
        let (jid, out) = jobs::run_one(&jc, t, "w1").await.unwrap().unwrap();
        assert_eq!(jid, id);
        let (state, attempts, wait): (String, i32, f64) = sqlx::query_as(
            "SELECT state, attempts, extract(epoch FROM run_after - now())::float8 FROM jobs WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&c.owner)
        .await
        .unwrap();
        assert_eq!(attempts, attempt);
        if attempt < 5 {
            assert_eq!((out, state.as_str()), (Outcome::Retry, "queued"));
            // Exponential backoff: 2^attempt seconds.
            assert!(
                (wait - 2f64.powi(attempt)).abs() < 1.5,
                "attempt {attempt}: wait {wait}"
            );
            assert!(
                jobs::run_one(&jc, t, "w1").await.unwrap().is_none(),
                "not due during backoff"
            );
            sqlx::query("UPDATE jobs SET run_after = now() WHERE id = $1")
                .bind(id)
                .execute(&c.owner)
                .await
                .unwrap();
        } else {
            assert_eq!((out, state.as_str()), (Outcome::Quarantined, "quarantined"));
        }
    }
    assert!(
        jobs::run_one(&jc, t, "w1").await.unwrap().is_none(),
        "quarantined jobs are not retried"
    );
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    let inc: (String, String, String) = sqlx::query_as(
        "SELECT rule, severity, subject FROM incidents WHERE tenant_id = $1 AND rule = 'job_quarantined'",
    )
    .bind(t)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(inc, ("job_quarantined".into(), "high".into(), "job:test.poison".into()));
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM response_actions WHERE tenant_id = $1 AND kind = 'notify_admins' AND status = 'executed' \
             AND alerted_at IS NULL",
            t
        )
        .await,
        1,
        "the alert job will deliver it"
    );
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM audit_events WHERE tenant_id = $1 AND action = 'job.quarantined'",
            t
        )
        .await,
        1
    );
    let r = c.get("/api/v1/jobs?state=quarantined", &tok).await;
    assert_eq!(r.body[0]["id"], id);
    assert_eq!(r.body[0]["last_error"], "unknown_job_kind");
    let r = c
        .post(&format!("/api/v1/jobs/{id}/requeue"), Some(&tok), json!({}))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let r = c.get(&format!("/api/v1/jobs/{id}"), &tok).await;
    assert_eq!(
        (r.body["state"].as_str(), r.body["attempts"].as_i64()),
        (Some("queued"), Some(0))
    );
    let r = c
        .post(&format!("/api/v1/jobs/{id}/requeue"), Some(&tok), json!({}))
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    // Scheduler: alert delivery is queued only when alerts are on.
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM jobs WHERE tenant_id = $1 AND kind = 'alerts.deliver'",
            t
        )
        .await,
        0
    );
    jobs::schedule_tick(&c.st.pool, false, true).await.unwrap();
    jobs::schedule_tick(&c.st.pool, false, true).await.unwrap();
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM jobs WHERE tenant_id = $1 AND kind = 'alerts.deliver'",
            t
        )
        .await,
        1
    );
}

#[tokio::test]
async fn leases_expire_and_only_the_holder_completes() {
    let c = ctx().await;
    let (t, _, _, _) = c.admin().await;
    let id = enqueue(&c, t, jobs::GC).await;
    let now = OffsetDateTime::now_utc();
    let a = jobs::claim(&c.st.pool, t, "A", now, 60).await.unwrap().unwrap();
    assert_eq!((a.id, a.attempts), (id, 1));
    assert!(
        jobs::claim(&c.st.pool, t, "B", now, 60).await.unwrap().is_none(),
        "leased"
    );
    // A dies; after the lease, B takes the job over (the takeover counts as an attempt).
    let b = jobs::claim(&c.st.pool, t, "B", now + time::Duration::seconds(61), 60)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((b.id, b.attempts), (id, 2));
    assert!(
        !jobs::complete(&c.st.pool, t, id, "A", now).await.unwrap(),
        "A lost its lease"
    );
    assert_eq!(
        jobs::fail(&c.st.pool, t, &a, "A", "late", now).await.unwrap(),
        jobs::Outcome::LeaseLost
    );
    assert!(jobs::complete(&c.st.pool, t, id, "B", now).await.unwrap());
    // A job whose workers keep dying is quarantined without running again.
    let id2 = enqueue(&c, t, jobs::GC).await;
    sqlx::query(
        "UPDATE jobs SET state = 'running', attempts = 5, lease_owner = 'dead', lease_expires_at = now() - interval '1 s' \
         WHERE id = $1",
    )
    .bind(id2)
    .execute(&c.owner)
    .await
    .unwrap();
    let (jid, out) = jobs::run_one(&job_ctx(&c), t, "C").await.unwrap().unwrap();
    assert_eq!((jid, out), (id2, Outcome::Quarantined));
}

#[tokio::test]
async fn verification_and_gc_run_as_jobs() {
    let c = ctx().await;
    let (t, _, _, tok) = c.admin().await;
    let r = c.post("/api/v1/claims", Some(&tok), claim_body("BRK-1")).await;
    assert_eq!(r.status, StatusCode::OK);
    let r = c.post("/api/v1/verification/jobs", Some(&tok), json!({})).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.body);
    let id = r.body["job_id"].as_i64().unwrap();
    let r2 = c.post("/api/v1/verification/jobs", Some(&tok), json!({})).await;
    assert_eq!(r2.body["job_id"].as_i64(), Some(id), "one queued run per tenant");
    // An old finished job and an expired key for GC.
    sqlx::query(
        "INSERT INTO jobs (tenant_id, kind, state, finished_at) VALUES ($1, 'gc.tenant', 'done', now() - interval '2 days')",
    )
    .bind(t)
    .execute(&c.owner)
    .await
    .unwrap();
    enqueue(&c, t, jobs::GC).await;
    let jc = job_ctx(&c);
    while jobs::run_one(&jc, t, "w").await.unwrap().is_some() {}
    let r = c.get(&format!("/api/v1/jobs/{id}"), &tok).await;
    assert_eq!(r.body["state"], "done", "{:?}", r.body);
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM audit_events WHERE tenant_id = $1 AND action = 'verification.run' AND actor_kind = 'system'",
            t
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM jobs WHERE tenant_id = $1 AND finished_at < now() - interval '1 day'",
            t
        )
        .await,
        0,
        "GC removed the old finished job"
    );
    // A re-run of the same job (at-least-once) is harmless: detection is idempotent.
    let id3 = enqueue(&c, t, jobs::VERIFICATION).await;
    while jobs::run_one(&jc, t, "w").await.unwrap().is_some() {}
    assert_ne!(id3, id);
    let open: i64 = count(&c, "SELECT count(*) FROM conflicts WHERE tenant_id = $1", t).await;
    let id4 = enqueue(&c, t, jobs::VERIFICATION).await;
    while jobs::run_one(&jc, t, "w").await.unwrap().is_some() {}
    assert_ne!(id4, id3);
    assert_eq!(
        count(&c, "SELECT count(*) FROM conflicts WHERE tenant_id = $1", t).await,
        open
    );
    let m = kizuna_api::metrics::render(&c.st, None).await;
    assert!(
        m.contains("kizuna_job_runs_total{kind=\"verification.run\",outcome=\"ok\"}"),
        "{m}"
    );
}

#[tokio::test]
async fn statement_timeout_follows_the_route_class() {
    let c = ctx_with(|cfg| cfg.stability.statement_timeout_ms[RouteClass::Read.index()] = 300).await;
    let (t, _, _, _) = c.admin().await;
    let ms = c.st.cfg.stability.statement_timeout(RouteClass::Read);
    let r = admission::scope(
        RouteCtx {
            class: RouteClass::Read,
            statement_timeout_ms: ms,
        },
        async {
            let mut tx = db::tenant_tx(&c.st.pool, t).await.unwrap();
            let shown: String = sqlx::query_scalar("SHOW statement_timeout")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            assert_eq!(shown, "300ms");
            let e = sqlx::query("SELECT pg_sleep(2)").execute(&mut *tx).await.unwrap_err();
            ApiError::from(e)
        },
    )
    .await;
    assert!(matches!(r, ApiError::DbTimeout), "{r:?}");
    // Outside a scope (admin tools, migrations) the server default stays.
    let mut tx = db::tenant_tx(&c.st.pool, t).await.unwrap();
    let shown: String = sqlx::query_scalar("SHOW statement_timeout")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_ne!(shown, "300ms");
}

#[tokio::test]
async fn full_admission_queues_answer_429_with_retry_after() {
    let c = ctx_with(|cfg| {
        cfg.stability.upload_gate.max_queue = 0;
        cfg.stability.extract_gate.max_queue = 0;
    })
    .await;
    let (_, _, _, tok) = c.admin().await;
    // Hold every upload permit (as in-flight uploads would).
    let mut held = Vec::new();
    for _ in 0..c.st.gates.upload.limits.permits {
        held.push(c.st.gates.upload.enter().await.unwrap());
    }
    let r = c.upload(&tok, "a.pdf", CLEAN_PDF, None).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        (r.body["error"].as_str(), r.body["gate"].as_str()),
        (Some("server_busy"), Some("upload"))
    );
    assert!(r.headers.contains_key("retry-after"));
    drop(held);
    let r = c.upload(&tok, "a.pdf", CLEAN_PDF, None).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let id = r.body["artifact"]["id"].as_str().unwrap().to_owned();
    let p1 = c.st.gates.extract.enter().await.unwrap();
    let p2 = c.st.gates.extract.enter().await.unwrap();
    let r = c
        .post(&format!("/api/v1/artifacts/{id}/extract"), Some(&tok), json!({}))
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(r.body["gate"], "extract");
    drop((p1, p2));
    // Reads are never gated.
    assert_eq!(c.get("/api/v1/artifacts", &tok).await.status, StatusCode::OK);
    let m = kizuna_api::metrics::render(&c.st, None).await;
    assert!(m.contains("kizuna_admission_rejected_total{gate=\"upload\"} 1"), "{m}");
    assert!(m.contains("kizuna_admission_rejected_total{gate=\"extract\"} 1"));
}

#[tokio::test]
async fn metrics_and_readiness() {
    let c = ctx().await;
    let (_, _, _, tok) = c.admin().await;
    assert_eq!(c.get("/api/v1/artifacts", &tok).await.status, StatusCode::OK);
    let r = c
        .raw(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "ready");
    let m = kizuna_api::metrics::render(&c.st, None).await;
    for needle in [
        "kizuna_http_requests_total{class=\"read\",method=\"GET\",status=\"2xx\"}",
        "kizuna_http_request_duration_seconds_bucket{class=\"read\",le=\"+Inf\"}",
        "kizuna_http_request_duration_seconds_count{class=\"auth\"}",
        "kizuna_db_pool_connections{pool=\"request\",state=\"idle\"}",
        "kizuna_db_pool_max_connections{pool=\"request\"}",
        "kizuna_jobs_queue_depth{state=\"queued\"}",
        "kizuna_jobs_depth_scrape_ok 1",
        "kizuna_admission_permits{gate=\"heavy\"} 3",
        "process_resident_memory_bytes ",
        "process_open_fds ",
    ] {
        assert!(m.contains(needle), "missing {needle}\n{m}");
    }
    assert!(c.st.metrics.requests_total() >= 4);
    // Draining (SIGTERM): readiness fails, liveness stays up.
    c.st.draining.store(true, Ordering::SeqCst);
    let r = c
        .raw(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
        .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.body["checks"]["draining"], true);
    let r = c
        .raw(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
        .await;
    assert_eq!(r.status, StatusCode::OK);
}

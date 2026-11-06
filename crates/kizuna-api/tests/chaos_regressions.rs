#![allow(clippy::unwrap_used)]
//! Regression tests for bugs found by the chaos/soak campaign (docs/testing/CHAOS_SOAK_2026-11-05.md).
mod common;

use std::time::Duration;

use axum::routing::post;
use common::*;
use kizuna_api::config::AiConfig;
use serde_json::json;

async fn slow_ai(sleep_ms: u64) -> String {
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        post(move || async move {
            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
            json!({ "choices": [ { "message": { "content": r#"{"mappings":[]}"# } } ] }).to_string()
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}/v1")
}

/// Session-level advisory locks held by any backend of this database for the extraction key of `artifact`.
/// Extraction leases held for `artifact` (the per-artifact extraction lock is a lease row since migration 0015).
async fn extraction_leases(c: &Ctx, tenant: uuid::Uuid, artifact: &str) -> i64 {
    let mut tx = kizuna_api::db::tenant_tx(&c.owner, tenant).await.unwrap();
    let n = sqlx::query_scalar("SELECT count(*) FROM extraction_leases WHERE artifact_id = $1::uuid")
        .bind(artifact)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    n
}

/// Chaos C1: a client that disconnects while its extraction is running (here: during a slow AI call) cancels the
/// handler. The per-artifact extraction lease must not survive the cancelled request, or the artifact answers
/// `extraction_in_progress` until the lease expires.
#[tokio::test]
async fn cancelled_extraction_releases_its_lock() {
    let base = slow_ai(4_000).await;
    let c = ctx_with(move |cfg| {
        cfg.ai = Some(AiConfig {
            base_url: base,
            model: "mock".into(),
            api_key: None,
            timeout_secs: 10,
            daily_calls: 100,
            daily_chars: 1_000_000,
            calls_per_min: 100,
        })
    })
    .await;
    let (t, slug, _, admin) = c.admin().await;
    let r = c
        .call(
            axum::http::Method::PUT,
            "/api/v1/integrations/ai-extraction",
            Some(&admin),
            Some(json!({ "expected_version": 0, "enabled": true, "accept_notice": "ai-notice@1" })),
        )
        .await;
    assert_eq!(r.status, axum::http::StatusCode::OK, "{:?}", r.body);
    let (_, email) = c.user(t, "analyst").await;
    let tok = c.token(&slug, &email).await;
    // An unmapped label ("Werkstoff") sends the records to the (slow) AI step after the sandboxed worker ran.
    let up = c
        .upload(
            &tok,
            "a.csv",
            b"Part Number,Werkstoff\nLD-1,SUS304\n",
            Some("spec_document"),
        )
        .await;
    let id = up.body["artifact"]["id"].as_str().unwrap().to_owned();
    let path = format!("/api/v1/artifacts/{id}/extract");
    let cancelled = tokio::time::timeout(Duration::from_millis(2_500), c.post(&path, Some(&tok), json!({}))).await;
    assert!(
        cancelled.is_err(),
        "extraction finished before the client gave up: {:?}",
        cancelled.map(|r| r.body)
    );
    // An immediate retry must not see the cancelled request's lease (its release runs in a background task: the
    // retry waits for it instead of answering 409 extraction_in_progress).
    let r = c.post(&path, Some(&tok), json!({})).await;
    assert_eq!(r.status, axum::http::StatusCode::OK, "{:?}", r.body);
    assert_eq!(extraction_leases(&c, t, &id).await, 0, "lease released after the retry");
}

/// A lease whose holder died (crash, no Drop) blocks extraction only until it expires.
#[tokio::test]
async fn stuck_extraction_lease_expires() {
    let c = ctx().await;
    let (t, slug, _, _) = c.admin().await;
    let (_, email) = c.user(t, "analyst").await;
    let tok = c.token(&slug, &email).await;
    let up = c
        .upload(
            &tok,
            "a.csv",
            b"Part Number,Material\nLD-1,SUS304\n",
            Some("spec_document"),
        )
        .await;
    let id = up.body["artifact"]["id"].as_str().unwrap().to_owned();
    let path = format!("/api/v1/artifacts/{id}/extract");
    let lease = |expiry: &'static str| {
        let (c, id) = (&c, id.clone());
        async move {
            let mut tx = kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap();
            sqlx::query(&format!(
                "INSERT INTO extraction_leases (tenant_id, artifact_id, holder, expires_at) VALUES ($1, $2::uuid, $3, {expiry}) \
                 ON CONFLICT (tenant_id, artifact_id) DO UPDATE SET expires_at = EXCLUDED.expires_at"
            ))
            .bind(t)
            .bind(&id)
            .bind(uuid::Uuid::now_v7())
            .execute(&mut *tx)
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }
    };
    lease("now() + interval '1 hour'").await;
    let r = c.post(&path, Some(&tok), json!({})).await;
    assert_eq!(r.status, axum::http::StatusCode::CONFLICT, "{:?}", r.body);
    assert_eq!(r.body["error"], "extraction_in_progress");
    // A lease being released (as a cancelled request's Drop does, from a background task) is waited for: the
    // retry succeeds instead of answering 409. Deterministic form of the intermittent C1 race.
    let (owner, id2) = (c.owner.clone(), id.clone());
    let releaser = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut tx = kizuna_api::db::tenant_tx(&owner, t).await.unwrap();
        sqlx::query("DELETE FROM extraction_leases WHERE artifact_id = $1::uuid")
            .bind(&id2)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    });
    let r = c.post(&path, Some(&tok), json!({})).await;
    releaser.await.unwrap();
    assert_eq!(r.status, axum::http::StatusCode::OK, "{:?}", r.body);
    // Now extracted: the remaining checks go through re-extraction (same lease).
    let re = format!("/api/v1/artifacts/{id}/reextract");
    lease("now() + interval '1 hour'").await;
    let r = c.post(&re, Some(&tok), json!({"reason": "lease test"})).await;
    assert_eq!(r.body["error"], "extraction_in_progress", "{:?}", r.body);
    lease("now() - interval '1 second'").await;
    let r = c.post(&re, Some(&tok), json!({"reason": "lease test"})).await;
    assert_eq!(r.status, axum::http::StatusCode::OK, "{:?}", r.body);
    assert_eq!(extraction_leases(&c, t, &id).await, 0);
}

/// Chaos C2: systemd stops services with SIGTERM. The server must drain in-flight requests and exit 0 on SIGTERM,
/// as it does on Ctrl-C, instead of dying mid-request (an upload cut off by a restart or deploy).
#[test]
fn sigterm_drains_in_flight_requests_and_exits_cleanly() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let storage = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("sigterm-{}", uuid::Uuid::now_v7()));
    let key = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(kizuna_api::crypto::random_bytes::<32>())
    };
    // target/<profile>/deps/<this test> -> target/<profile>/kizuna-server (built by `cargo test` with the tests).
    let exe = option_env!("CARGO_BIN_EXE_kizuna-server")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let me = std::env::current_exe().unwrap();
            me.parent().unwrap().parent().unwrap().join("kizuna-server")
        });
    let mut child = std::process::Command::new(exe)
        .env_clear()
        .env("KIZUNA_DATABASE_URL", common::env("KIZUNA_TEST_DATABASE_URL"))
        .env("KIZUNA_DATA_KEY", key)
        .env("KIZUNA_BIND", format!("127.0.0.1:{port}"))
        .env("KIZUNA_STORAGE_DIR", &storage)
        .env("KIZUNA_ALLOW_ROOT", "1")
        .env("KIZUNA_DEV", "1")
        .env("KIZUNA_LOG", "error")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut up = false;
    for _ in 0..100 {
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
            let _ = s.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
            let mut r = String::new();
            let _ = s.read_to_string(&mut r);
            if r.starts_with("HTTP/1.1 200") {
                up = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(up, "server did not start");
    // A request whose body is still arriving when SIGTERM is delivered.
    let body = br#"{"tenant":"no-such-tenant","email":"x@example.test","password":"not the password"}"#;
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "POST /api/v1/auth/login HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    s.write_all(&body[..10]).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let st = std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(st.success());
    std::thread::sleep(Duration::from_millis(300));
    let _ = s.write_all(&body[10..]);
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    let mut status = None;
    for _ in 0..100 {
        if let Some(x) = child.try_wait().unwrap() {
            status = Some(x);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let status = status.unwrap_or_else(|| {
        let _ = child.kill();
        panic!("server still running 10 s after SIGTERM")
    });
    let _ = std::fs::remove_dir_all(&storage);
    assert!(resp.starts_with("HTTP/1.1 "), "in-flight request was cut off: {resp:?}");
    assert!(
        status.success(),
        "server did not shut down cleanly on SIGTERM: {status:?}"
    );
}

/// Chaos C3: while the virus scanner is down every upload is quarantined (fail closed, correct). Those quarantines
/// are an outage, not abuse: they must not count toward the `upload_abuse` rule, whose pre-authorised containment
/// would otherwise restrict every user who uploaded during the outage.
#[tokio::test]
async fn scanner_outage_quarantines_do_not_restrict_uploaders() {
    use kizuna_api::antivirus::{Clamd, MalwareScanner};
    let mut c = ctx().await;
    let gone = std::env::temp_dir().join(format!("kz-nothing-{}.sock", uuid::Uuid::now_v7().simple()));
    c.st.av = std::sync::Arc::new(Clamd {
        socket: gone,
        timeout: Duration::from_secs(1),
        max_bytes: 1024 * 1024,
    }) as std::sync::Arc<dyn MalwareScanner>;
    c.app = kizuna_api::router(c.st.clone());
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "analyst").await;
    let tok = c.token(&slug, &email).await;
    for i in 0..7 {
        let r = c
            .upload(&tok, "ok.csv", format!("part,qty\nOUT-{i},1\n").as_bytes(), None)
            .await;
        assert_eq!(r.body["artifact"]["status"], "quarantined", "{:?}", r.body);
        kizuna_api::incidents::run_detection(&c.st.pool, t).await.unwrap();
    }
    let kinds = c.security_kinds(t).await;
    assert!(kinds.iter().any(|k| k == "ingest.scanner_unavailable"), "{kinds:?}");
    let r = c.upload(&tok, "ok.csv", b"part,qty\nOUT-9,1\n", None).await;
    assert_eq!(r.status, axum::http::StatusCode::OK, "{:?}", r.body);
}

/// Chaos C6 (clock jumps, no bug found): the application clock jumps forward past token expiry and then backwards a
/// day relative to the database clock. Expired tokens are refused, fresh logins work, uploads and extraction keep
/// working after the backward jump, and the audit chain stays intact (it orders by sequence, not by time).
#[tokio::test]
async fn clock_jumps_keep_auth_safe_and_audit_intact() {
    use std::sync::Arc;
    let mut c = ctx().await;
    let clock = Arc::new(kizuna_api::clock::OffsetClock::default());
    c.st.clock = clock.clone();
    c.app = kizuna_api::router(c.st.clone());
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "analyst").await;
    let (_, aud) = c.user(t, "auditor").await;
    let tok = c.token(&slug, &email).await;
    clock.advance(time::Duration::days(2));
    assert_eq!(
        c.get("/api/v1/artifacts", &tok).await.status,
        axum::http::StatusCode::UNAUTHORIZED
    );
    let tok = c.token(&slug, &email).await;
    clock.advance(time::Duration::days(-3));
    assert_eq!(
        c.get("/api/v1/artifacts", &tok).await.status,
        axum::http::StatusCode::OK
    );
    let up = c
        .upload(
            &tok,
            "j.csv",
            b"PO Number,Part No,Qty\nPO-7,CLK-1,3\n",
            Some("purchase_order"),
        )
        .await;
    let id = up.body["artifact"]["id"].as_str().unwrap().to_owned();
    let x = c
        .post(&format!("/api/v1/artifacts/{id}/extract"), Some(&tok), json!({}))
        .await;
    assert_eq!(x.status, axum::http::StatusCode::OK, "{:?}", x.body);
    let a = c.token(&slug, &aud).await;
    let v = c.get("/api/v1/audit/verify", &a).await;
    assert_eq!(v.body["intact"], true, "{:?}", v.body);
}

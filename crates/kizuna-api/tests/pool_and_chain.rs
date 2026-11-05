#![allow(clippy::unwrap_used)]
//! Extraction must not pin pooled connections (8 concurrent extractions on a pool of 2), and checkpoints are chained.
mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

#[tokio::test]
async fn eight_concurrent_extractions_on_a_pool_of_two() {
    let mut c = ctx().await;
    let url = std::env::var("KIZUNA_TEST_DATABASE_URL").unwrap_or_else(|_| c.st.cfg.database_url.expose().clone());
    let pool = kizuna_api::db::connect(&url, 2).await.unwrap();
    c.st = kizuna_api::AppState::new(pool, c.st.cfg.as_ref().clone());
    c.app = kizuna_api::router(c.st.clone());
    let (_, _, _, admin) = c.admin().await;
    let mut ids = Vec::new();
    for i in 0..7 {
        let r = c
            .upload(
                &admin,
                &format!("p{i}.csv"),
                format!("Part Number,Material\nPOOL-{i},S45C\n").as_bytes(),
                Some("spec_document"),
            )
            .await;
        ids.push(r.body["artifact"]["id"].as_str().unwrap().to_owned());
    }
    ids.push(ids[0].clone()); // the same artifact twice at once
    let runs = ids.iter().map(|id| {
        let c = &c;
        let admin = admin.clone();
        async move {
            c.post(&format!("/api/v1/artifacts/{id}/extract"), Some(&admin), json!({}))
                .await
        }
    });
    let out = futures_join_all(runs).await;
    let ok = out.iter().filter(|r| r.status == StatusCode::OK).count();
    let busy = out
        .iter()
        .filter(|r| r.body["error"] == "extraction_in_progress" || r.body["error"] == "already_extracted")
        .count();
    assert_eq!(
        (ok, busy),
        (7, 1),
        "{:?}",
        out.iter().map(|r| (r.status, r.body.clone())).collect::<Vec<_>>()
    );
}

async fn futures_join_all<F: std::future::Future>(fs: impl Iterator<Item = F>) -> Vec<F::Output> {
    let handles: Vec<_> = fs.collect();
    let mut out = Vec::new();
    // Poll all concurrently on this task.
    let mut set = Vec::new();
    for f in handles {
        set.push(Box::pin(f));
    }
    let results = futures_util_join(set).await;
    out.extend(results);
    out
}

async fn futures_util_join<F: std::future::Future + Unpin>(mut fs: Vec<F>) -> Vec<F::Output> {
    use std::task::Poll;
    let mut done: Vec<Option<F::Output>> = (0..fs.len()).map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (i, f) in fs.iter_mut().enumerate() {
            if done[i].is_none() {
                match std::pin::Pin::new(f).poll(cx) {
                    Poll::Ready(v) => done[i] = Some(v),
                    Poll::Pending => pending = true,
                }
            }
        }
        if pending { Poll::Pending } else { Poll::Ready(()) }
    })
    .await;
    done.into_iter().map(|d| d.unwrap()).collect()
}

#[tokio::test]
async fn checkpoints_are_chained_and_a_removed_link_is_detected() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let (_, aud) = c.user(t, "auditor").await;
    let at = c.token(&slug, &aud).await;
    let first = c.post("/api/v1/audit/checkpoints", Some(&admin), json!({})).await.body;
    let second = c.post("/api/v1/audit/checkpoints", Some(&admin), json!({})).await.body;
    assert_eq!(first["prev_signature_sha256"], "00".repeat(32));
    let sig1 = hex::decode(first["signature"].as_str().unwrap()).unwrap();
    assert_eq!(
        second["prev_signature_sha256"],
        hex::encode(kizuna_api::crypto::sha256(&sig1))
    );
    // The exported second checkpoint verifies externally (KZCP2 message).
    let ext = c
        .post("/api/v1/audit/checkpoints/verify-external", Some(&at), second.clone())
        .await;
    assert_eq!(ext.body["intact"], true, "{:?}", ext.body);
    // Tampering with the link fields is a bad signature.
    let mut forged = second.clone();
    forged["prev_signature_sha256"] = json!("11".repeat(32));
    assert_eq!(
        c.post("/api/v1/audit/checkpoints/verify-external", Some(&at), forged)
            .await
            .body["error"],
        "invalid_checkpoint_signature"
    );
    // A DB owner deletes the first checkpoint: the chain of checkpoints no longer links.
    let mut tx = kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap();
    sqlx::query("ALTER TABLE audit_checkpoints DISABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM audit_checkpoints WHERE seq = $1")
        .bind(first["seq"].as_i64().unwrap())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE audit_checkpoints ENABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let v = c.get("/api/v1/audit/checkpoints/verify", &at).await;
    assert_eq!(
        (v.body["intact"].clone(), v.body["checkpoint_links_intact"].clone()),
        (json!(false), json!(false)),
        "{:?}",
        v.body
    );
    let r = c.post("/api/v1/audit/checkpoints", Some(&at), json!({})).await;
    assert_eq!(r.body["error"], "audit_chain_not_intact");
}

#[tokio::test]
async fn tenant_timezone_is_validated_audited_and_used() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let g = c.get("/api/v1/settings/timezone", &admin).await;
    assert_eq!(
        (g.body["timezone"].clone(), g.body["version"].clone()),
        (json!("Asia/Tokyo"), json!(0))
    );
    let put = |tok: String, tz: &str| {
        let c = &c;
        let body = json!({ "expected_version": 0, "timezone": tz });
        async move {
            c.call(
                axum::http::Method::PUT,
                "/api/v1/settings/timezone",
                Some(&tok),
                Some(body),
            )
            .await
        }
    };
    for bad in ["Mars/Olympus", "../etc/passwd", "", "Asia/Tokyo; DROP TABLE x"] {
        assert_eq!(put(admin.clone(), bad).await.body["error"], "invalid_timezone", "{bad}");
    }
    let (_, rev) = c.user(t, "reviewer").await;
    let rt = c.mfa_token(&slug, &rev).await;
    assert_eq!(put(rt, "UTC").await.status, StatusCode::FORBIDDEN);
    let r = put(admin.clone(), "America/New_York").await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    // Report dates follow it (DST-aware: 5 Nov 2026 is EST, UTC-5).
    let rep = c
        .post(
            "/api/v1/reports",
            Some(&admin),
            json!({ "from_date": "2026-11-05", "to_date": "2026-11-05" }),
        )
        .await;
    assert_eq!(rep.body["range"]["timezone"], "America/New_York");
    assert_eq!(rep.body["range"]["from"], 1_793_854_800); // 2026-11-05T05:00:00Z
    let mut tx = kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = 'settings.general_updated'")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(n, 1);
}

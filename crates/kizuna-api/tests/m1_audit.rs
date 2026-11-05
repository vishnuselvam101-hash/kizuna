#![allow(clippy::unwrap_used)]
mod common;

use axum::http::StatusCode;
use common::*;
use kizuna_api::audit::{self, Record};
use kizuna_api::db;
use serde_json::json;

#[tokio::test]
async fn actions_are_audited_and_chain_verifies() {
    let c = ctx().await;
    let (_, _, _, tok) = c.admin().await;
    let ev = c.get("/api/v1/audit/events", &tok).await;
    let actions: Vec<&str> = ev
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["auth.login", "auth.mfa_enrolled", "auth.login"]);
    let v = c.get("/api/v1/audit/verify", &tok).await;
    assert_eq!(v.body["intact"], true, "{:?}", v.body);
    assert_eq!(v.body["events_checked"], 3);
}

#[tokio::test]
async fn tampering_is_detected() {
    let c = ctx().await;
    let (t, _, _, tok) = c.admin().await;
    // Owner (not runtime) can disable the append-only trigger: simulate a malicious DBA edit.
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    sqlx::query("ALTER TABLE audit_events DISABLE TRIGGER audit_events_append_only")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE audit_events SET details = '{\"mfa\": false}' WHERE tenant_id = $1 AND seq = 1")
        .bind(t)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE audit_events ENABLE TRIGGER audit_events_append_only")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let v = c.get("/api/v1/audit/verify", &tok).await;
    assert_eq!(v.status, StatusCode::OK);
    assert_eq!(v.body["intact"], false);
    assert!(
        v.body["problem"].as_str().unwrap().contains("HashMismatch"),
        "{:?}",
        v.body
    );
    assert!(
        c.security_kinds(t)
            .await
            .contains(&"integrity.audit_chain_broken".to_string())
    );
}

#[tokio::test]
async fn tail_truncation_is_detected_via_head() {
    let c = ctx().await;
    let (t, _, _, tok) = c.admin().await;
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    sqlx::query("ALTER TABLE audit_events DISABLE TRIGGER audit_events_append_only")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM audit_events WHERE tenant_id = $1 AND seq = 3")
        .bind(t)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE audit_events ENABLE TRIGGER audit_events_append_only")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let v = c.get("/api/v1/audit/verify", &tok).await;
    assert_eq!(v.body["intact"], false);
    assert_eq!(v.body["problem"], "HeadMismatch");
}

#[tokio::test]
async fn append_only_even_for_owner_without_disabling_trigger() {
    let c = ctx().await;
    let (t, _, _, _) = c.admin().await;
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    let e = sqlx::query("UPDATE audit_events SET action = 'x' WHERE tenant_id = $1")
        .bind(t)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("append-only"), "{e}");
}

#[tokio::test]
async fn concurrent_appends_form_one_chain() {
    let c = ctx().await;
    let (t, _) = c.tenant().await;
    let mut handles = Vec::new();
    for i in 0..24 {
        let pool = c.st.pool.clone();
        handles.push(tokio::spawn(async move {
            let mut tx = db::tenant_tx(&pool, t).await.unwrap();
            audit::append(
                &mut tx,
                t,
                Record {
                    actor_user_id: None,
                    actor_kind: "system",
                    action: "test.concurrent",
                    target_type: "test",
                    target_id: Some(i.to_string()),
                    details: json!({ "i": i }),
                },
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let mut tx = db::tenant_tx(&c.st.pool, t).await.unwrap();
    let s = audit::verify_tenant(&mut tx, t).await.unwrap();
    assert!(s.intact, "{:?}", s.problem);
    assert_eq!(s.events_checked, 24);
}

#[tokio::test]
async fn float_details_rejected_not_stored() {
    let c = ctx().await;
    let (t, _) = c.tenant().await;
    let mut tx = db::tenant_tx(&c.st.pool, t).await.unwrap();
    let r = audit::append(
        &mut tx,
        t,
        Record {
            actor_user_id: None,
            actor_kind: "system",
            action: "x",
            target_type: "x",
            target_id: None,
            details: json!({ "f": 1.5 }),
        },
    )
    .await;
    assert!(r.is_err());
}

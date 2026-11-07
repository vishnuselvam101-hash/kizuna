#![allow(clippy::unwrap_used)]
mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{Value, json};

fn typo_pair(body: &Value, a: &str, b: &str) -> bool {
    body["candidates"].as_array().unwrap().iter().any(|c| {
        let (l, r) = (c["left"].as_str().unwrap(), c["right"].as_str().unwrap());
        ((l == a && r == b) || (l == b && r == a))
            && c["outcome"] == "requires_human_review"
            && c["reasons"][0] == "possible_typo_or_different_part"
    })
}

#[tokio::test]
async fn typo_review_mode_is_a_tenant_setting_and_strict_uses_context() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let (_, rev) = c.user(t, "reviewer").await;
    let tok = c.mfa_token(&slug, &rev).await;
    // A sibling pair (own drawing, size and supplier) and a typo pair (same supplier, drawing, material).
    c.ingest(
        &tok,
        "po.csv",
        "PO Number,Part No,Part Name,Supplier,Drawing No,Material\n\
         PO-1,BRK-10234,BRACKET L50,Tokai Seiki,D-1,SS400\n\
         PO-1,BRK-10235,BRACKET L80,Osaka Kinzoku,D-2,SPCC\n\
         PO-1,SHF-77810,SHAFT 12,Tokai Seiki,D-9,S45C\n\
         PO-2,SHF-77811,SHAFT 12,Tokai Seiki,D-9,S45C\n"
            .as_bytes(),
        "purchase_order",
    )
    .await;

    let g = c.get("/api/v1/settings/entity-resolution", &admin).await;
    assert_eq!(
        (g.body["typo_review"].clone(), g.body["version"].clone()),
        (json!("strict"), json!(0)),
        "{:?}",
        g.body
    );
    let strict = c.get("/api/v1/entities/candidates", &tok).await;
    assert_eq!(strict.body["stats"]["typo_review"], "strict", "{:?}", strict.body);
    assert!(typo_pair(&strict.body, "SHF-77810", "SHF-77811"), "{:?}", strict.body);
    assert!(!typo_pair(&strict.body, "BRK-10234", "BRK-10235"), "{:?}", strict.body);
    assert_eq!(strict.body["stats"]["typo_left_to_fs"], 1);
    // ER v3 review order: stratified active learning, advisory probability on every pending pair.
    assert_eq!(strict.body["stats"]["review_order"], "stratified_active_learning");
    assert_eq!(strict.body["stats"]["decisions_used"], 0);
    let pending = strict.body["candidates"].as_array().unwrap();
    assert!(!pending.is_empty());
    assert!(pending.iter().all(|c| {
        c["suggested_p_match"]
            .as_f64()
            .is_some_and(|p| (0.0..=1.0).contains(&p))
    }));

    let put = |tok: String, mode: &str, v: i64| {
        let c = &c;
        let body = json!({ "expected_version": v, "typo_review": mode });
        async move {
            c.call(
                Method::PUT,
                "/api/v1/settings/entity-resolution",
                Some(&tok),
                Some(body),
            )
            .await
        }
    };
    assert_eq!(
        put(admin.clone(), "loose", 0).await.body["error"],
        "invalid_typo_review"
    );
    assert_eq!(put(tok.clone(), "broad", 0).await.status, StatusCode::FORBIDDEN);
    let r = put(admin.clone(), "broad", 0).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(
        put(admin.clone(), "off", 0).await.body["error"],
        "settings_version_conflict"
    );
    let broad = c.get("/api/v1/entities/candidates", &tok).await;
    assert!(typo_pair(&broad.body, "BRK-10234", "BRK-10235"), "{:?}", broad.body);
    assert!(typo_pair(&broad.body, "SHF-77810", "SHF-77811"));
    assert_eq!(put(admin.clone(), "off", 1).await.status, StatusCode::OK);
    let off = c.get("/api/v1/entities/candidates", &tok).await;
    assert!(!typo_pair(&off.body, "SHF-77810", "SHF-77811"), "{:?}", off.body);
    // Never merged automatically in any mode: v2 lists only non-safe pairs for a human decision.
    for b in [&strict.body, &broad.body, &off.body] {
        assert!(b["candidates"].as_array().unwrap().iter().all(|c| c["safe"] == false));
    }

    // Audited, and broad → off is flagged as weakening.
    let mut tx = kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap();
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = 'settings.entity_resolution_updated'")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let weak: Vec<Value> = sqlx::query_scalar(
        "SELECT details FROM security_events WHERE tenant_id = $1 AND kind = 'admin.settings_changed' AND details->>'settings' = 'entity_resolution' ORDER BY occurred_at",
    )
    .bind(t)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(n, 2);
    assert_eq!(
        weak.iter().map(|d| d["weakening"].clone()).collect::<Vec<_>>(),
        vec![json!(false), json!(true)]
    );

    // A human decision feeds the order model; it is recorded, not applied, and the pair leaves the queue.
    let first = off.body["candidates"][0].clone();
    let d = c
        .post(
            "/api/v1/entities/decisions",
            Some(&tok),
            json!({"subject_type": "part", "left_key": first["left"], "right_key": first["right"], "outcome": "different"}),
        )
        .await;
    assert_eq!(d.status, StatusCode::OK, "{:?}", d.body);
    let after = c.get("/api/v1/entities/candidates", &tok).await;
    assert_eq!(after.body["stats"]["decisions_used"], 1, "{:?}", after.body);
    assert!(
        after.body["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["left"] != first["left"] || x["right"] != first["right"])
    );
}

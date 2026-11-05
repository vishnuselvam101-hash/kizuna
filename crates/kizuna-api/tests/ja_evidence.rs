//! Japanese evidence fidelity: a low-confidence OCR value (source_location.review = needs_confirmation) cannot
//! back a decision until a person accepted it — in the API and, for any writer, in the database (0017).

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};

fn decision(claim: &str) -> Value {
    json!({ "kind": "supplier_part_release", "title": "t", "subject_type": "part", "subject_key": "KZ-1",
            "outcome": "approved", "rationale": "r", "evidence": [ { "claim_id": claim, "role": "basis" } ], "depends_on": [] })
}

#[tokio::test]
async fn unconfirmed_low_confidence_ocr_claim_cannot_back_a_decision() {
    let c = ctx().await;
    let (t, slug, _, _admin) = c.admin().await;
    let (_, e1) = c.user(t, "reviewer").await;
    let (_, e2) = c.user(t, "reviewer").await;
    let (r1, r2) = (c.mfa_token(&slug, &e1).await, c.mfa_token(&slug, &e2).await);
    let claim = |loc: Value| {
        let (c, r1) = (&c, r1.clone());
        async move {
            let r = c
                .post(
                    "/api/v1/claims",
                    Some(&r1),
                    json!({ "subject_type": "part", "subject_key": "KZ-1", "predicate": "qa.property.tensile_strength",
                            "value_num": "440", "value_unit": "MPa", "source_location": loc }),
                )
                .await;
            assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
            r.body["id"].as_str().unwrap().to_owned()
        }
    };
    // 和暦 original and a confident OCR value: usable as is.
    let ok = claim(json!({"page": 1, "bbox_px": [10, 20, 90, 38], "ocr_conf_bps": 9100, "original": "４４０"})).await;
    let d = c.post("/api/v1/decisions", Some(&r1), decision(&ok)).await;
    assert_eq!(d.status, StatusCode::OK, "{:?}", d.body);

    let low =
        claim(json!({"page": 1, "bbox_px": [10, 40, 90, 58], "ocr_conf_bps": 6200, "review": "needs_confirmation"}))
            .await;
    let d = c.post("/api/v1/decisions", Some(&r1), decision(&low)).await;
    assert_eq!(d.body["error"], "claim_needs_confirmation", "{:?}", d.body);

    // The database refuses the link even when the API check is bypassed.
    let dec = c.post("/api/v1/decisions", Some(&r1), decision(&ok)).await.body["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut tx = kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap();
    let e = sqlx::query(
        "INSERT INTO decision_dependencies (id, tenant_id, decision_id, depends_on_claim, role, subject_key, \
         subject_type, materiality, snapshot) VALUES (gen_random_uuid(), $1, $2::uuid, $3::uuid, 'basis', 'KZ-1', \
         'part', '{}', '{}')",
    )
    .bind(t)
    .bind(&dec)
    .bind(&low)
    .execute(&mut *tx)
    .await;
    assert!(e.unwrap_err().to_string().contains("needs confirmation"));
    drop(tx);

    // A second person confirms the value: now it may back the decision.
    let r = c
        .post(
            &format!("/api/v1/claims/{low}/review"),
            Some(&r2),
            json!({ "decision": "approve", "comment": "checked against the paper original" }),
        )
        .await;
    assert_eq!(r.body["review_status"], "accepted", "{:?}", r.body);
    let d = c.post("/api/v1/decisions", Some(&r1), decision(&low)).await;
    assert_eq!(d.status, StatusCode::OK, "{:?}", d.body);
}

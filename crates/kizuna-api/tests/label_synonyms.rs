//! Fallback label mapping and tenant-scoped synonym learning (mfg-map@5, migration 0025): an unknown label is
//! mapped by a guess that needs confirmation; a reviewer's acceptance teaches that tenant (only) the synonym; a
//! revoked synonym is no longer used.
#![allow(clippy::unwrap_used)]
mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};

fn report(part: &str) -> Vec<u8> {
    make_pdf(&[
        "INSPECTION REPORT",
        &format!("Item code: {part}"),
        "Item name: Bracket",
        "Inspected on: 2025-04-01",
    ])
}

async fn part_name(c: &Ctx, tok: &str, artifact: &str) -> Value {
    let r = c.get(&format!("/api/v1/claims?artifact_id={artifact}"), tok).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["predicate"] == "qa.part_name")
        .cloned()
        .unwrap_or_else(|| panic!("no part name in {:?}", r.body))
}

async fn synonyms(c: &Ctx, tok: &str) -> Vec<Value> {
    let r = c.get("/api/v1/label-synonyms", tok).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body.as_array().unwrap().clone()
}

#[tokio::test]
async fn accepted_fallback_mappings_become_tenant_synonyms_and_can_be_revoked() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let (_, up) = c.user(t, "reviewer").await;
    let (_, rv) = c.user(t, "reviewer").await;
    let up = c.token(&slug, &up).await;
    let rv = c.mfa_token(&slug, &rv).await;

    // 1. Unknown labels: mapped by fallback, flagged for confirmation, lower confidence.
    let a1 = c.ingest(&up, "r1.pdf", &report("A12-3345"), "inspection_report").await;
    let pn = part_name(&c, &up, &a1).await;
    assert_eq!(pn["subject_key"], "A12-3345");
    assert_eq!(pn["value_text"], "BRACKET");
    let loc = &pn["source_location"];
    assert_eq!(loc["label_basis"], "fallback", "{loc}");
    assert_eq!(loc["subject_basis"], "fallback", "{loc}");
    assert_eq!(loc["review"], "needs_confirmation", "{loc}");
    assert!(pn["confidence_bps"].as_i64().unwrap() <= 6000);
    assert!(synonyms(&c, &up).await.is_empty());

    // 2. A second person accepts it: the tenant learns both labels (audited).
    let id = pn["id"].as_str().unwrap();
    let r = c
        .post(
            &format!("/api/v1/claims/{id}/review"),
            Some(&rv),
            json!({ "decision": "approve", "comment": "matches the paper" }),
        )
        .await;
    assert_eq!(r.body["review_status"], "accepted", "{:?}", r.body);
    let syn = synonyms(&c, &up).await;
    let mut pairs: Vec<(String, String)> = syn
        .iter()
        .map(|s| {
            (
                s["label"].as_str().unwrap().into(),
                s["target"].as_str().unwrap().into(),
            )
        })
        .collect();
    pairs.sort();
    assert_eq!(
        pairs,
        vec![
            ("item code".into(), "part number".into()),
            ("item name".into(), "part name".into())
        ]
    );
    let audit = c.get("/api/v1/audit/events", &admin).await;
    let learnt = audit
        .body
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["action"] == "label_synonym.learn")
        .count();
    assert_eq!(learnt, 2);

    // 3. Next document of the same tenant: mapped by the synonyms, no confirmation needed for the mapping.
    let a2 = c.ingest(&up, "r2.pdf", &report("A12-9999"), "inspection_report").await;
    let pn2 = part_name(&c, &up, &a2).await;
    assert_eq!(pn2["subject_key"], "A12-9999");
    assert_eq!(
        pn2["source_location"]["label_basis"], "tenant_synonym",
        "{}",
        pn2["source_location"]
    );
    assert!(pn2["source_location"].get("subject_basis").is_none());
    assert!(
        pn2["source_location"].get("review").is_none(),
        "{}",
        pn2["source_location"]
    );

    // 4. Never across tenants: another tenant sees no synonyms and still gets the fallback.
    let (t2, slug2) = c.tenant().await;
    let (_, other) = c.user(t2, "reviewer").await;
    let other = c.mfa_token(&slug2, &other).await;
    assert!(synonyms(&c, &other).await.is_empty());
    let b1 = c
        .ingest(&other, "r1.pdf", &report("A12-3345"), "inspection_report")
        .await;
    assert_eq!(
        part_name(&c, &other, &b1).await["source_location"]["label_basis"],
        "fallback"
    );
    let sid = syn[0]["id"].as_str().unwrap();
    let r = c
        .post(&format!("/api/v1/label-synonyms/{sid}/revoke"), Some(&other), json!({}))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "another tenant's synonym is invisible");

    // 5. Revoking: the synonym stays listed (with who/when) and is no longer used.
    let code = syn.iter().find(|s| s["label"] == "item code").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = c
        .post(
            &format!("/api/v1/label-synonyms/{code}/revoke"),
            Some(&rv),
            json!({ "reason": "wrong field" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert!(r.body["revoked_at"].is_string());
    let again = c
        .post(&format!("/api/v1/label-synonyms/{code}/revoke"), Some(&rv), json!({}))
        .await;
    assert_eq!(again.body["error"], "synonym_already_revoked");
    let a3 = c.ingest(&up, "r3.pdf", &report("A12-7777"), "inspection_report").await;
    let pn3 = part_name(&c, &up, &a3).await;
    assert_eq!(
        pn3["source_location"]["label_basis"], "tenant_synonym",
        "item name is still learnt"
    );
    assert_eq!(
        pn3["source_location"]["subject_basis"], "fallback",
        "item code is guessed again"
    );
    assert_eq!(pn3["source_location"]["review"], "needs_confirmation");
    let audit = c.get("/api/v1/audit/events", &admin).await;
    assert!(
        audit
            .body
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "label_synonym.revoke")
    );
}

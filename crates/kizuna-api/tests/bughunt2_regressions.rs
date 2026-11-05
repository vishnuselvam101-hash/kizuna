#![allow(clippy::unwrap_used)]
//! Regression tests for the second bughunt batch (tenant calendar, claim identity, key normalisation, typed
//! decision links, report date ranges). CSV encoding / cell size are covered in the Python worker tests.
mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use common::*;
use kizuna_api::clock::OffsetClock;
use kizuna_api::router;
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};

async fn claim(c: &Ctx, tok: &str, body: Value) -> Value {
    let r = c.post("/api/v1/claims", Some(tok), body).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body
}

#[tokio::test]
async fn expiry_uses_injected_clock_and_tenant_calendar() {
    let mut c = ctx().await;
    let clock = Arc::new(OffsetClock::default());
    c.st.clock = clock.clone();
    c.app = router(c.st.clone());
    // Move "now" to between 15:00 and 23:59 UTC, where the JST date is already the next day.
    let h = i64::from(OffsetDateTime::now_utc().hour());
    if !(15..=23).contains(&h) {
        clock.advance(Duration::hours((15 - h).rem_euclid(24)));
    }
    let now = OffsetDateTime::now_utc() + Duration::seconds(clock_offset(&clock));
    let utc_day = now.date().to_string();
    let (_, _, _, admin) = c.admin().await;
    claim(&c, &admin, json!({ "subject_type": "part", "subject_key": "EXP-1", "predicate": "mfg.certificate.expiry", "value_text": utc_day })).await;
    let r = c.post("/api/v1/verification/run", Some(&admin), json!({})).await;
    assert_eq!(r.status, StatusCode::OK);
    let open = c.get("/api/v1/conflicts?status=open", &admin).await.body;
    let hit = open
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["kind"] == "expired_evidence")
        .cloned();
    assert!(hit.is_some(), "expiry {utc_day} is yesterday in JST: {open:?}");
}

fn clock_offset(c: &OffsetClock) -> i64 {
    use kizuna_api::clock::Clock;
    (c.now() - OffsetDateTime::now_utc()).whole_seconds()
}

#[tokio::test]
async fn typed_keys_normalised_zero_width_refused_and_supersede_keeps_identity() {
    let c = ctx().await;
    let (_, _, _, admin) = c.admin().await;
    let a = claim(&c, &admin, json!({ "subject_type": "part", "subject_key": "ｂｒｋ－１００", "predicate": "mfg.material", "value_text": "S355" })).await;
    assert_eq!(a["subject_key"], "BRK-100");
    let r = c
        .post("/api/v1/claims", Some(&admin), json!({ "subject_type": "part", "subject_key": "BRK\u{200B}-100", "predicate": "mfg.material", "value_text": "S355" }))
        .await;
    assert_eq!(r.body["error"], "invalid_subject_key");
    let id = a["id"].as_str().unwrap();
    for (k, p) in [("BRK-100", "mfg.revision"), ("BRK-200", "mfg.material")] {
        let r = c
            .post(
                &format!("/api/v1/claims/{id}/supersede"),
                Some(&admin),
                json!({ "subject_type": "part", "subject_key": k, "predicate": p, "value_text": "X", "reason": "fix" }),
            )
            .await;
        assert_eq!(r.body["error"], "supersede_changes_identity", "{k} {p}");
    }
    let r = c
        .post(&format!("/api/v1/claims/{id}/supersede"), Some(&admin), json!({ "subject_type": "part", "subject_key": "brk-100", "predicate": "mfg.material", "value_text": "S275", "reason": "fix" }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
}

#[tokio::test]
async fn decision_links_match_subject_type() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let (_, e) = c.user(t, "reviewer").await;
    let rev = c.mfa_token(&slug, &e).await;
    let part = claim(
        &c,
        &rev,
        json!({ "subject_type": "part", "subject_key": "ACME", "predicate": "mfg.material", "value_text": "S355" }),
    )
    .await;
    let d = c
        .post("/api/v1/decisions", Some(&rev), json!({ "kind": "supplier_part_release", "title": "t", "subject_type": "part", "subject_key": "ACME",
            "outcome": "approved", "rationale": "r", "evidence": [ { "claim_id": part["id"], "role": "basis" } ], "depends_on": [] }))
        .await;
    assert_eq!(d.status, StatusCode::OK, "{:?}", d.body);
    let dec = d.body["id"].as_str().unwrap().to_owned();
    let status =
        || async { c.get(&format!("/api/v1/decisions/{dec}"), &admin).await.body["decision"]["status"].clone() };
    // Same key and predicate, but a supplier: not this decision's evidence.
    claim(
        &c,
        &rev,
        json!({ "subject_type": "supplier", "subject_key": "ACME", "predicate": "mfg.material", "value_text": "S275" }),
    )
    .await;
    assert_eq!(status().await, "valid");
    claim(
        &c,
        &rev,
        json!({ "subject_type": "part", "subject_key": "ACME", "predicate": "mfg.material", "value_text": "S275" }),
    )
    .await;
    assert_eq!(status().await, "potentially_affected");
}

#[tokio::test]
async fn report_dates_are_tenant_local_days() {
    let c = ctx().await;
    let (_, _, _, admin) = c.admin().await;
    let r = c
        .post(
            "/api/v1/reports",
            Some(&admin),
            json!({ "scope": "all", "from_date": "2026-11-05", "to_date": "2026-11-05" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    // JST (+09:00): 5 Nov 00:00 JST = 4 Nov 15:00 UTC; the inclusive end is 6 Nov 00:00 JST.
    assert_eq!(r.body["range"]["from"], 1_793_804_400);
    assert_eq!(r.body["range"]["to_exclusive"], 1_793_804_400 + 86_400);
    assert_eq!(r.body["range"]["utc_offset_minutes"], 540);
    let bad = c
        .post("/api/v1/reports", Some(&admin), json!({ "from_date": "2026-02-30" }))
        .await;
    assert_eq!(bad.body["error"], "invalid_date");
}

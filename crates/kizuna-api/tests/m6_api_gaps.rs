#![allow(clippy::unwrap_used)]
//! Server endpoints added for the web UI (web/API_GAPS.md).
mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;

#[tokio::test]
async fn dashboard_subjects_paging_display_names_and_decision_evidence() {
    let c = ctx().await;
    let (t, slug, uid, admin) = c.admin().await;
    let (_, e) = c.user(t, "reviewer").await;
    let tok = c.token(&slug, &e).await;
    let art = c
        .ingest(
            &tok,
            "po.csv",
            b"PO Number,Part No,Rev\nPO-1,BRK-1,A\nPO-2,BRK-2,A\nPO-3,BRK-3,B\n",
            "purchase_order",
        )
        .await;
    let d = c.get("/api/v1/dashboard/summary", &tok).await;
    assert_eq!(d.body["artifacts"]["accepted"], 1);
    assert_eq!(d.body["subjects"]["part"], 3);
    let s = c.get("/api/v1/subjects?type=part&limit=2", &tok).await;
    assert_eq!(s.body.as_array().unwrap().len(), 2);
    let s2 = c
        .get(
            &format!(
                "/api/v1/subjects?type=part&after={}",
                s.body[1]["subject_key"].as_str().unwrap()
            ),
            &tok,
        )
        .await;
    assert_eq!(s2.body[0]["subject_key"], "BRK-3");
    let p1 = c.get("/api/v1/claims?limit=1", &tok).await;
    assert_eq!(p1.body[0]["artifact_display_name"], "po.csv");
    let p2 = c
        .get(
            &format!("/api/v1/claims?limit=1&after={}", p1.body[0]["id"].as_str().unwrap()),
            &tok,
        )
        .await;
    assert_ne!(p1.body[0]["id"], p2.body[0]["id"]);
    let cid = p1.body[0]["id"].as_str().unwrap();
    let dec = c
        .post("/api/v1/decisions", Some(&tok), json!({"kind":"po_release","title":"t","subject_type":"part","subject_key":"BRK-1","outcome":"approved","rationale":"r","evidence":[{"claim_id": cid}]}))
        .await;
    assert_eq!(dec.body["evidence_claim_ids"], json!([cid]));
    let _ = (art, uid, admin);
}

#[tokio::test]
async fn org_report_import_settings_sessions_supplier_entities_typed_compare() {
    let c = ctx().await;
    let (t, slug, uid, admin) = c.admin().await;
    let (_, v) = c.user(t, "viewer").await;
    let vt = c.token(&slug, &v).await;
    let r = c.post("/api/v1/reports", Some(&admin), json!({"scope": "all"})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(
        r.headers["content-disposition"]
            .to_str()
            .unwrap()
            .contains("attachment")
    );
    assert_eq!(r.body["audit_chain"]["intact"], true);
    assert_eq!(r.body["report_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(
        c.post("/api/v1/reports", Some(&vt), json!({})).await.status,
        StatusCode::FORBIDDEN
    );

    assert_eq!(
        c.get("/api/v1/integrations/import-settings", &admin).await.body["version"],
        0
    );
    let put = |exp: i32, s: serde_json::Value| {
        c.call(
            Method::PUT,
            "/api/v1/integrations/import-settings",
            Some(&admin),
            Some(json!({"expected_version": exp, "settings": s})),
        )
    };
    assert_eq!(
        put(
            0,
            json!({"csv": {"delimiter": ";", "header_row": 1}, "pdf": {"ocr": true}})
        )
        .await
        .body["version"],
        1
    );
    assert_eq!(
        put(0, json!({"csv": {"delimiter": ","}})).await.body["error"],
        "settings_version_conflict"
    );
    assert_eq!(
        put(1, json!({"csv": {"nested": {"too": "deep"}}})).await.body["error"],
        "invalid_settings"
    );
    assert_eq!(
        c.get("/api/v1/integrations/import-settings", &admin).await.body["settings"]["csv"]["delimiter"],
        ";"
    );

    let sess = c.get(&format!("/api/v1/users/{uid}/sessions"), &admin).await;
    assert!(sess.body.as_array().unwrap().len() >= 2);
    assert!(!sess.body.to_string().contains("kza."), "no token material");

    let ok = c
        .post(
            "/api/v1/entities/decisions",
            Some(&admin),
            json!({"subject_type":"supplier","left_key":"ACME STEEL","right_key":"ACME-STEEL","outcome":"same"}),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.body);
    for (rev, val) in [("A", "S275"), ("B", "S355")] {
        c.post("/api/v1/claims", Some(&admin), json!({"subject_type":"supplier","subject_key":"ACME STEEL","predicate":"mfg.material","value_text":val,"revision":rev})).await;
    }
    let cmp = c.get("/api/v1/compare/supplier/ACME%20STEEL?from=A&to=B", &admin).await;
    assert_eq!(cmp.body["differences"][0]["change"], "changed", "{:?}", cmp.body);
}

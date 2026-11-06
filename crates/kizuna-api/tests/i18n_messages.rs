//! Server-generated text on seeded data: every finding explanation, invalidation reason, incident summary and
//! limitation carries a message code from the shared catalogue next to its English fallback, and renders as
//! Japanese without English sentences. Server-rendered reports follow the request language.
#![allow(clippy::unwrap_used)]
mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::*;
use kizuna_messages::{Lang, Msg, catalog, english_run};
use serde_json::{Value, json};

fn spec_pdf(rev: &str, length: &str) -> Vec<u8> {
    make_pdf(&[
        "SUPPLIER SPECIFICATION SHEET",
        "Part Number: BRK-100",
        &format!("Revision: {rev}"),
        "Material: S355JR",
        &format!("Length (mm): {length}"),
        "Length tolerance: +/-0.2",
        "Approval Status: Released",
        "Supplier: ACME",
        "Certificate expiry: 2026-01-31",
    ])
}

/// Asserts `m` is a known message whose Japanese rendering is complete and free of English sentences, and that
/// the English fallback the API also returns is the same message in English.
fn check(at: &str, m: &Value, english: Option<&Value>) {
    let msg = Msg::from_value(m).unwrap_or_else(|| panic!("{at}: no message code: {m}"));
    assert!(catalog().contains_key(&msg.code), "{at}: unknown code {}", msg.code);
    let ja = msg.render(Lang::Ja);
    assert!(
        !ja.contains('{') && !ja.contains(".msg") && !ja.is_empty(),
        "{at}: incomplete JA: {ja}"
    );
    assert_eq!(english_run(&ja), None, "{at}: English in JA text: {ja}");
    if let Some(en) = english {
        assert_eq!(
            en.as_str(),
            Some(msg.en().as_str()),
            "{at}: English fallback differs from the message"
        );
    }
}

async fn get_lang(c: &Ctx, path: &str, tok: &str, lang: &str) -> Resp {
    let req = Request::builder()
        .method(Method::GET)
        .uri(path)
        .header("authorization", format!("Bearer {tok}"))
        .header("accept-language", lang)
        .body(Body::empty())
        .unwrap();
    c.raw(req).await
}

#[tokio::test]
async fn findings_and_limitations_carry_message_codes_rendered_in_japanese() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "reviewer").await;
    let tok = c.token(&slug, &email).await;
    c.ingest(&tok, "spec.pdf", &spec_pdf("C", "120"), "spec_document").await;
    c.ingest(
        &tok,
        "drawing.pdf",
        &make_pdf(&["Drawing No: D-77", "Part Number: BRK-100", "Rev: C", "Length (mm): 125"]),
        "drawing",
    )
    .await;
    c.ingest(
        &tok,
        "po.csv",
        b"PO Number,Part No,Rev,Qty,Vendor\nPO-1,BRK-100,B,50,Beta\n",
        "purchase_order",
    )
    .await;
    c.ingest(
        &tok,
        "inspection.csv",
        b"Lot,Part Number,Rev,Measured length (mm),Result\nL1,BRK-100,C,120.5,pass\nL1,BRK-100,C,120.5,fail\n",
        "inspection_report",
    )
    .await;
    let run = c.post("/api/v1/verification/run", Some(&tok), json!({})).await;
    assert_eq!(run.status, StatusCode::OK, "{:?}", run.body);
    let list = c.get("/api/v1/conflicts?status=open", &tok).await;
    let findings = list.body.as_array().unwrap();
    assert!(findings.len() >= 4, "seeded faults produce findings: {:?}", list.body);
    let mut codes = std::collections::BTreeSet::new();
    for f in findings {
        let e = &f["explanation"];
        check(&format!("finding {}", f["kind"]), &e["msg"], Some(&e["why"]));
        codes.insert(e["msg"]["code"].as_str().unwrap().to_owned());
    }
    assert!(codes.len() >= 3, "several distinct rules covered: {codes:?}");
    let id = findings[0]["id"].as_str().unwrap();
    let d = c.get(&format!("/api/v1/conflicts/{id}"), &tok).await;
    check(
        "finding limitations",
        &d.body["limitations_msg"],
        Some(&d.body["limitations"]),
    );
}

#[tokio::test]
async fn invalidation_reasons_and_reports_follow_the_request_language() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, e) = c.user(t, "reviewer").await;
    let tok = c.mfa_token(&slug, &e).await;
    let claim = c
        .post(
            "/api/v1/claims",
            Some(&tok),
            json!({"subject_type":"part","subject_key":"BRK-7","predicate":"mfg.dimension.length","value_num":"120","value_unit":"mm","revision":"C"}),
        )
        .await;
    let claim = claim.body["id"].as_str().unwrap().to_owned();
    let d = c
        .post(
            "/api/v1/decisions",
            Some(&tok),
            json!({ "kind": "supplier_part_release", "title": "BRK-7 出荷可否", "subject_type": "part", "subject_key": "BRK-7",
                    "outcome": "approved", "rationale": "寸法を確認済み", "evidence": [{ "claim_id": claim }], "depends_on": [] }),
        )
        .await;
    assert_eq!(d.status, StatusCode::OK, "{:?}", d.body);
    let did = d.body["id"].as_str().unwrap().to_owned();
    let s = c
        .post(
            &format!("/api/v1/claims/{claim}/supersede"),
            Some(&tok),
            json!({"subject_type":"part","subject_key":"BRK-7","predicate":"mfg.dimension.length","value_num":"121","value_unit":"mm","revision":"C","reason":"訂正"}),
        )
        .await;
    assert_eq!(s.status, StatusCode::OK, "{:?}", s.body);

    let tr = c.get(&format!("/api/v1/decisions/{did}/trace"), &tok).await;
    let inv = tr.body["invalidations"].as_array().unwrap();
    assert!(!inv.is_empty(), "{:?}", tr.body);
    for i in inv {
        check("invalidation", &i["reason_msg"], Some(&i["reason"]));
    }

    // The Markdown report is written in the request language; English stays available.
    let ja = get_lang(
        &c,
        &format!("/api/v1/decisions/{did}/report?format=markdown"),
        &tok,
        "ja",
    )
    .await;
    let md = ja.body.as_str().unwrap();
    assert!(md.starts_with("# 検証レポート："), "{md}");
    assert_eq!(english_run(md), None, "English sentence in the Japanese report:\n{md}");
    let en = get_lang(
        &c,
        &format!("/api/v1/decisions/{did}/report?format=markdown&lang=en"),
        &tok,
        "ja",
    )
    .await;
    assert!(en.body.as_str().unwrap().starts_with("# Verification report: "));
    let json_ja = get_lang(&c, &format!("/api/v1/decisions/{did}/report"), &tok, "ja-JP").await;
    assert_eq!(json_ja.body["lang"], "ja");
    check(
        "report statement",
        &json_ja.body["statement_msg"],
        Some(&json_ja.body["statement"]),
    );
    for i in json_ja.body["invalidations"].as_array().unwrap() {
        assert_eq!(english_run(i["reason_text"].as_str().unwrap()), None, "{i}");
    }
    let bad = get_lang(&c, &format!("/api/v1/decisions/{did}/report?lang=fr"), &tok, "ja").await;
    assert_eq!(bad.body["error"], "invalid_lang");
}

#[tokio::test]
async fn incident_summaries_carry_message_codes() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let (_, v) = c.user(t, "viewer").await;
    let vt = c.token(&slug, &v).await;
    for _ in 0..12 {
        c.get("/api/v1/users", &vt).await;
    }
    let list = c.get("/api/v1/security/incidents", &admin).await;
    let incidents = list.body.as_array().unwrap();
    assert!(!incidents.is_empty(), "{:?}", list.body);
    for i in incidents {
        check(
            &format!("incident {}", i["rule"]),
            &i["summary_msg"],
            Some(&i["summary"]),
        );
        let id = i["id"].as_str().unwrap();
        let d = c.get(&format!("/api/v1/security/incidents/{id}"), &admin).await;
        for a in d.body["actions"].as_array().into_iter().flatten() {
            if !a["detail"].is_null() {
                check(&format!("action {}", a["kind"]), &a["detail_msg"], Some(&a["detail"]));
            }
        }
    }
}

#![allow(clippy::unwrap_used)]
//! 8D responses through the supplier portal: an approved CAPA supplier request is published to one portal supplier;
//! what that supplier uploads joins the case as supplier evidence (never an approval or a completed step); other
//! suppliers and the portal scope see nothing of the case. Overdue alerts use the tenant time zone.
mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::*;
use kizuna_api::crypto::totp_now;
use serde_json::{Value, json};
use uuid::Uuid;

const PART: &str = "PORTAL-8D";

async fn onboard(c: &Ctx, admin: &str, supplier: &str) -> (Uuid, String) {
    let email = format!("{}@supplier.test", Uuid::new_v4().simple());
    let r = c
        .post(
            "/api/v1/portal-users",
            Some(admin),
            json!({ "supplier": supplier, "email": email, "display_name": "Sato" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let invite = r.body["invite_token"].as_str().unwrap().to_owned();
    let id: Uuid = r.body["user"]["supplier_id"].as_str().unwrap().parse().unwrap();
    let r = c
        .post(
            "/api/v1/portal/invite/start",
            None,
            json!({ "token": invite, "password": PW }),
        )
        .await;
    let secret = base32_decode(r.body["secret_base32"].as_str().unwrap());
    let r = c
        .post(
            "/api/v1/portal/invite/finish",
            None,
            json!({ "token": invite, "code": totp_now(&secret, now()) }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    (id, r.body["access_token"].as_str().unwrap().to_owned())
}

async fn portal_upload(c: &Ctx, token: &str, request: &str, name: &str, data: &[u8]) -> Resp {
    let boundary = "kizuna8dboundary";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/pdf\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/portal/requests/{request}/uploads"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap();
    c.raw(req).await
}

async fn claim(c: &Ctx, tok: &str, v: &str) -> String {
    let r = c
        .post(
            "/api/v1/claims",
            Some(tok),
            json!({"subject_type":"part","subject_key":PART,"predicate":"mfg.material.hardness","value_num":v,"value_unit":"mm","scope":{"supplier":"ACME"}}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn supplier_submits_8d_response_through_the_portal_as_evidence_only() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let (a_id, ea) = c.user(t, "reviewer").await;
    let (b_id, eb) = c.user(t, "reviewer").await;
    let (a, b) = (c.mfa_token(&slug, &ea).await, c.mfa_token(&slug, &eb).await);
    // A conflict → CAPA case → D1/D2 → supplier request approved by a second person.
    let (x, y) = (claim(&c, &a, "30").await, claim(&c, &a, "45").await);
    let cf = Uuid::now_v7();
    let mut tx = kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap();
    sqlx::query(
        "INSERT INTO conflicts (id, tenant_id, kind, subject_type, subject_key, claim_a, claim_b, explanation, rule_version) \
         VALUES ($1,$2,'value_mismatch','part',$3,$4::uuid,$5::uuid,'{\"why\":\"differs\"}','t')",
    )
    .bind(cf)
    .bind(t)
    .bind(PART)
    .bind(&x)
    .bind(&y)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let r = c
        .post(
            "/api/v1/capa/cases",
            Some(&a),
            json!({"source_kind":"conflict","source_id":cf}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let case = r.body["case"]["id"].as_str().unwrap().to_owned();
    let step = |d: u8, content: Value| {
        let (c, a, case) = (&c, a.clone(), case.clone());
        async move {
            let r = c
                .call(
                    Method::PUT,
                    &format!("/api/v1/capa/cases/{case}/steps/{d}"),
                    Some(&a),
                    Some(json!({ "content": content })),
                )
                .await;
            assert_eq!(r.status, StatusCode::OK, "D{d} {:?}", r.body);
            let r = c
                .post(
                    &format!("/api/v1/capa/cases/{case}/steps/{d}/complete"),
                    Some(&a),
                    json!({}),
                )
                .await;
            assert_eq!(r.status, StatusCode::OK, "D{d} {:?}", r.body);
        }
    };
    step(
        1,
        json!({"members":[{"user":a_id,"role":"leader"},{"user":b_id,"role":"member"}]}),
    )
    .await;
    step(2, json!({"statement":"Hardness differs","is_is_not":[{"dimension":"what","is":"hardness","is_not":"chemistry"}],"evidence":[{"claim": x}]})).await;
    let r = c
        .post(
            &format!("/api/v1/capa/cases/{case}/supplier-requests"),
            Some(&a),
            json!({"due_date":"2099-01-31","disciplines":[3,4,5]}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let req = r.body["supplier_requests"][0]["id"].as_str().unwrap().to_owned();
    let (acme, acme_tok) = onboard(&c, &admin, "ACME").await;
    let (_other, other_tok) = onboard(&c, &admin, "Borealis Forge").await;
    let publish = |tok: String| {
        let (c, req) = (&c, req.clone());
        async move {
            c.post(
                &format!("/api/v1/capa/supplier-requests/{req}/portal"),
                Some(&tok),
                json!({ "supplier_id": acme }),
            )
            .await
        }
    };
    assert_eq!(
        publish(a.clone()).await.body["error"],
        "request_not_approved",
        "a draft is never published"
    );
    let ok = c
        .post(
            &format!("/api/v1/capa/supplier-requests/{req}/approve"),
            Some(&b),
            json!({}),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.body);
    let p = publish(a.clone()).await;
    assert_eq!(p.status, StatusCode::OK, "{:?}", p.body);
    let er = p.body["evidence_request"].as_str().unwrap().to_owned();
    assert_eq!(publish(a.clone()).await.body["error"], "already_published");
    // ACME sees the 8D request (the approved letter); another supplier does not.
    let mine = c.get("/api/v1/portal/requests", &acme_tok).await;
    assert!(
        mine.body.as_array().unwrap().iter().any(|r| r["id"] == er.as_str()),
        "{:?}",
        mine.body
    );
    let theirs = c.get("/api/v1/portal/requests", &other_tok).await;
    assert!(theirs.body.as_array().unwrap().iter().all(|r| r["id"] != er.as_str()));
    assert_eq!(
        portal_upload(&c, &other_tok, &er, "x.pdf", CLEAN_PDF)
            .await
            .status
            .as_u16()
            / 100,
        4
    );
    // The 8D response document joins the case as supplier evidence; no step is completed by it.
    let before = c.get(&format!("/api/v1/capa/cases/{case}"), &a).await.body;
    let up = portal_upload(&c, &acme_tok, &er, "acme-8d.pdf", CLEAN_PDF).await;
    assert_eq!(
        (up.status, up.body["status"].clone()),
        (StatusCode::OK, json!("received_for_review")),
        "{:?}",
        up.body
    );
    let after = c.get(&format!("/api/v1/capa/cases/{case}"), &a).await.body;
    let resp = after["supplier_responses"].as_array().unwrap();
    assert_eq!(resp.len(), 1, "{after:?}");
    let done = |d: &Value| {
        d["steps"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["status"] == "complete")
            .count()
    };
    assert_eq!(done(&before), done(&after), "evidence only");
    assert_eq!(before["case"]["status"], after["case"]["status"]);
    let mut tx = kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap();
    let ev: i64 = sqlx::query_scalar("SELECT count(*) FROM capa_events WHERE kind = 'capa.supplier_response_received'")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(ev, 1);
    // The portal scope sees no CAPA data at all (deny-by-default covers tables created after the portal).
    let rt = runtime_pool().await;
    let mut tx = kizuna_api::db::portal_tx(&rt, t, acme).await.unwrap();
    for table in [
        "capa_cases",
        "capa_supplier_requests",
        "capa_supplier_responses",
        "capa_events",
        "capa_portal_links",
    ] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(n, 0, "{table} under the portal scope");
    }
    tx.rollback().await.unwrap();
}

#[test]
fn overdue_alert_carries_identifiers_only() {
    let a = kizuna_api::capa::overdue_alert("kizuna", "acme-jp", time::macros::date!(2026 - 11 - 06), 3);
    assert_eq!(a.key, "capa-overdue:acme-jp:2026-11-06");
    assert!(a.text.contains("3 new overdue") && !a.text.contains('@'));
}

#[tokio::test]
async fn overdue_sweep_uses_the_tenant_calendar() {
    let c = ctx().await;
    let (t, _, _, _) = c.admin().await;
    // 2026-11-05 20:00 UTC is already 6 Nov in Tokyo: an action due on 5 Nov is overdue there, not in UTC.
    let now = time::macros::datetime!(2026 - 11 - 05 20:00 UTC);
    let mut tx = kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap();
    assert_eq!(
        kizuna_api::db::tenant_today(&mut tx, t, now).await.unwrap(),
        time::macros::date!(2026 - 11 - 06)
    );
    tx.commit().await.unwrap();
}

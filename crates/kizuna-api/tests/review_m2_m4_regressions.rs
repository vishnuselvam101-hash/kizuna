#![allow(clippy::unwrap_used)]
//! Regression tests for docs/security/REVIEW_M2_M4.md findings.
mod common;

use axum::http::StatusCode;
use common::*;
use kizuna_api::db;
use serde_json::{Value, json};
use uuid::Uuid;

async fn claim(c: &Ctx, tok: &str, body: Value) -> String {
    let r = c.post("/api/v1/claims", Some(tok), body).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body["id"].as_str().unwrap().to_owned()
}

fn len_claim(key: &str, v: &str) -> Value {
    json!({"subject_type":"part","subject_key":key,"predicate":"mfg.dimension.length","value_num":v,"value_unit":"mm","revision":"C"})
}

async fn decision(c: &Ctx, tok: &str, ev: Value, up: Value) -> String {
    let r = c
        .post(
            "/api/v1/decisions",
            Some(tok),
            json!({"kind":"part_release","title":"t","subject_type":"part","subject_key":"BRK-1",
            "outcome":"approved","rationale":"r","evidence":ev,"depends_on":up}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body["id"].as_str().unwrap().to_owned()
}

async fn status(c: &Ctx, tok: &str, id: &str) -> String {
    c.get(&format!("/api/v1/decisions/{id}"), tok).await.body["decision"]["status"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn open_conflicts(c: &Ctx, tok: &str) -> Vec<Value> {
    c.get("/api/v1/conflicts?status=open", tok)
        .await
        .body
        .as_array()
        .unwrap()
        .clone()
}

#[tokio::test]
async fn h1_reviewer_cannot_approve_own_upload_waive_own_findings_or_reconfirm_own_decision() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, e) = c.user(t, "reviewer").await;
    let r = c.mfa_token(&slug, &e).await;
    let art = c
        .ingest(&r, "po.csv", b"PO Number,Part No,Rev\nPO-1,BRK-1,A\n", "purchase_order")
        .await;
    let cl = c.get(&format!("/api/v1/claims?artifact_id={art}"), &r).await.body[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let own = c
        .post(
            &format!("/api/v1/claims/{cl}/review"),
            Some(&r),
            json!({"decision":"approve","comment":"ok"}),
        )
        .await;
    assert_eq!(own.body["error"], "no_self_review");
    c.post("/api/v1/verification/run", Some(&r), json!({})).await;
    let k = open_conflicts(&c, &r).await[0]["id"].as_str().unwrap().to_owned();
    let w = c
        .post(
            &format!("/api/v1/conflicts/{k}/review"),
            Some(&r),
            json!({"decision":"approve","comment":"fine"}),
        )
        .await;
    assert_eq!(w.body["error"], "no_self_review");

    let x = claim(&c, &r, len_claim("BRK-1", "10")).await;
    let d = decision(&c, &r, json!([{"claim_id": x}]), json!([])).await;
    c.post(&format!("/api/v1/claims/{x}/supersede"), Some(&r), {
        let mut b = len_claim("BRK-1", "11");
        b["reason"] = json!("fix");
        b
    })
    .await;
    let rc = c
        .post(
            &format!("/api/v1/decisions/{d}/reconfirm"),
            Some(&r),
            json!({"comment":"mine"}),
        )
        .await;
    assert_eq!(rc.body["error"], "no_self_review");
}

#[tokio::test]
async fn h2_obsolete_findings_reopen_and_rejection_is_final() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, a) = c.user(t, "analyst").await;
    let (_, rv) = c.user(t, "reviewer").await;
    let at = c.token(&slug, &a).await;
    let rt = c.mfa_token(&slug, &rv).await;
    let x = claim(&c, &at, len_claim("BRK-1", "10")).await;
    claim(&c, &at, len_claim("BRK-1", "12")).await;
    c.post("/api/v1/verification/run", Some(&at), json!({})).await;
    assert_eq!(open_conflicts(&c, &at).await.len(), 1);
    let set = |s: &'static str| {
        let (owner, x) = (c.owner.clone(), x.clone());
        async move {
            let mut tx = db::tenant_tx(&owner, t).await.unwrap();
            sqlx::query("UPDATE claims SET review_status = $2 WHERE id = $1::uuid")
                .bind(&x)
                .bind(s)
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    };
    set("rejected").await;
    c.post("/api/v1/verification/run", Some(&at), json!({})).await;
    assert!(open_conflicts(&c, &at).await.is_empty(), "obsolete while A is rejected");
    set("unreviewed").await;
    c.post("/api/v1/verification/run", Some(&at), json!({})).await;
    assert_eq!(open_conflicts(&c, &at).await.len(), 1, "finding re-opened");

    let rej = c
        .post(
            &format!("/api/v1/claims/{x}/review"),
            Some(&rt),
            json!({"decision":"reject","comment":"wrong"}),
        )
        .await;
    assert_eq!(rej.body["review_status"], "rejected");
    let again = c
        .post(
            &format!("/api/v1/claims/{x}/review"),
            Some(&rt),
            json!({"decision":"approve","comment":"oops"}),
        )
        .await;
    assert_eq!(again.body["error"], "claim_rejected_supersede_instead");
}

#[tokio::test]
async fn m2_reconfirm_keeps_decision_links_and_m1_scope_change_is_material() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, a) = c.user(t, "reviewer").await;
    let (_, b) = c.user(t, "reviewer").await;
    let at = c.mfa_token(&slug, &a).await;
    let bt = c.mfa_token(&slug, &b).await;
    let x = claim(&c, &at, len_claim("BRK-1", "10")).await;
    let d1 = decision(&c, &at, json!([{"claim_id": x}]), json!([])).await;
    let d2 = decision(&c, &at, json!([]), json!([{"decision_id": d1}])).await;
    // Same value, different revision: material (M1).
    let mut corr = len_claim("BRK-1", "10");
    corr["revision"] = json!("D");
    corr["reason"] = json!("revision D");
    c.post(&format!("/api/v1/claims/{x}/supersede"), Some(&at), corr).await;
    assert_eq!(status(&c, &at, &d1).await, "potentially_affected");
    assert_eq!(status(&c, &at, &d2).await, "potentially_affected");
    let early = c
        .post(
            &format!("/api/v1/decisions/{d2}/reconfirm"),
            Some(&bt),
            json!({"comment":"x"}),
        )
        .await;
    assert_eq!(early.body["error"], "upstream_unresolved");
    let d1v2 = c
        .post(
            &format!("/api/v1/decisions/{d1}/reconfirm"),
            Some(&bt),
            json!({"comment":"rev D ok"}),
        )
        .await
        .body["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let d2v2 = c
        .post(
            &format!("/api/v1/decisions/{d2}/reconfirm"),
            Some(&bt),
            json!({"comment":"ok"}),
        )
        .await
        .body["id"]
        .as_str()
        .unwrap()
        .to_owned();
    c.post(
        &format!("/api/v1/decisions/{d1v2}/revoke"),
        Some(&at),
        json!({"comment":"withdrawn"}),
    )
    .await;
    assert_eq!(
        status(&c, &at, &d2v2).await,
        "potentially_affected",
        "link to the new upstream version kept"
    );
}

#[tokio::test]
async fn m3_decision_racing_a_supersession_is_refused() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (uid, e) = c.user(t, "reviewer").await;
    let tok = c.token(&slug, &e).await;
    let x: Uuid = claim(&c, &tok, len_claim("BRK-1", "10")).await.parse().unwrap();
    let y: Uuid = claim(&c, &tok, len_claim("BRK-1", "11")).await.parse().unwrap();
    // A supersession is in flight (row locked, not yet committed).
    let mut tx = db::tenant_tx(&c.st.pool, t).await.unwrap();
    sqlx::query("SELECT 1 FROM claims WHERE id = $1 FOR UPDATE")
        .bind(x)
        .execute(&mut *tx)
        .await
        .unwrap();
    let c2 = c.app.clone();
    let tok2 = tok.clone();
    let create = tokio::spawn(async move {
        let req = axum::http::Request::post("/api/v1/decisions")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {tok2}"))
            .body(axum::body::Body::from(
                json!({"kind":"part_release","title":"t","subject_type":"part","subject_key":"BRK-1",
                "outcome":"approved","rationale":"r","evidence":[{"claim_id": x}]})
                .to_string(),
            ))
            .unwrap();
        tower::ServiceExt::oneshot(c2, req).await.unwrap().status()
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    sqlx::query("UPDATE claims SET superseded_at = now(), superseded_by = $2 WHERE id = $1")
        .bind(x)
        .bind(y)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        create.await.unwrap(),
        StatusCode::CONFLICT,
        "create waited for the lock and saw the supersession"
    );
    let _ = uid;
}

#[tokio::test]
async fn m4_manual_claims_cannot_cite_unreleased_documents_or_set_confidence() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, e) = c.user(t, "analyst").await;
    let tok = c.token(&slug, &e).await;
    let q = c.upload(&tok, "q.pdf", b"%PDF-1.7\n<< /JS (x) >>\n%%EOF", None).await;
    let qid = q.body["artifact"]["id"].as_str().unwrap();
    let mut body = len_claim("BRK-1", "10");
    body["artifact_id"] = json!(qid);
    assert_eq!(
        c.post("/api/v1/claims", Some(&tok), body).await.body["error"],
        "artifact_not_released"
    );
    let mut body = len_claim("BRK-1", "10");
    body["confidence_bps"] = json!(1);
    assert_eq!(
        c.post("/api/v1/claims", Some(&tok), body).await.body["confidence_bps"],
        10000
    );
}

#[tokio::test]
async fn m7_confirming_a_finding_withdraws_faulty_evidence_from_decisions() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, a) = c.user(t, "analyst").await;
    let (_, rv) = c.user(t, "reviewer").await;
    let at = c.token(&slug, &a).await;
    let rt = c.mfa_token(&slug, &rv).await;
    let x = claim(&c, &at, len_claim("BRK-1", "10")).await;
    claim(&c, &at, len_claim("BRK-1", "12")).await;
    let d = decision(&c, &rt, json!([{"claim_id": x}]), json!([])).await;
    c.post("/api/v1/verification/run", Some(&at), json!({})).await;
    let k = open_conflicts(&c, &at).await[0]["id"].as_str().unwrap().to_owned();
    let no = c
        .post(
            &format!("/api/v1/conflicts/{k}/review"),
            Some(&rt),
            json!({"decision":"reject","comment":"10 is wrong"}),
        )
        .await;
    assert_eq!(no.body["error"], "faulty_claims_required");
    let ok = c
        .post(
            &format!("/api/v1/conflicts/{k}/review"),
            Some(&rt),
            json!({"decision":"reject","comment":"10 is wrong","faulty_claims":[x]}),
        )
        .await;
    assert_eq!(ok.body["status"], "resolved_rejected", "{:?}", ok.body);
    assert_eq!(status(&c, &at, &d).await, "potentially_affected");
}

#[tokio::test]
async fn l1_l2_aliases_merge_and_reach_decisions() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, rv) = c.user(t, "reviewer").await;
    let rt = c.mfa_token(&slug, &rv).await;
    let x = claim(&c, &rt, len_claim("BRK-100", "10")).await;
    let d = decision(&c, &rt, json!([{"claim_id": x}]), json!([])).await;
    let same = |l: &str, r: &str| json!({"subject_type":"part","left_key":l,"right_key":r,"outcome":"same"});
    assert_eq!(
        c.post("/api/v1/entities/decisions", Some(&rt), same("BRK-100", "BRK100"))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        c.post("/api/v1/entities/decisions", Some(&rt), same("BRK-0100", "BRK-100"))
            .await
            .status,
        StatusCode::OK
    );
    let aliases: Vec<(String, String)> =
        sqlx::query_as("SELECT alias_key, canonical_key FROM entity_aliases ORDER BY 1")
            .fetch_all(&mut *db::tenant_tx(&c.owner, t).await.unwrap())
            .await
            .unwrap();
    assert!(
        aliases.iter().all(|(_, can)| can == "BRK-0100"),
        "chain flattened: {aliases:?}"
    );
    claim(&c, &rt, len_claim("brk100", "12")).await;
    assert_eq!(
        status(&c, &rt, &d).await,
        "potentially_affected",
        "new evidence under an alias reaches the decision"
    );
}

#[tokio::test]
async fn l6_l7_l8_l9_mfa_audit_signature_escaping() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, rv) = c.user(t, "reviewer").await;
    let plain = c.token(&slug, &rv).await;
    let x = claim(&c, &plain, len_claim("BRK-1", "10")).await;
    claim(&c, &plain, len_claim("BRK-1", "12")).await;
    c.post("/api/v1/verification/run", Some(&plain), json!({})).await;
    let k = open_conflicts(&c, &plain).await[0]["id"].as_str().unwrap().to_owned();
    let r = c
        .post(
            &format!("/api/v1/conflicts/{k}/review"),
            Some(&plain),
            json!({"decision":"request_evidence"}),
        )
        .await;
    assert_eq!(r.body["error"], "mfa_required", "L6");

    let id = c.upload(&plain, "s.csv", b"a,b\n1,2\n", None).await.body["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    c.get(&format!("/api/v1/artifacts/{id}/content"), &plain).await;
    let r = c
        .post(
            "/api/v1/decisions",
            Some(&plain),
            json!({"kind":"part_release","title":"![a](http://x) [b](javascript:y)","subject_type":"part",
            "subject_key":"BRK-1","outcome":"approved","rationale":"r","evidence":[{"claim_id": x}]}),
        )
        .await;
    let d = r.body["id"].as_str().unwrap().to_owned();
    let rep = c.get(&format!("/api/v1/decisions/{d}/report"), &plain).await;
    assert_eq!(rep.body["derivation_matches_audit_log"], true);
    assert_eq!(
        rep.body["signature"]["algorithm"], "ed25519",
        "L8: signed when a key is configured"
    );
    let md = c
        .get(&format!("/api/v1/decisions/{d}/report?format=markdown"), &plain)
        .await;
    let text = md.body.as_str().unwrap();
    assert!(
        !text.contains("![a](") && !text.contains("[b](javascript"),
        "L9: {text}"
    );

    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    let actions: Vec<String> = sqlx::query_scalar("SELECT action FROM audit_events")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert!(actions.contains(&"artifact.downloaded".to_string()), "L7");
    let export: Value = sqlx::query_scalar(
        "SELECT details FROM security_events WHERE kind = 'export.report' AND tenant_id = $1 LIMIT 1",
    )
    .bind(t)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(export["report_sha256"].as_str().unwrap().len(), 64, "L7");
    // L8: a derivation rewritten in place (trigger disabled, hash recomputed) no longer matches the audit log.
    sqlx::query("ALTER TABLE decisions DISABLE TRIGGER decisions_guard")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE decisions SET derivation = derivation || '{\"outcome\":\"rejected\"}', derivation_sha256 = sha256(convert_to('x', 'UTF8')) WHERE id = $1::uuid")
        .bind(&d).execute(&mut *tx).await.unwrap();
    sqlx::query("ALTER TABLE decisions ENABLE TRIGGER decisions_guard")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let tr = c.get(&format!("/api/v1/decisions/{d}/trace"), &plain).await;
    assert_eq!(tr.body["derivation_matches_audit_log"], false);
}

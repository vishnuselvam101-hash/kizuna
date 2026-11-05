#![allow(clippy::unwrap_used)]
mod common;

use axum::http::StatusCode;
use common::*;
use kizuna_api::db;
use serde_json::{Value, json};

struct Setup {
    c: Ctx,
    tok: String,
    rev2: String,
    length: String,
    approval: String,
}

async fn claim(c: &Ctx, tok: &str, body: Value) -> String {
    let r = c.post("/api/v1/claims", Some(tok), body).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body["id"].as_str().unwrap().to_owned()
}

async fn setup() -> Setup {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, e) = c.user(t, "reviewer").await;
    let (_, e2) = c.user(t, "reviewer").await;
    let tok = c.mfa_token(&slug, &e).await;
    let rev2 = c.mfa_token(&slug, &e2).await;
    let length = claim(&c, &tok, json!({"subject_type":"part","subject_key":"BRK-100","predicate":"mfg.dimension.length","value_num":"120","value_unit":"mm","revision":"C","scope":{"supplier":"ACME"}})).await;
    let approval = claim(&c, &tok, json!({"subject_type":"part","subject_key":"BRK-100","predicate":"mfg.approval.status","value_text":"approved","revision":"C"})).await;
    Setup {
        c,
        tok,
        rev2,
        length,
        approval,
    }
}

async fn decide(s: &Setup, title: &str, evidence: Value, depends_on: Value) -> String {
    let r = s
        .c
        .post("/api/v1/decisions", Some(&s.tok), json!({
            "kind": "supplier_part_release", "title": title, "subject_type": "part", "subject_key": "BRK-100",
            "outcome": "approved", "rationale": "Spec rev C approved; dimensions verified", "evidence": evidence, "depends_on": depends_on
        }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body["id"].as_str().unwrap().to_owned()
}

async fn status(s: &Setup, id: &str) -> String {
    s.c.get(&format!("/api/v1/decisions/{id}"), &s.tok).await.body["decision"]["status"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn supersede(s: &Setup, old: &str, value: &str) -> String {
    let r = s
        .c
        .post(&format!("/api/v1/claims/{old}/supersede"), Some(&s.tok), json!({
            "subject_type":"part","subject_key":"BRK-100","predicate":"mfg.dimension.length","value_num":value,"value_unit":"mm","revision":"C","scope":{"supplier":"ACME"},"reason":"corrected"
        }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn selective_invalidation_with_materiality_and_propagation() {
    let s = setup().await;
    let release = decide(
        &s,
        "Release BRK-100 rev C",
        json!([
            {"claim_id": s.length, "materiality": {"type": "numeric_abs", "threshold": "0.05"}},
            {"claim_id": s.approval}
        ]),
        json!([]),
    )
    .await;
    let po = decide(&s, "Release PO-1", json!([]), json!([{"decision_id": release}])).await;
    let other_claim = claim(
        &s.c,
        &s.tok,
        json!({"subject_type":"part","subject_key":"SHF-9","predicate":"mfg.material","value_text":"C45"}),
    )
    .await;
    let unrelated = decide(&s, "Release SHF-9", json!([{"claim_id": other_claim}]), json!([])).await;

    // 1. Immaterial correction: recorded, nothing flagged.
    let v2 = supersede(&s, &s.length, "120.04").await;
    assert_eq!(status(&s, &release).await, "valid");
    let t = s.c.get(&format!("/api/v1/decisions/{release}/trace"), &s.tok).await;
    assert_eq!(t.body["invalidations"][0]["material"], false);

    // 2. Second correction drifts beyond the threshold relative to the value the decision used.
    supersede(&s, &v2, "120.08").await;
    assert_eq!(status(&s, &release).await, "potentially_affected");
    assert_eq!(
        status(&s, &po).await,
        "potentially_affected",
        "propagates to dependent decision"
    );
    assert_eq!(status(&s, &unrelated).await, "valid", "unrelated decision untouched");
    let t = s.c.get(&format!("/api/v1/decisions/{po}/trace"), &s.tok).await;
    let inv = t.body["invalidations"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(inv["cause_kind"], "upstream_decision");
    assert!(inv["path"].as_array().unwrap().iter().any(|x| x == &json!(release)));
}

#[tokio::test]
async fn new_evidence_and_withdrawal_respect_applicability() {
    let s = setup().await;
    let d = decide(
        &s,
        "Release",
        json!([{"claim_id": s.length}, {"claim_id": s.approval}]),
        json!([]),
    )
    .await;
    claim(&s.c, &s.tok, json!({"subject_type":"part","subject_key":"BRK-100","predicate":"mfg.dimension.length","value_num":"130","value_unit":"mm","revision":"D"})).await;
    claim(&s.c, &s.tok, json!({"subject_type":"part","subject_key":"BRK-100","predicate":"mfg.dimension.length","value_num":"125","value_unit":"mm","revision":"C","scope":{"supplier":"OTHER"}})).await;
    claim(&s.c, &s.tok, json!({"subject_type":"part","subject_key":"BRK-100","predicate":"mfg.dimension.length","value_num":"12","value_unit":"cm","revision":"C","scope":{"supplier":"ACME"}})).await;
    assert_eq!(
        status(&s, &d).await,
        "valid",
        "other revision, other supplier and confirming evidence do not apply"
    );
    // Rejecting the approval claim (by a second reviewer) withdraws evidence.
    let r =
        s.c.post(
            &format!("/api/v1/claims/{}/review", s.approval),
            Some(&s.rev2),
            json!({"decision":"reject","comment":"approval was for rev B"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(status(&s, &d).await, "potentially_affected");
}

#[tokio::test]
async fn trace_reconfirm_revoke_and_immutability() {
    let s = setup().await;
    let d = decide(&s, "Release", json!([{"claim_id": s.length}]), json!([])).await;
    let down = decide(&s, "Downstream", json!([]), json!([{"decision_id": d}])).await;
    assert_eq!(
        s.c.post(
            &format!("/api/v1/decisions/{d}/reconfirm"),
            Some(&s.rev2),
            json!({"comment":"x"})
        )
        .await
        .status,
        StatusCode::CONFLICT
    );
    let new_len = supersede(&s, &s.length, "121").await;

    let t = s.c.get(&format!("/api/v1/decisions/{d}/trace"), &s.tok).await;
    assert_eq!(t.body["derivation_reproducible"], true);
    assert_eq!(t.body["evidence"][0]["changed"], true);
    assert_eq!(t.body["evidence"][0]["current_claim_id"], new_len);
    assert!(
        t.body["known_at_decision_time"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k["claim_id"] == json!(s.length))
    );
    assert!(
        t.body["changed_since_decision"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k["claim_id"] == json!(new_len))
    );

    let r =
        s.c.post(
            &format!("/api/v1/decisions/{d}/reconfirm"),
            Some(&s.rev2),
            json!({"comment":"121 mm within assembly clearance"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let v2 = r.body["id"].as_str().unwrap().to_owned();
    assert_eq!(r.body["supersedes"], json!(d));
    assert_eq!(status(&s, &d).await, "superseded");
    let t2 = s.c.get(&format!("/api/v1/decisions/{v2}/trace"), &s.tok).await;
    assert_eq!(
        t2.body["evidence"][0]["claim_id"], new_len,
        "new version uses current evidence"
    );

    let r =
        s.c.post(
            &format!("/api/v1/decisions/{v2}/revoke"),
            Some(&s.tok),
            json!({"comment":"supplier withdrawn"}),
        )
        .await;
    assert_eq!(r.body["status"], "revoked", "{:?}", r.body);
    assert_eq!(
        s.c.post(
            &format!("/api/v1/decisions/{v2}/revoke"),
            Some(&s.tok),
            json!({"comment":"again"})
        )
        .await
        .status,
        StatusCode::CONFLICT
    );
    assert_eq!(status(&s, &down).await, "potentially_affected");

    // Decision content is immutable even for the owner role.
    let tenant: uuid::Uuid = s.c.get("/api/v1/auth/me", &s.tok).await.body["tenant_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let mut tx = db::tenant_tx(&s.c.owner, tenant).await.unwrap();
    let e = sqlx::query("UPDATE decisions SET outcome = 'rejected' WHERE id = $1::uuid")
        .bind(&d)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("immutable"), "{e}");
    tx.rollback().await.unwrap();
    let mut tx = db::tenant_tx(&s.c.owner, tenant).await.unwrap();
    let e = sqlx::query("UPDATE decisions SET status = 'valid' WHERE id = $1::uuid")
        .bind(&d)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("final"), "{e}");
}

#[tokio::test]
async fn verification_report_json_and_markdown() {
    let s = setup().await;
    let d = decide(
        &s,
        "Release | <script>x</script>",
        json!([{"claim_id": s.length}]),
        json!([]),
    )
    .await;
    let r = s.c.get(&format!("/api/v1/decisions/{d}/report"), &s.tok).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["audit_chain"]["intact"], true);
    assert_eq!(r.body["derivation_reproducible"], true);
    assert_eq!(r.body["report_sha256"].as_str().unwrap().len(), 64);
    assert!(
        r.body["statement"]
            .as_str()
            .unwrap()
            .contains("do not show that document contents are true")
    );
    let m =
        s.c.get(&format!("/api/v1/decisions/{d}/report?format=markdown"), &s.tok)
            .await;
    assert!(m.headers["content-type"].to_str().unwrap().starts_with("text/markdown"));
    let text = m.body.as_str().unwrap();
    assert!(text.contains("# Verification report"));
    assert!(
        text.contains("Release \\| &lt;script>"),
        "markdown/HTML injection escaped: {text}"
    );
}

#[tokio::test]
async fn decision_rbac_validation_and_isolation() {
    let s = setup().await;
    let tenant: String = s.c.get("/api/v1/auth/me", &s.tok).await.body["tenant_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let slug: String = sqlx::query_scalar("SELECT slug FROM tenants WHERE id = $1::uuid")
        .bind(&tenant)
        .fetch_one(&s.c.owner)
        .await
        .unwrap();
    let (_, ve) = s.c.user(tenant.parse().unwrap(), "viewer").await;
    let v = s.c.token(&slug, &ve).await;
    let body = json!({"kind":"x_release","title":"t","subject_type":"part","subject_key":"BRK-100","outcome":"approved","rationale":"r","evidence":[{"claim_id": s.length}]});
    assert_eq!(
        s.c.post("/api/v1/decisions", Some(&v), body.clone()).await.status,
        StatusCode::FORBIDDEN
    );
    let mut bad = body.clone();
    bad["evidence"] = json!([]);
    assert_eq!(
        s.c.post("/api/v1/decisions", Some(&s.tok), bad).await.body["error"],
        "evidence_required"
    );
    let mut bad = body.clone();
    bad["evidence"] = json!([{"claim_id": s.length, "materiality": {"type": "numeric_abs", "threshold": "1e9"}}]);
    assert_eq!(
        s.c.post("/api/v1/decisions", Some(&s.tok), bad).await.body["error"],
        "invalid_threshold"
    );
    let d = decide(&s, "R", json!([{"claim_id": s.length}]), json!([])).await;
    assert_eq!(
        s.c.get(&format!("/api/v1/decisions/{d}"), &v).await.status,
        StatusCode::OK,
        "viewer may read"
    );

    let other = ctx().await;
    let (tb, sb) = other.tenant().await;
    let (_, eb) = other.user(tb, "reviewer").await;
    let b = other.token(&sb, &eb).await;
    assert_eq!(
        other.get(&format!("/api/v1/decisions/{d}"), &b).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        other.post("/api/v1/decisions", Some(&b), body).await.body["error"],
        "unknown_claim"
    );
}

#[tokio::test]
async fn finding_detail_lists_affected_decisions() {
    let s = setup().await;
    let d = decide(&s, "Release", json!([{"claim_id": s.length}]), json!([])).await;
    claim(&s.c, &s.tok, json!({"subject_type":"part","subject_key":"BRK-100","predicate":"mfg.dimension.length","value_num":"125","value_unit":"mm","revision":"C"})).await;
    s.c.post("/api/v1/verification/run", Some(&s.tok), json!({})).await;
    let list = s.c.get("/api/v1/conflicts?kind=value_mismatch", &s.tok).await;
    let id = list.body[0]["id"].as_str().unwrap();
    let detail = s.c.get(&format!("/api/v1/conflicts/{id}"), &s.tok).await;
    assert_eq!(detail.body["potentially_affected_decisions"][0]["id"], json!(d));
}

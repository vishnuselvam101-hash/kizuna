#![allow(clippy::unwrap_used)]
//! Corrective-action cases (8D / 是正処置) through the API: opening from an SPC out-of-control finding and from a
//! conflict with their evidence, per-discipline evidence requirements, four-eyes closure, supplier 8D responses as
//! evidence only, reopening when later evidence contradicts the effectiveness check, overdue notifications, audit,
//! tenant isolation and FORCE RLS. The state machine itself is property-tested in domain-manufacturing.
mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{Value, json};
use uuid::Uuid;

const PART: &str = "SHAFT-7";
const PRED: &str = "mfg.dimension.diameter";

struct S {
    c: Ctx,
    t: Uuid,
    a: String,
    b: String,
    a_id: Uuid,
    b_id: Uuid,
}

async fn setup() -> S {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (a_id, ea) = c.user(t, "reviewer").await;
    let (b_id, eb) = c.user(t, "reviewer").await;
    let a = c.mfa_token(&slug, &ea).await;
    let b = c.mfa_token(&slug, &eb).await;
    S { c, t, a, b, a_id, b_id }
}

async fn claim(s: &S, pred: &str, v: f64) -> String {
    let r = s
        .c
        .post(
            "/api/v1/claims",
            Some(&s.a),
            json!({"subject_type":"part","subject_key":PART,"predicate":pred,"value_num":format!("{v:.3}"),"value_unit":"mm","scope":{"supplier":"ACME"}}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body["id"].as_str().unwrap().to_owned()
}

/// 12 stable points and one far outlier: out of control by WE1.
async fn spc_series(s: &S) -> Vec<String> {
    let mut ids = Vec::new();
    for i in 0..12 {
        ids.push(claim(s, PRED, 10.0 + [0.0, 0.1, -0.1][i % 3]).await);
    }
    ids.push(claim(s, PRED, 12.5).await);
    ids
}

fn path(case: &str, rest: &str) -> String {
    format!("/api/v1/capa/cases/{case}{rest}")
}

async fn put(s: &S, case: &str, d: u8, content: Value) -> Value {
    let r =
        s.c.call(
            Method::PUT,
            &path(case, &format!("/steps/{d}")),
            Some(&s.a),
            Some(json!({ "content": content })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "D{d}: {:?}", r.body);
    r.body
}

async fn complete(s: &S, case: &str, d: u8) -> (StatusCode, Value) {
    let r =
        s.c.post(&path(case, &format!("/steps/{d}/complete")), Some(&s.a), json!({}))
            .await;
    (r.status, r.body)
}

async fn ok_complete(s: &S, case: &str, d: u8) -> Value {
    let (st, b) = complete(s, case, d).await;
    assert_eq!(st, StatusCode::OK, "D{d}: {b:?}");
    b
}

fn missing(detail: &Value, d: u8) -> Vec<String> {
    detail["steps"][usize::from(d) - 1]["missing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_owned())
        .collect()
}

fn cite(id: &str) -> Value {
    json!({ "claim": id })
}

/// Drives D1–D5 with valid content (cause and containment cite `evidence`), adds one action owned by B and
/// records its implementation. Returns the action id.
async fn through_d5(s: &S, case: &str, evidence: &str) -> String {
    put(
        s,
        case,
        1,
        json!({"members":[{"user":s.a_id,"role":"leader"},{"user":s.b_id,"role":"member"}]}),
    )
    .await;
    ok_complete(s, case, 1).await;
    put(
        s,
        case,
        2,
        json!({"statement":"Diameter outside limits on lot 13","is_is_not":[{"dimension":"what","is":"SHAFT-7 diameter","is_not":"length"}],"evidence":[cite(evidence)]}),
    )
    .await;
    ok_complete(s, case, 2).await;
    put(
        s,
        case,
        3,
        json!({"items":[{"description":"100% sort of stock","evidence":[cite(evidence)]}]}),
    )
    .await;
    ok_complete(s, case, 3).await;
    put(
        s,
        case,
        4,
        json!({"items":[{"kind":"occurrence","description":"Worn grinding wheel","evidence":[cite(evidence)]}]}),
    )
    .await;
    ok_complete(s, case, 4).await;
    let today = time::OffsetDateTime::now_utc().date().to_string();
    let d = s
        .c
        .post(
            &path(case, "/actions"),
            Some(&s.a),
            json!({"description":"Replace wheel and add dressing interval","owner":s.b_id,"due_date":today,"causes":[0]}),
        )
        .await;
    assert_eq!(d.status, StatusCode::OK, "{:?}", d.body);
    let action = d.body["actions"][0]["id"].as_str().unwrap().to_owned();
    // Implementation is recorded only after D5 is decided.
    let early =
        s.c.post(
            &format!("/api/v1/capa/actions/{action}/done"),
            Some(&s.a),
            json!({"evidence":[cite(evidence)]}),
        )
        .await;
    assert_eq!(early.body["error"], "actions_not_decided");
    ok_complete(s, case, 5).await;
    let none =
        s.c.post(
            &format!("/api/v1/capa/actions/{action}/done"),
            Some(&s.a),
            json!({"evidence":[]}),
        )
        .await;
    assert_eq!(none.body["error"], "evidence_required");
    let done =
        s.c.post(
            &format!("/api/v1/capa/actions/{action}/done"),
            Some(&s.a),
            json!({"evidence":[cite(evidence)],"comment":"wheel replaced"}),
        )
        .await;
    assert_eq!(done.status, StatusCode::OK, "{:?}", done.body);
    action
}

async fn d7_d8_close(s: &S, case: &str) -> Value {
    put(
        s,
        case,
        7,
        json!({"items":[{"kind":"control_plan","description":"Dressing interval in control plan"}],"horizontal":[{"target":"Line 2 grinders"}]}),
    )
    .await;
    ok_complete(s, case, 7).await;
    let early = s.c.post(&path(case, "/closure/request"), Some(&s.a), json!({})).await;
    assert_eq!(early.body["error"], "step_requirements_unmet");
    put(s, case, 8, json!({"statement":"Effective; closed with team"})).await;
    let r = s.c.post(&path(case, "/closure/request"), Some(&s.a), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["case"]["status"], "closure_pending");
    let own = s.c.post(&path(case, "/closure/approve"), Some(&s.a), json!({})).await;
    assert_eq!(
        (own.status, own.body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("own_closure_request"))
    );
    let r =
        s.c.post(
            &path(case, "/closure/approve"),
            Some(&s.b),
            json!({"comment":"checked"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["case"]["status"], "closed");
    r.body
}

fn event_kinds(d: &Value) -> Vec<String> {
    d["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn spc_case_full_lifecycle_and_reopen_on_new_out_of_control_point() {
    let s = setup().await;
    let series = spc_series(&s).await;
    let outlier = series.last().unwrap().clone();
    // In-control characteristic: nothing to correct.
    for i in 0..10 {
        claim(&s, "mfg.dimension.length", 50.0 + [0.0, 0.1, -0.1][i % 3]).await;
    }
    let calm = s
        .c
        .post("/api/v1/capa/cases", Some(&s.a), json!({"source_kind":"spc","spc":{"subject_type":"part","subject_key":PART,"predicate":"mfg.dimension.length"}}))
        .await;
    assert_eq!(calm.body["error"], "spc_in_control");

    let open = json!({"source_kind":"spc","spc":{"subject_type":"part","subject_key":PART,"predicate":PRED}});
    let r = s.c.post("/api/v1/capa/cases", Some(&s.a), open.clone()).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let case = r.body["case"]["id"].as_str().unwrap().to_owned();
    assert!(
        r.body["source_links"].as_array().unwrap().contains(&cite(&outlier)),
        "carries the violating point"
    );
    assert!(
        r.body["source"]["violations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["rule"] == "WE1" && v["claim"] == outlier.as_str()),
        "{:?}",
        r.body["source"]
    );
    assert_eq!(r.body["case"]["supplier_key"], "ACME");
    assert_eq!(
        r.body["steps"][1]["content"]["evidence"], r.body["source_links"],
        "D2 starts anchored to the finding"
    );
    let dup = s.c.post("/api/v1/capa/cases", Some(&s.a), open).await;
    assert_eq!(dup.body["error"], "case_already_open");

    // Order and requirements.
    let (st, b) = complete(&s, &case, 3).await;
    assert_eq!(
        (st, b["error"].as_str()),
        (StatusCode::CONFLICT, Some("earlier_step_open"))
    );
    let (_, b) = complete(&s, &case, 1).await;
    assert_eq!(b["error"], "step_requirements_unmet");
    let g = s.c.get(&path(&case, ""), &s.a).await;
    assert!(missing(&g.body, 1).contains(&"team_needs_two_people".to_string()));
    let bad =
        s.c.call(
            Method::PUT,
            &path(&case, "/steps/4"),
            Some(&s.a),
            Some(json!({"content":{"causes":[]}})),
        )
        .await;
    assert_eq!(bad.body["error"], "invalid_content");
    let bad =
        s.c.call(
            Method::PUT,
            &path(&case, "/steps/9"),
            Some(&s.a),
            Some(json!({"content":{}})),
        )
        .await;
    assert_eq!(bad.body["error"], "invalid_discipline");
    // A cause without a citation cannot be completed.
    put(
        &s,
        &case,
        4,
        json!({"items":[{"kind":"occurrence","description":"guess","evidence":[]}]}),
    )
    .await;
    let g = s.c.get(&path(&case, ""), &s.a).await;
    assert!(missing(&g.body, 4).contains(&"root_cause_evidence_required".to_string()));

    through_d5(&s, &case, &outlier).await;

    // D6: old evidence and too few new points are refused.
    put(
        &s,
        &case,
        6,
        json!({"statement":"Back in control","evidence":[cite(&series[0])],"spc":true}),
    )
    .await;
    let g = s.c.get(&path(&case, ""), &s.a).await;
    let m = missing(&g.body, 6);
    assert!(m.contains(&"verification_evidence_not_new".to_string()), "{m:?}");
    assert!(m.contains(&"spc_too_few_new_points".to_string()), "{m:?}");
    let mut fresh = Vec::new();
    for i in 0..8 {
        fresh.push(claim(&s, PRED, 10.0 + [0.0, 0.05, -0.05][i % 3]).await);
    }
    put(
        &s,
        &case,
        6,
        json!({"statement":"Back in control","evidence":[cite(&fresh[7])],"spc":true}),
    )
    .await;
    let g = s.c.get(&path(&case, ""), &s.a).await;
    assert_eq!(g.body["spc_recheck"], "incontrol");
    ok_complete(&s, &case, 6).await;
    let closed = d7_d8_close(&s, &case).await;
    let kinds = event_kinds(&closed);
    for k in [
        "capa.opened",
        "capa.step_completed",
        "capa.action_added",
        "capa.action_done",
        "capa.closure_requested",
        "capa.closed",
    ] {
        assert!(kinds.iter().any(|x| x == k), "missing {k}: {kinds:?}");
    }
    let audit: i64 = scalar(
        &s,
        "SELECT count(*) FROM audit_events WHERE action LIKE 'capa.%' AND target_id = $1",
        &case,
    )
    .await;
    assert!(audit >= 15, "transitions audited: {audit}");
    // Closed: no more edits.
    let edit =
        s.c.call(
            Method::PUT,
            &path(&case, "/steps/7"),
            Some(&s.a),
            Some(json!({"content":{}})),
        )
        .await;
    assert_eq!(edit.body["error"], "case_not_open");

    // A later point beyond the recorded limits contradicts the verification: the case reopens.
    claim(&s, PRED, 9.0).await;
    let g = s.c.get(&path(&case, ""), &s.a).await;
    assert_eq!(
        (g.body["case"]["status"].as_str(), g.body["case"]["reopened"].as_i64()),
        (Some("open"), Some(1))
    );
    let st: Vec<&str> = g.body["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["status"].as_str().unwrap())
        .collect();
    assert_eq!(st, ["done", "done", "done", "done", "done", "open", "open", "open"]);
    let ev = g.body["events"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(
        (
            ev["kind"].as_str(),
            ev["actor_kind"].as_str(),
            ev["detail"]["cause"].as_str()
        ),
        (Some("capa.reopened"), Some("system"), Some("spc_point_beyond_limits"))
    );
    // An ordinary point does not reopen anything further.
    claim(&s, PRED, 10.0).await;
    let g = s.c.get(&path(&case, ""), &s.a).await;
    assert_eq!(g.body["case"]["reopened"], 1);
}

async fn scalar(s: &S, sql: &str, arg: &str) -> i64 {
    let mut tx = kizuna_api::db::tenant_tx(&s.c.owner, s.t).await.unwrap();
    let n: i64 = sqlx::query_scalar(sql).bind(arg).fetch_one(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    n
}

/// A conflict between two claims, written directly (the conflict engine is covered elsewhere).
async fn conflict(s: &S, a: &str, b: &str) -> Uuid {
    let id = Uuid::now_v7();
    let mut tx = kizuna_api::db::tenant_tx(&s.c.owner, s.t).await.unwrap();
    sqlx::query(
        "INSERT INTO conflicts (id, tenant_id, kind, subject_type, subject_key, claim_a, claim_b, explanation, rule_version) \
         VALUES ($1,$2,'value_mismatch','part',$3,$4::uuid,$5::uuid,'{\"why\":\"differs\"}','t')",
    )
    .bind(id)
    .bind(s.t)
    .bind(PART)
    .bind(a)
    .bind(b)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

#[tokio::test]
async fn conflict_case_supplier_response_is_evidence_not_approval_and_correction_reopens() {
    let s = setup().await;
    let a = claim(&s, "mfg.material.hardness", 30.0).await;
    let b = claim(&s, "mfg.material.hardness", 45.0).await;
    let cf = conflict(&s, &a, &b).await;
    let r =
        s.c.post(
            "/api/v1/capa/cases",
            Some(&s.a),
            json!({"source_kind":"conflict","source_id":cf}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let case = r.body["case"]["id"].as_str().unwrap().to_owned();
    assert_eq!(r.body["source_links"], json!([cite(&a), cite(&b)]));
    assert_eq!(r.body["case"]["supplier_key"], "ACME");
    let unknown =
        s.c.post(
            "/api/v1/capa/cases",
            Some(&s.a),
            json!({"source_kind":"conflict","source_id":Uuid::now_v7()}),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    let bad =
        s.c.post(
            "/api/v1/capa/cases",
            Some(&s.a),
            json!({"source_kind":"rumour","source_id":cf}),
        )
        .await;
    assert_eq!(bad.body["error"], "invalid_source_kind");

    // Supplier request: only after the problem is described, approved by a second person, never sent by KIZUNA.
    let early =
        s.c.post(
            &path(&case, "/supplier-requests"),
            Some(&s.a),
            json!({"due_date":"2099-01-31"}),
        )
        .await;
    assert_eq!(early.body["error"], "problem_not_described");
    put(
        &s,
        &case,
        1,
        json!({"members":[{"user":s.a_id,"role":"leader"},{"user":s.b_id,"role":"member"}]}),
    )
    .await;
    ok_complete(&s, &case, 1).await;
    put(
        &s,
        &case,
        2,
        json!({"statement":"Hardness differs between certificate and inspection","is_is_not":[{"dimension":"what","is":"hardness","is_not":"chemistry"}],"evidence":[cite(&a)]}),
    )
    .await;
    ok_complete(&s, &case, 2).await;
    let r =
        s.c.post(
            &path(&case, "/supplier-requests"),
            Some(&s.a),
            json!({"due_date":"2099-01-31","disciplines":[3,4,5]}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let req = r.body["supplier_requests"][0]["id"].as_str().unwrap().to_owned();
    let doc =
        s.c.get(&format!("/api/v1/capa/supplier-requests/{req}/document?lang=ja"), &s.a)
            .await;
    let text = doc.body.as_str().unwrap();
    assert!(
        text.starts_with("【下書き・未承認／送付不可】")
            && text.contains("是正処置報告書")
            && text.contains("暫定処置"),
        "{text}"
    );
    let upload =
        s.c.upload(
            &s.a,
            "acme-8d.pdf",
            &make_pdf(&["ACME 8D report", "Root cause: furnace drift"]),
            Some("other"),
        )
        .await;
    assert_eq!(upload.status, StatusCode::OK, "{:?}", upload.body);
    let art = upload.body["artifact"]["id"].as_str().unwrap().to_owned();
    let not_yet =
        s.c.post(
            &path(&case, "/supplier-responses"),
            Some(&s.a),
            json!({"request":req,"artifact":art}),
        )
        .await;
    assert_eq!(not_yet.body["error"], "request_not_approved");
    let own =
        s.c.post(
            &format!("/api/v1/capa/supplier-requests/{req}/approve"),
            Some(&s.a),
            json!({}),
        )
        .await;
    assert_eq!(
        (own.status, own.body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("own_request"))
    );
    let ok =
        s.c.post(
            &format!("/api/v1/capa/supplier-requests/{req}/approve"),
            Some(&s.b),
            json!({}),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.body);
    let doc =
        s.c.get(&format!("/api/v1/capa/supplier-requests/{req}/document?lang=en"), &s.a)
            .await;
    assert!(doc.body.as_str().unwrap().starts_with("To: ACME"));
    let r =
        s.c.post(
            &path(&case, "/supplier-responses"),
            Some(&s.a),
            json!({"request":req,"artifact":art}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["evidence"][&art]["origin"], "supplier");
    assert_eq!(
        r.body["steps"][2]["status"], "open",
        "a supplier response completes nothing"
    );

    let action = through_d5_from_d3(&s, &case, &a).await;
    assert!(!action.is_empty());
    // Evidence from before the fix is not a verification.
    put(
        &s,
        &case,
        6,
        json!({"statement":"Supplier says fixed","evidence":[{"artifact":art}]}),
    )
    .await;
    let g = s.c.get(&path(&case, ""), &s.a).await;
    assert!(missing(&g.body, 6).contains(&"verification_evidence_not_new".to_string()));
    // The supplier's own follow-up report alone does not verify effectiveness, even when new.
    let up =
        s.c.upload(
            &s.a,
            "acme-8d-v2.pdf",
            &make_pdf(&["ACME 8D follow-up", "Effective"]),
            Some("other"),
        )
        .await;
    let art2 = up.body["artifact"]["id"].as_str().unwrap().to_owned();
    let r =
        s.c.post(
            &path(&case, "/supplier-responses"),
            Some(&s.a),
            json!({"request":req,"artifact":art2}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    put(
        &s,
        &case,
        6,
        json!({"statement":"Supplier says fixed","evidence":[{"artifact":art},{"artifact":art2}]}),
    )
    .await;
    let g = s.c.get(&path(&case, ""), &s.a).await;
    let m = missing(&g.body, 6);
    assert!(
        m.contains(&"verification_needs_independent_evidence".to_string()),
        "{m:?}"
    );
    let inspection = claim(&s, "mfg.material.hardness", 31.0).await;
    put(
        &s,
        &case,
        6,
        json!({"statement":"Incoming inspection confirms","evidence":[{"artifact":art},cite(&inspection)]}),
    )
    .await;
    ok_complete(&s, &case, 6).await;
    // Reject closure needs a reason and goes back to open.
    put(&s, &case, 7, json!({"items":[{"kind":"standard","description":"Furnace calibration standard"}],"horizontal_na_reason":"single furnace"})).await;
    ok_complete(&s, &case, 7).await;
    put(&s, &case, 8, json!({"statement":"done"})).await;
    s.c.post(&path(&case, "/closure/request"), Some(&s.a), json!({})).await;
    let r = s.c.post(&path(&case, "/closure/reject"), Some(&s.b), json!({})).await;
    assert_eq!(r.body["error"], "comment_required");
    let r =
        s.c.post(
            &path(&case, "/closure/reject"),
            Some(&s.b),
            json!({"comment":"attach the calibration record"}),
        )
        .await;
    assert_eq!(r.body["case"]["status"], "open");
    let closed = d7_d8_close_from_d8(&s, &case).await;
    assert_eq!(closed["case"]["closed_by"], s.b_id.to_string());

    // Correcting the verification evidence (selective invalidation) reopens the closed case.
    let r = s
        .c
        .post(
            &format!("/api/v1/claims/{inspection}/supersede"),
            Some(&s.a),
            json!({"subject_type":"part","subject_key":PART,"predicate":"mfg.material.hardness","value_num":"44","value_unit":"mm","scope":{"supplier":"ACME"},"reason":"misread"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let g = s.c.get(&path(&case, ""), &s.a).await;
    assert_eq!(
        (g.body["case"]["status"].as_str(), g.body["case"]["reopened"].as_i64()),
        (Some("open"), Some(1))
    );
    assert_eq!(
        g.body["events"].as_array().unwrap().last().unwrap()["detail"]["cause"],
        "verification_evidence_superseded"
    );
    assert!(
        missing(&g.body, 6).contains(&"verification_evidence_required".to_string()),
        "superseded citation no longer usable"
    );
    // Reopening a step manually needs a reason and cascades.
    let r = s.c.post(&path(&case, "/steps/4/reopen"), Some(&s.a), json!({})).await;
    assert_eq!(r.body["error"], "comment_required");
    let r =
        s.c.post(
            &path(&case, "/steps/4/reopen"),
            Some(&s.a),
            json!({"comment":"second cause found"}),
        )
        .await;
    let st: Vec<&str> = r.body["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["status"].as_str().unwrap())
        .collect();
    assert_eq!(st, ["done", "done", "done", "open", "open", "open", "open", "open"]);
}

async fn through_d5_from_d3(s: &S, case: &str, evidence: &str) -> String {
    put(
        s,
        case,
        3,
        json!({"items":[{"description":"Quarantine lot","evidence":[cite(evidence)]}]}),
    )
    .await;
    ok_complete(s, case, 3).await;
    put(s, case, 4, json!({"items":[{"kind":"occurrence","description":"Furnace drift","evidence":[cite(evidence)]},{"kind":"escape","description":"No hardness check at receiving","evidence":[cite(evidence)]}]})).await;
    ok_complete(s, case, 4).await;
    let today = time::OffsetDateTime::now_utc().date().to_string();
    let r =
        s.c.post(
            &path(case, "/actions"),
            Some(&s.a),
            json!({"description":"Recalibrate","owner":s.b_id,"due_date":today,"causes":[0]}),
        )
        .await;
    let action = r.body["actions"][0]["id"].as_str().unwrap().to_owned();
    let bad =
        s.c.post(
            &path(case, "/actions"),
            Some(&s.a),
            json!({"description":"x","owner":s.b_id,"due_date":today,"causes":[5]}),
        )
        .await;
    assert_eq!(bad.body["error"], "invalid_cause");
    let (_, b) = complete(s, case, 5).await;
    assert_eq!(b["error"], "step_requirements_unmet", "escape cause has no action yet");
    let r =
        s.c.post(
            &path(case, "/actions"),
            Some(&s.a),
            json!({"description":"Receiving hardness check","owner":s.a_id,"due_date":today,"causes":[1]}),
        )
        .await;
    let action2 = r.body["actions"][1]["id"].as_str().unwrap().to_owned();
    ok_complete(s, case, 5).await;
    for x in [&action, &action2] {
        let r =
            s.c.post(
                &format!("/api/v1/capa/actions/{x}/done"),
                Some(&s.a),
                json!({"evidence":[cite(evidence)]}),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    }
    action
}

async fn d7_d8_close_from_d8(s: &S, case: &str) -> Value {
    let r = s.c.post(&path(case, "/closure/request"), Some(&s.a), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let r = s.c.post(&path(case, "/closure/approve"), Some(&s.b), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body
}

#[tokio::test]
async fn overdue_actions_notify_owner_and_opener_once() {
    let s = setup().await;
    spc_series(&s).await;
    let r =
        s.c.post(
            "/api/v1/capa/cases",
            Some(&s.a),
            json!({"source_kind":"spc","spc":{"subject_type":"part","subject_key":PART,"predicate":PRED}}),
        )
        .await;
    let case = r.body["case"]["id"].as_str().unwrap().to_owned();
    let ev = r.body["source_links"][0]["claim"].as_str().unwrap().to_owned();
    put(
        &s,
        &case,
        1,
        json!({"members":[{"user":s.a_id,"role":"leader"},{"user":s.b_id,"role":"member"}]}),
    )
    .await;
    ok_complete(&s, &case, 1).await;
    put(
        &s,
        &case,
        2,
        json!({"statement":"x","is_is_not":[{"dimension":"what","is":"a","is_not":"b"}],"evidence":[cite(&ev)]}),
    )
    .await;
    ok_complete(&s, &case, 2).await;
    put(
        &s,
        &case,
        3,
        json!({"items":[{"description":"sort","evidence":[cite(&ev)]}]}),
    )
    .await;
    ok_complete(&s, &case, 3).await;
    put(
        &s,
        &case,
        4,
        json!({"items":[{"kind":"occurrence","description":"c","evidence":[cite(&ev)]}]}),
    )
    .await;
    ok_complete(&s, &case, 4).await;
    // Due dates are judged on the tenant's calendar day (Asia/Tokyo by default), not the server's UTC date.
    let today = {
        let mut tx = kizuna_api::db::tenant_tx(&s.c.owner, s.t).await.unwrap();
        let d = kizuna_api::db::tenant_today(&mut tx, s.t, time::OffsetDateTime::now_utc())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        d
    };
    let past =
        s.c.post(
            &path(&case, "/actions"),
            Some(&s.a),
            json!({"description":"a","owner":s.b_id,"due_date":"2000-01-01","causes":[0]}),
        )
        .await;
    assert_eq!(past.body["error"], "due_date_before_case");
    s.c.post(
        &path(&case, "/actions"),
        Some(&s.a),
        json!({"description":"Fix the wheel","owner":s.b_id,"due_date":today.to_string(),"causes":[0]}),
    )
    .await;
    let none = s.c.post("/api/v1/capa/overdue/run", Some(&s.a), json!({})).await;
    assert_eq!(none.body["notifications_new"], 0, "not overdue on its due date");

    let later = today.next_day().unwrap().next_day().unwrap();
    let mut tx = kizuna_api::db::tenant_tx(&s.c.st.pool, s.t).await.unwrap();
    let n = kizuna_api::capa::sweep_overdue(&mut tx, s.t, later).await.unwrap();
    let again = kizuna_api::capa::sweep_overdue(&mut tx, s.t, later).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!((n, again), (2, 0), "owner and opener, once");
    let mine = s.c.get("/api/v1/capa/notifications", &s.b).await;
    assert_eq!(mine.body.as_array().unwrap().len(), 1);
    assert_eq!(mine.body[0]["kind"], "action_overdue");
    let nid = mine.body[0]["id"].as_str().unwrap();
    let theirs =
        s.c.post(&format!("/api/v1/capa/notifications/{nid}/read"), Some(&s.a), json!({}))
            .await;
    assert_eq!(theirs.status, StatusCode::NOT_FOUND, "only the recipient marks it read");
    let r =
        s.c.post(&format!("/api/v1/capa/notifications/{nid}/read"), Some(&s.b), json!({}))
            .await;
    assert_eq!(r.status, StatusCode::OK);
    let audit: i64 = scalar(
        &s,
        "SELECT count(*) FROM audit_events WHERE action = 'capa.action_overdue' AND target_id = $1",
        &case,
    )
    .await;
    assert_eq!(audit, 1);
}

#[tokio::test]
async fn tenant_isolation_permissions_and_force_rls() {
    let s = setup().await;
    spc_series(&s).await;
    let r =
        s.c.post(
            "/api/v1/capa/cases",
            Some(&s.a),
            json!({"source_kind":"spc","spc":{"subject_type":"part","subject_key":PART,"predicate":PRED}}),
        )
        .await;
    let case = r.body["case"]["id"].as_str().unwrap().to_owned();

    let (_, slug2, _, other) = s.c.admin().await;
    assert!(!slug2.is_empty());
    assert_eq!(s.c.get(&path(&case, ""), &other).await.status, StatusCode::NOT_FOUND);
    assert_eq!(s.c.get("/api/v1/capa/cases", &other).await.body, json!([]));
    let put =
        s.c.call(
            Method::PUT,
            &path(&case, "/steps/1"),
            Some(&other),
            Some(json!({"content":{}})),
        )
        .await;
    assert_eq!(put.status, StatusCode::NOT_FOUND);
    // Another tenant cannot open a case from this tenant's finding either.
    let spc =
        s.c.post(
            "/api/v1/capa/cases",
            Some(&other),
            json!({"source_kind":"spc","spc":{"subject_type":"part","subject_key":PART,"predicate":PRED}}),
        )
        .await;
    assert_eq!(spc.body["error"], "spc_too_few_points");

    let slug: String = sqlx::query_scalar("SELECT slug FROM tenants WHERE id = $1")
        .bind(s.t)
        .fetch_one(&s.c.owner)
        .await
        .unwrap();
    let (_, ev) = s.c.user(s.t, "viewer").await;
    let viewer = s.c.token(&slug, &ev).await;
    assert_eq!(s.c.get(&path(&case, ""), &viewer).await.status, StatusCode::OK);
    let w =
        s.c.call(
            Method::PUT,
            &path(&case, "/steps/1"),
            Some(&viewer),
            Some(json!({"content":{}})),
        )
        .await;
    assert_eq!(w.status, StatusCode::FORBIDDEN);
    // Closure approval is privileged: a reviewer without an MFA session cannot approve.
    let (_, er) = s.c.user(s.t, "reviewer").await;
    let plain = s.c.token(&slug, &er).await;
    let ap =
        s.c.post(&path(&case, "/closure/approve"), Some(&plain), json!({}))
            .await;
    assert_eq!(ap.status, StatusCode::FORBIDDEN);

    let forced: Vec<(String, bool)> = sqlx::query_as(
        "SELECT relname::text, relforcerowsecurity FROM pg_class WHERE relname LIKE 'capa\\_%' AND relkind = 'r' ORDER BY 1",
    )
    .fetch_all(&s.c.owner)
    .await
    .unwrap();
    assert_eq!(forced.len(), 8); // incl. capa_portal_links (migration 0029)
    assert!(forced.iter().all(|f| f.1), "{forced:?}");
    // The database itself refuses a self-approved closure.
    let mut tx = kizuna_api::db::tenant_tx(&s.c.owner, s.t).await.unwrap();
    let selfie = sqlx::query(
        "UPDATE capa_cases SET status = 'closed', closure_requested_by = $2, closed_by = $2 WHERE id = $1::uuid",
    )
    .bind(&case)
    .bind(s.a_id)
    .execute(&mut *tx)
    .await;
    assert!(selfie.is_err());
}

/// Stability soak 2026-11-06: two concurrent opens of the same finding gave one 200 and one 500 (unique index
/// violation). Now the loser gets 409 `case_already_open`.
#[tokio::test]
async fn concurrent_opens_of_one_finding_yield_one_case_and_a_conflict() {
    let s = setup().await;
    let a = claim(&s, "mfg.material.hardness", 30.0).await;
    let b = claim(&s, "mfg.material.hardness", 45.0).await;
    let cf = conflict(&s, &a, &b).await;
    let body = json!({"source_kind":"conflict","source_id":cf});
    for _ in 0..5 {
        let rs = futures_join(&s, &body).await;
        let ok = rs.iter().filter(|r| r.status == StatusCode::OK).count();
        assert!(ok <= 1, "{:?}", rs.iter().map(|r| r.status).collect::<Vec<_>>());
        for r in &rs {
            assert!(
                r.status == StatusCode::OK || r.body["error"] == "case_already_open",
                "{}: {:?}",
                r.status,
                r.body
            );
        }
    }
}

async fn futures_join(s: &S, body: &Value) -> Vec<Resp> {
    let (x, y, z) = tokio::join!(
        s.c.post("/api/v1/capa/cases", Some(&s.a), body.clone()),
        s.c.post("/api/v1/capa/cases", Some(&s.b), body.clone()),
        s.c.post("/api/v1/capa/cases", Some(&s.a), body.clone()),
    );
    vec![x, y, z]
}

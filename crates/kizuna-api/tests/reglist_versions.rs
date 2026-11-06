//! Regulation-list versions as dependency nodes (research I6, `mfg-reglist@1`, migration 0031): a new list version
//! flags only the decisions whose declarations could be affected, through selective invalidation; people re-review.
//!
//! All list content here is synthetic: substance names are invented and the CAS numbers are only shaped to pass the
//! check-digit rule. They are not real regulation-list entries.
#![allow(clippy::unwrap_used)]
mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};
use uuid::Uuid;

const V1: &str =
    "Substance name,CAS No.,Date added\nKizunium oxide,1234-56-6,2025-06-01\nTetrafoo,9001-01-8,2025-06-01\n";
const V2: &str = "Substance name,CAS No.,Date added\nKizunium oxide,1234-56-6,2025-06-01\nTetrafoo,9001-01-8,2025-06-01\n\
Imaginol A,9002-02-2,2026-01-15\nImaginol B,9003-03-6,2026-01-15\n";

const FMD: &str = "Part,Material,Substance name,CAS No.\nP-A,Resin A,,\nP-B,Steel B,,\nP-C,Mystery mix,,\n\
P-D,Coat D,Imaginol B,9003-03-6\n";

const DECL: &str = "Supplier,Part,List,List version,Status\nSynthetic Supplier,P-A,svhc,2025-06,compliant\n\
Synthetic Supplier,P-B,svhc,2025-06,compliant\nSynthetic Supplier,P-C,svhc,2025-06,compliant\n\
Synthetic Supplier,P-D,svhc,2025-06,compliant\n";

struct T {
    c: Ctx,
    tid: Uuid,
    slug: String,
    tok: String,
    admin: String,
}

async fn setup() -> T {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let (_, e) = c.user(t, "reviewer").await;
    let tok = c.mfa_token(&slug, &e).await;
    T {
        c,
        tid: t,
        slug,
        tok,
        admin,
    }
}

async fn upload(t: &T, name: &str, data: &str, kind: &str) -> String {
    let r = t.c.upload(&t.tok, name, data.as_bytes(), Some(kind)).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["artifact"]["status"], "accepted", "{:?}", r.body);
    r.body["artifact"]["id"].as_str().unwrap().to_owned()
}

async fn import_version(t: &T, list: &str, label: &str, csv: &str) -> (StatusCode, Value) {
    let a = upload(t, &format!("svhc-{label}.csv"), csv, "regulation_list").await;
    let r =
        t.c.post(
            &format!("/api/v1/reg-lists/{list}/versions"),
            Some(&t.tok),
            json!({ "label": label, "artifact_id": a }),
        )
        .await;
    (r.status, r.body)
}

async fn decide(t: &T, part: &str, claim: &str, upstream: Option<&str>) -> String {
    let r = t
        .c
        .post(
            "/api/v1/decisions",
            Some(&t.tok),
            json!({
                "kind": "po_line_acceptance", "title": format!("Accept {part}"), "subject_type": "part", "subject_key": part,
                "outcome": "approved", "rationale": "declaration checked",
                "evidence": [{ "claim_id": claim, "materiality": { "type": "any_change" } }],
                "depends_on": upstream.map(|u| vec![json!({ "decision_id": u })]).unwrap_or_default(),
            }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body["id"].as_str().unwrap().to_owned()
}

async fn status(t: &T, d: &str) -> String {
    let r = t.c.get(&format!("/api/v1/decisions/{d}"), &t.tok).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let s = r.body.get("decision").unwrap_or(&r.body)["status"]
        .as_str()
        .map(str::to_owned);
    s.unwrap_or_else(|| panic!("no status in {:?}", r.body))
}

/// Declaration claim ids by part.
async fn declarations(t: &T) -> std::collections::BTreeMap<String, String> {
    let r = t.c.get("/api/v1/claims?predicate=reg.declaration", &t.tok).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["predicate"] == "reg.declaration")
        .map(|c| {
            (
                c["subject_key"].as_str().unwrap().to_owned(),
                c["id"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn new_list_version_flags_only_possibly_affected_declarations() {
    let t = setup().await;
    let r =
        t.c.post(
            "/api/v1/reg-lists",
            Some(&t.tok),
            json!({ "code": "SVHC", "name": "Synthetic candidate list" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["code"], "svhc");
    let list = r.body["id"].as_str().unwrap().to_owned();
    let (s, v1) = import_version(&t, &list, "2025-06", V1).await;
    assert_eq!(s, StatusCode::OK, "{v1:?}");
    assert_eq!(v1["version"]["seq"], 1);
    assert_eq!(v1["version"]["entry_count"], 2);
    assert_eq!(v1["version"]["summary"]["declarations_considered"], 0);

    // Composition data and curated profiles: Resin A could contain Imaginol A; Steel B contains none of interest;
    // nobody has described "Mystery mix"; Coat D's own FMD declares Imaginol B.
    let fmd = upload(&t, "fmd.csv", FMD, "material_declaration").await;
    let r =
        t.c.post(
            "/api/v1/reg-lists/compositions",
            Some(&t.tok),
            json!({ "artifact_id": fmd }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["claims_created"], 4);
    for (m, subs) in [("resin a", json!(["9002-02-2"])), ("Steel B", json!([]))] {
        let r =
            t.c.call(
                axum::http::Method::PUT,
                "/api/v1/material-profiles",
                Some(&t.tok),
                Some(json!({ "material": m, "substances": subs })),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    }

    let decl = upload(&t, "decl.csv", DECL, "compliance_declaration").await;
    let r =
        t.c.post(
            "/api/v1/reg-lists/declarations",
            Some(&t.tok),
            json!({ "artifact_id": decl }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["claims_created"], 4);
    let again =
        t.c.post(
            "/api/v1/reg-lists/declarations",
            Some(&t.tok),
            json!({ "artifact_id": decl }),
        )
        .await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    assert_eq!(again.body["error"], "reglist_already_imported");

    let ids = declarations(&t).await;
    assert_eq!(ids.len(), 4, "{ids:?}");
    let da = decide(&t, "P-A", &ids["P-A"], None).await;
    let db = decide(&t, "P-B", &ids["P-B"], None).await;
    let dc = decide(&t, "P-C", &ids["P-C"], None).await;
    let dd = decide(&t, "P-D", &ids["P-D"], None).await;
    // Relies on P-A's acceptance (and on P-B's declaration, which stays unaffected): flagged through the edge only.
    let down = decide(&t, "ASSY-1", &ids["P-B"], Some(&da)).await;
    // An assembly decision that relies on P-B's acceptance only: must stay valid.
    let quiet = decide(&t, "P-B", &ids["P-B"], Some(&db)).await;

    // Version 2 adds two substances.
    let (s, v2) = import_version(&t, &list, "2026-01", V2).await;
    assert_eq!(s, StatusCode::OK, "{v2:?}");
    let sum = &v2["version"]["summary"];
    assert_eq!(v2["version"]["seq"], 2);
    assert_eq!(sum["declarations_considered"], 4, "{sum}");
    assert_eq!(sum["may_contain"], 2, "{sum}");
    assert_eq!(sum["cannot_rule_out"], 1, "{sum}");
    assert_eq!(sum["not_affected"], 1, "{sum}");
    // P-A (plus its downstream decision), P-C and P-D: never P-B or the decision built on it.
    assert_eq!(sum["decisions_flagged"], 4, "{sum}");
    for d in [&da, &dc, &dd, &down] {
        assert_eq!(status(&t, d).await, "potentially_affected", "{d}");
    }
    for d in [&db, &quiet] {
        assert_eq!(status(&t, d).await, "valid", "{d}");
    }

    // The invalidation event names the declaration claim, the list change and why.
    let mut tx = kizuna_api::db::tenant_tx(&t.c.owner, t.tid).await.unwrap();
    let ev: (String, Option<Uuid>, Value, String) = sqlx::query_as(
        "SELECT cause_kind, cause_claim, semantic_change, reason FROM invalidation_events WHERE decision_id = $1",
    )
    .bind(Uuid::parse_str(&da).unwrap())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(ev.0, "regulation_list_changed");
    assert_eq!(ev.1.unwrap().to_string(), ids["P-A"]);
    assert_eq!(ev.2["predicate"], "reg.list.svhc");
    assert_eq!(ev.2["from"], "2025-06");
    assert_eq!(ev.2["to"], "2026-01");
    assert_eq!(ev.2["assessment"]["verdict"], "may_contain");
    assert_eq!(ev.2["assessment"]["substances"], json!(["cas:9002-02-2"]));
    assert!(ev.3.contains("svhc") && ev.3.contains("Imaginol A"), "{}", ev.3);
    let down_ev: (String, Value) =
        sqlx::query_as("SELECT cause_kind, path FROM invalidation_events WHERE decision_id = $1")
            .bind(Uuid::parse_str(&down).unwrap())
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(down_ev.0, "upstream_decision");
    assert_eq!(down_ev.1, json!([ids["P-A"], da, down]));
    tx.commit().await.unwrap();

    // The version view: diff against the previous version and every declaration with its verdict.
    let vid = v2["version"]["id"].as_str().unwrap();
    let r =
        t.c.get(&format!("/api/v1/reg-lists/{list}/versions/{vid}"), &t.tok)
            .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let added: Vec<&str> = r.body["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["substance_key"].as_str().unwrap())
        .collect();
    assert_eq!(added, ["cas:9002-02-2", "cas:9003-03-6"]);
    assert_eq!(r.body["removed"], json!([]));
    let verdicts: std::collections::BTreeMap<String, String> = r.body["assessments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["part"].as_str().unwrap().to_owned(),
                a["verdict"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(verdicts["P-A"], "may_contain");
    assert_eq!(verdicts["P-B"], "not_affected");
    assert_eq!(verdicts["P-C"], "cannot_rule_out");
    assert_eq!(verdicts["P-D"], "may_contain");
    let listed = t.c.get("/api/v1/reg-lists", &t.tok).await;
    assert_eq!(listed.body[0]["versions"].as_array().unwrap().len(), 2);

    // Re-request planning treats the stale declaration as supplier evidence to ask for again.
    let r =
        t.c.get(&format!("/api/v1/evidence-requests/preview?decision_id={da}"), &t.tok)
            .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert!(
        r.body.to_string().contains("reg.declaration"),
        "declaration re-request expected: {}",
        r.body
    );

    // Same label twice: refused. Audit trail records the import.
    let (s, b) = import_version(&t, &list, "2026-01", V2).await;
    assert_eq!(s, StatusCode::CONFLICT, "{b:?}");
    assert_eq!(b["error"], "reglist_version_exists");
    let audit = t.c.get("/api/v1/audit/events", &t.admin).await;
    assert!(
        audit
            .body
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "reg_list.version_imported")
    );

    // Stored list versions and assessments are append-only.
    for sql in [
        "UPDATE reg_list_entries SET name = 'x'",
        "DELETE FROM reg_assessments",
        "DELETE FROM reg_declarations",
    ] {
        let mut tx = kizuna_api::db::tenant_tx(&t.c.owner, t.tid).await.unwrap();
        let r = sqlx::query(sql).execute(&mut *tx).await;
        assert!(r.is_err(), "{sql} must be refused");
    }
}

#[tokio::test]
async fn imports_are_validated_scoped_and_permissioned() {
    let t = setup().await;
    let r =
        t.c.post(
            "/api/v1/reg-lists",
            Some(&t.tok),
            json!({ "code": "rohs", "name": "Synthetic list" }),
        )
        .await;
    let list = r.body["id"].as_str().unwrap().to_owned();
    let dup =
        t.c.post(
            "/api/v1/reg-lists",
            Some(&t.tok),
            json!({ "code": "rohs", "name": "again" }),
        )
        .await;
    assert_eq!(dup.body["error"], "reglist_exists");
    let bad =
        t.c.post(
            "/api/v1/reg-lists",
            Some(&t.tok),
            json!({ "code": "Bad Code!", "name": "x" }),
        )
        .await;
    assert_eq!(bad.body["error"], "reglist_invalid_code");

    // Bad check digit, wrong document kind, unknown list version, extraction refused.
    let (s, b) = import_version(&t, &list, "v1", "name,cas\nFakeium,1234-56-7\n").await;
    assert_eq!(
        (s, b["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("reglist_invalid_cas"))
    );
    let decl = upload(&t, "d.csv", DECL, "compliance_declaration").await;
    let r =
        t.c.post(
            &format!("/api/v1/reg-lists/{list}/versions"),
            Some(&t.tok),
            json!({ "label": "v1", "artifact_id": decl }),
        )
        .await;
    assert_eq!(r.body["error"], "reglist_wrong_document_kind");
    let r =
        t.c.post(
            "/api/v1/reg-lists/declarations",
            Some(&t.tok),
            json!({ "artifact_id": decl }),
        )
        .await;
    assert_eq!(r.body["error"], "decl_unknown_list_version", "{:?}", r.body);
    assert!(declarations(&t).await.is_empty(), "a refused import creates no claims");
    let r =
        t.c.post(&format!("/api/v1/artifacts/{decl}/extract"), Some(&t.tok), json!({}))
            .await;
    assert_eq!(r.body["error"], "reglist_use_import");
    let r =
        t.c.call(
            axum::http::Method::PUT,
            "/api/v1/material-profiles",
            Some(&t.tok),
            Some(json!({ "material": "X", "substances": ["lead"] })),
        )
        .await;
    assert_eq!(r.body["error"], "reglist_invalid_substance");

    // Auditors read but cannot write; profiles need an MFA session (privileged).
    let (tid, slug) = (t.tid, t.slug.clone());
    let (_, aud) = t.c.user(tid, "auditor").await;
    let aud = t.c.token(&slug, &aud).await;
    assert_eq!(t.c.get("/api/v1/reg-lists", &aud).await.status, StatusCode::OK);
    let r =
        t.c.post("/api/v1/reg-lists", Some(&aud), json!({ "code": "x2", "name": "x" }))
            .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let (_, rv) = t.c.user(tid, "reviewer").await;
    let no_mfa = t.c.token(&slug, &rv).await;
    let r =
        t.c.call(
            axum::http::Method::PUT,
            "/api/v1/material-profiles",
            Some(&no_mfa),
            Some(json!({ "material": "X", "substances": [] })),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{:?}", r.body);

    // Another tenant sees nothing and cannot import into this list.
    let (t2, slug2) = t.c.tenant().await;
    let (_, other) = t.c.user(t2, "reviewer").await;
    let other = t.c.mfa_token(&slug2, &other).await;
    assert_eq!(t.c.get("/api/v1/reg-lists", &other).await.body, json!([]));
    let r =
        t.c.upload(&other, "l.csv", V1.as_bytes(), Some("regulation_list"))
            .await;
    let a = r.body["artifact"]["id"].as_str().unwrap();
    let r =
        t.c.post(
            &format!("/api/v1/reg-lists/{list}/versions"),
            Some(&other),
            json!({ "label": "v1", "artifact_id": a }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

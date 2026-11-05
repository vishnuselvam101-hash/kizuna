#![allow(clippy::unwrap_used)]
//! Quality three-way match and minimal supplier re-requests, end to end on real PostgreSQL with real extraction.
mod common;

use axum::http::StatusCode;
use common::*;
use kizuna_api::db;
use serde_json::{Value, json};

/// Owner-role statement inside the tenant (FORCE RLS applies to the owner too); returns whether it succeeded.
async fn owner_exec(s: &S, sql: &str) -> bool {
    let mut tx = db::tenant_tx(&s.c.owner, s.t).await.unwrap();
    let ok = sqlx::query(sql)
        .execute(&mut *tx)
        .await
        .is_ok_and(|r| r.rows_affected() > 0);
    if ok {
        tx.commit().await.unwrap();
    }
    ok
}

const PO_CSV: &[u8] = b"PO Number,Line,Part No,Drawing No,Rev,Material,Qty,Vendor,Certificate type\n\
PO-77,1,MX-100,DM-100,C,SUS 304,50,Nagoya Forge,EN 10204 3.1\n\
PO-77,2,MX-200,DM-200,B,S355JR,10,Nagoya Forge,EN 10204 3.2\n";

struct S {
    c: Ctx,
    t: uuid::Uuid,
    tok: String,
    rev2: String,
    viewer: String,
    ms1: String,
    ir1: String,
    ir2: String,
}

fn drawing(part: &str, dn: &str, rev: &str, mat: &str, len: &str) -> Vec<u8> {
    make_pdf(&[
        &format!("Drawing No: {dn}"),
        &format!("Part Number: {part}"),
        &format!("Rev: {rev}"),
        &format!("Material: {mat}"),
        &format!("Length (mm): {len}"),
        "Length tolerance: +/-0.1",
        "Tensile strength (MPa): min 520",
        "Approval Status: Released",
    ])
}

fn mill_sheet(part: &str, line: &str, ctype: &str, mat: &str, ts: &str) -> Vec<u8> {
    make_pdf(&[
        "MILL TEST CERTIFICATE",
        &format!("Part Number: {part}"),
        "PO Number: PO-77",
        &format!("PO Line: {line}"),
        "Heat No: H-881",
        &format!("Certificate type: {ctype}"),
        &format!("Material: {mat}"),
        &format!("Tensile strength (MPa): {ts}"),
        "Supplier: Nagoya Forge",
    ])
}

fn inspection(part: &str, line: &str, rev: &str, len: &str) -> Vec<u8> {
    make_pdf(&[
        "INSPECTION REPORT",
        &format!("Part Number: {part}"),
        "PO Number: PO-77",
        &format!("PO Line: {line}"),
        &format!("Lot: L-{line}"),
        &format!("Rev: {rev}"),
        &format!("Measured length (mm): {len}"),
        "Result: pass",
        "Supplier: Nagoya Forge",
    ])
}

/// Line 1 is clean; line 2 has an EN 10204 3.1 certificate where 3.2 was ordered and an out-of-tolerance length.
async fn setup() -> S {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, e) = c.user(t, "reviewer").await;
    let (_, e2) = c.user(t, "reviewer").await;
    let (_, ev) = c.user(t, "viewer").await;
    let tok = c.mfa_token(&slug, &e).await;
    let rev2 = c.mfa_token(&slug, &e2).await;
    let viewer = c.token(&slug, &ev).await;
    c.ingest(&tok, "po-77.csv", PO_CSV, "purchase_order").await;
    c.ingest(
        &tok,
        "dm-100.pdf",
        &drawing("MX-100", "DM-100", "C", "SUS304", "120"),
        "drawing",
    )
    .await;
    c.ingest(
        &tok,
        "dm-200.pdf",
        &drawing("MX-200", "DM-200", "B", "S355JR", "80"),
        "drawing",
    )
    .await;
    let ms1 = c
        .ingest(
            &tok,
            "ms-1.pdf",
            &mill_sheet("MX-100", "1", "EN 10204 3.1", "SUS304", "560"),
            "inspection_report",
        )
        .await;
    let ir1 = c
        .ingest(
            &tok,
            "ir-1.pdf",
            &inspection("MX-100", "1", "C", "120.04"),
            "inspection_report",
        )
        .await;
    c.ingest(
        &tok,
        "ms-2.pdf",
        &mill_sheet("MX-200", "2", "3.1", "S355JR", "540"),
        "inspection_report",
    )
    .await;
    let ir2 = c
        .ingest(
            &tok,
            "ir-2.pdf",
            &inspection("MX-200", "2", "B", "80.3"),
            "inspection_report",
        )
        .await;
    S {
        c,
        t,
        tok,
        rev2,
        viewer,
        ms1,
        ir1,
        ir2,
    }
}

async fn line(s: &S, n: &str) -> Value {
    let r =
        s.c.get(&format!("/api/v1/match/line?po=po-77&line={n}"), &s.viewer)
            .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body
}

fn kinds(v: &Value) -> Vec<String> {
    v["match"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap().to_owned())
        .collect()
}

async fn accept(s: &S, n: &str, outcome: &str) -> axum::http::StatusCode {
    s.c.post(
        "/api/v1/match/line/accept",
        Some(&s.tok),
        json!({ "po": "PO-77", "line": n, "outcome": outcome, "rationale": "3-way match reviewed" }),
    )
    .await
    .status
}

async fn accept_ok(s: &S, n: &str) -> String {
    let r =
        s.c.post(
            "/api/v1/match/line/accept",
            Some(&s.tok),
            json!({ "po": "PO-77", "line": n, "outcome": "approved", "rationale": "3-way match clean" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body["decision"]["id"].as_str().unwrap().to_owned()
}

async fn claim_of(s: &S, part: &str, pred: &str) -> Value {
    let r =
        s.c.get(&format!("/api/v1/claims?subject_type=part&subject_key={part}"), &s.tok)
            .await;
    r.body
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["predicate"] == pred && c["superseded_at"].is_null())
        .cloned()
        .unwrap_or_else(|| panic!("{pred}"))
}

async fn supersede(s: &S, old: &Value, num: Option<&str>, text: Option<&str>) {
    let mut b = json!({ "subject_type": "part", "subject_key": old["subject_key"], "predicate": old["predicate"],
        "revision": old["revision"], "scope": old["scope"], "value_unit": old["value_unit"], "reason": "drawing corrected" });
    if let Some(n) = num {
        b["value_num"] = json!(n);
    }
    if let Some(t) = text {
        b["value_text"] = json!(t);
    }
    let r =
        s.c.post(
            &format!("/api/v1/claims/{}/supersede", old["id"].as_str().unwrap()),
            Some(&s.tok),
            b,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
}

async fn status(s: &S, id: &str) -> String {
    s.c.get(&format!("/api/v1/decisions/{id}"), &s.tok).await.body["decision"]["status"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn three_way_match_cites_all_sides_and_gates_acceptance() {
    let s = setup().await;
    let r = s.c.get("/api/v1/match/lines", &s.viewer).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let lines = r.body["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(
        lines[0]["critical"].as_u64().unwrap() + lines[0]["major"].as_u64().unwrap(),
        0,
        "{:?}",
        lines[0]
    );
    assert_eq!(r.body["rule_version"], "mfg-match@1");

    let l1 = line(&s, "1").await;
    assert!(kinds(&l1).is_empty(), "{:?}", l1["match"]["findings"]);
    assert_eq!(l1["match"]["drawing_revision"], "C");
    assert_eq!(l1["match"]["certificates"].as_array().unwrap().len(), 2);
    for row in l1["match"]["checks"].as_array().unwrap() {
        assert!(
            matches!(row["status"].as_str().unwrap(), "match" | "not_required"),
            "{row:?}"
        );
    }

    let l2 = line(&s, "2").await;
    let k = kinds(&l2);
    assert_eq!(
        k,
        ["certificate_type_insufficient", "measurement_out_of_tolerance"],
        "{:?}",
        l2["match"]["findings"]
    );
    // Every cited claim resolves to its document and exact location.
    let cites = &l2["citations"];
    for f in l2["match"]["findings"].as_array().unwrap() {
        let sides = ["po_claims", "drawing_claims", "certificate_claims"];
        let cited: Vec<&str> = sides
            .iter()
            .flat_map(|side| f[side].as_array().unwrap().iter().map(|x| x.as_str().unwrap()))
            .collect();
        assert!(
            sides
                .iter()
                .filter(|side| !f[**side].as_array().unwrap().is_empty())
                .count()
                >= 2,
            "{f:?}"
        );
        for id in cited {
            let c = &cites["claims"][id];
            assert!(c["source_location"].as_object().is_some_and(|o| !o.is_empty()), "{c:?}");
            assert!(cites["documents"][c["artifact_id"].as_str().unwrap()]["display_name"].is_string());
        }
    }
    let oot = l2["match"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == "measurement_out_of_tolerance")
        .unwrap();
    assert_eq!(oot["certificate_doc"], json!(s.ir2));
    assert_eq!(oot["severity"], "critical");

    // Approval is refused while blocking findings are open; the reviewer may still reject or accept conditionally.
    assert_eq!(accept(&s, "2", "approved").await, StatusCode::CONFLICT);
    assert_eq!(accept(&s, "2", "rejected").await, StatusCode::OK);
    let r =
        s.c.post(
            "/api/v1/match/line/accept",
            Some(&s.viewer),
            json!({ "po": "PO-77", "line": "1", "outcome": "approved", "rationale": "x" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let d = accept_ok(&s, "1").await;
    let dec = s.c.get(&format!("/api/v1/decisions/{d}"), &s.viewer).await.body;
    assert_eq!(dec["decision"]["kind"], "po_line_acceptance");
    assert_eq!(dec["decision"]["subject_key"], "PO-77/1");
    let cited = l1["match"]["po_claims"].as_array().unwrap().len()
        + l1["match"]["drawing_claims"].as_array().unwrap().len()
        + l1["match"]["certificate_claims"].as_array().unwrap().len();
    assert_eq!(dec["decision"]["evidence_claim_ids"].as_array().unwrap().len(), cited);
    let mut tx = db::tenant_tx(&s.c.owner, s.t).await.unwrap();
    let roles: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT role FROM decision_dependencies WHERE decision_id = $1::uuid ORDER BY role",
    )
    .bind(&d)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(roles, ["basis", "condition"]);
    let l1b = line(&s, "1").await;
    assert_eq!(l1b["runs"][0]["decision_id"], json!(d));
    assert_eq!(l1b["runs"][0]["result_sha256"].as_str().unwrap().len(), 64);
    // Recorded runs are append-only, and findings of the rejected line were persisted with their sides.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM match_findings f JOIN match_runs r ON r.id = f.run_id WHERE r.po_line = '2' AND cardinality(f.certificate_claims) > 0")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 2);
    tx.commit().await.unwrap();
    assert!(
        !owner_exec(&s, "UPDATE match_runs SET blocking = 0").await,
        "append-only"
    );
    let r =
        s.c.post(
            "/api/v1/match/line/runs",
            Some(&s.tok),
            json!({ "po": "PO-77", "line": "2" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["blocking"], 2);

    // Unknown lines and other tenants see nothing.
    assert_eq!(
        s.c.get("/api/v1/match/line?po=PO-77&line=9", &s.viewer).await.status,
        StatusCode::NOT_FOUND
    );
    let (t2, slug2) = s.c.tenant().await;
    let (_, eo) = s.c.user(t2, "reviewer").await;
    let other = s.c.token(&slug2, &eo).await;
    assert_eq!(
        s.c.get("/api/v1/match/line?po=PO-77&line=1", &other).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(s.c.get("/api/v1/match/lines", &other).await.body["lines"], json!([]));

    // Evidence change on a cited drawing claim reaches the acceptance through its typed dependency.
    supersede(
        &s,
        &claim_of(&s, "MX-100", "mfg.tolerance.length.plus").await,
        Some("0.03"),
        None,
    )
    .await;
    assert_eq!(status(&s, &d).await, "potentially_affected");
}

#[tokio::test]
async fn minimal_rerequest_asks_only_for_stale_supplier_documents() {
    let s = setup().await;
    let d1 = accept_ok(&s, "1").await;
    // Tolerance tightened on the drawing → only the inspection report of line 1 must be resent (not the mill sheet).
    supersede(
        &s,
        &claim_of(&s, "MX-100", "mfg.tolerance.length.plus").await,
        Some("0.03"),
        None,
    )
    .await;
    assert_eq!(status(&s, &d1).await, "potentially_affected");
    let p = s.c.get("/api/v1/evidence-requests/preview", &s.viewer).await;
    assert_eq!(p.status, StatusCode::OK, "{:?}", p.body);
    let sup = &p.body["suppliers"][0];
    assert_eq!(sup["supplier"], "NAGOYA FORGE");
    let docs: Vec<&str> = sup["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["doc"].as_str().unwrap())
        .collect();
    assert_eq!(docs, [s.ir1.as_str()], "{sup:?}");
    assert_eq!(sup["baseline_count"], 2, "mill sheet + inspection report on file");
    assert_eq!(sup["exact"], true);
    assert_eq!(sup["documents"][0]["items"][0]["predicate"], "qa.measured.length");
    assert_eq!(
        sup["documents"][0]["reasons"][0]["predicate"],
        "mfg.tolerance.length.plus"
    );
    let _ = &s.ms1;

    // Draft (no sending), four-eyes approval, bilingual download, audit trail, immutability.
    let r = s.c.post("/api/v1/evidence-requests", Some(&s.tok), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let req = &r.body[0];
    let id = req["id"].as_str().unwrap().to_owned();
    assert_eq!(req["status"], "draft");
    assert_eq!(req["requested_count"], 1);
    let ja = req["body_ja"].as_str().unwrap();
    assert!(
        ja.contains("NAGOYA FORGE 御中") && ja.contains("ir-1.pdf") && !ja.contains("ms-1.pdf"),
        "{ja}"
    );
    assert!(ja.contains("測定値（長さ）") && ja.contains("再提出は不要"), "{ja}");
    let txt =
        s.c.get(&format!("/api/v1/evidence-requests/{id}/document?lang=en"), &s.viewer)
            .await;
    assert_eq!(txt.status, StatusCode::OK);
    assert!(txt.headers["content-type"].to_str().unwrap().starts_with("text/plain"));
    assert!(
        txt.headers["content-disposition"]
            .to_str()
            .unwrap()
            .contains("attachment")
    );
    assert!(
        txt.body
            .as_str()
            .unwrap()
            .starts_with("[DRAFT - NOT APPROVED - DO NOT SEND]")
    );
    let r =
        s.c.post(
            &format!("/api/v1/evidence-requests/{id}/approve"),
            Some(&s.tok),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "drafter cannot approve");
    let r =
        s.c.post(
            &format!("/api/v1/evidence-requests/{id}/approve"),
            Some(&s.viewer),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r =
        s.c.post(
            &format!("/api/v1/evidence-requests/{id}/approve"),
            Some(&s.rev2),
            json!({ "comment": "ok to send" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "approved");
    let txt =
        s.c.get(&format!("/api/v1/evidence-requests/{id}/document?lang=ja"), &s.viewer)
            .await;
    assert!(txt.body.as_str().unwrap().starts_with("NAGOYA FORGE 御中"));
    let r =
        s.c.post(
            &format!("/api/v1/evidence-requests/{id}/cancel"),
            Some(&s.rev2),
            json!({ "comment": "x" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(
        !owner_exec(&s, "UPDATE evidence_requests SET body_en = 'x'").await,
        "content immutable"
    );
    assert!(!owner_exec(&s, "DELETE FROM evidence_requests").await, "append-only");
    let mut tx = db::tenant_tx(&s.c.owner, s.t).await.unwrap();
    let actions: Vec<String> = sqlx::query_scalar("SELECT action FROM audit_events WHERE target_id = $1 ORDER BY seq")
        .bind(&id)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        actions,
        [
            "evidence_request.drafted",
            "evidence_request.exported",
            "evidence_request.approved",
            "evidence_request.exported"
        ]
    );
}

#[tokio::test]
async fn material_change_asks_for_mill_sheet_and_internal_change_asks_nothing() {
    let s = setup().await;
    let d1 = accept_ok(&s, "1").await;
    supersede(&s, &claim_of(&s, "MX-100", "mfg.material").await, None, Some("SUS316L")).await;
    assert_eq!(status(&s, &d1).await, "potentially_affected");
    let p =
        s.c.get(
            &format!("/api/v1/evidence-requests/preview?decision_id={d1}"),
            &s.viewer,
        )
        .await
        .body;
    let docs: Vec<&str> = p["suppliers"][0]["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["doc"].as_str().unwrap())
        .collect();
    assert_eq!(docs, [s.ms1.as_str()], "{p:?}");

    // An approval-status correction flags the decision but is not the supplier's to fix.
    let s2 = setup().await;
    let d = accept_ok(&s2, "1").await;
    supersede(
        &s2,
        &claim_of(&s2, "MX-100", "mfg.approval.status").await,
        None,
        Some("pending"),
    )
    .await;
    assert_eq!(status(&s2, &d).await, "potentially_affected");
    let p = s2.c.get("/api/v1/evidence-requests/preview", &s2.viewer).await.body;
    assert_eq!(p["suppliers"], json!([]), "{p:?}");
    assert_eq!(p["no_supplier_action"][0]["decision_id"], json!(d));
    let r = s2.c.post("/api/v1/evidence-requests", Some(&s2.tok), json!({})).await;
    assert_eq!(r.body, json!([]));
}

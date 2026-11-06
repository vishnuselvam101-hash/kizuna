#![allow(clippy::unwrap_used)]
//! Cross-standard grade guard (I9, `mfg-grade@1`) end to end on real PostgreSQL with real extraction. All grade
//! names are fictional (XQ…, K9…): KIZUNA ships no standards content, and neither do its tests.
mod common;

use axum::http::StatusCode;
use common::*;
use kizuna_api::db;
use serde_json::{Value, json};

const EXAMPLE: &str = include_str!("../../../docs/examples/grade-equivalences.example.csv");

const PO_CSV: &[u8] = b"PO Number,Line,Part No,Drawing No,Rev,Material,Qty,Vendor,Certificate type\n\
PO-91,1,MX-910,DM-910,A,JIS XQ202,5,Nagoya Forge,EN 10204 3.1\n";

struct S {
    c: Ctx,
    t: uuid::Uuid,
    tok: String,
    rev2: String,
    viewer: String,
}

async fn setup() -> S {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, e) = c.user(t, "reviewer").await;
    let (_, e2) = c.user(t, "reviewer").await;
    let (_, ev) = c.user(t, "viewer").await;
    let tok = c.mfa_token(&slug, &e).await;
    let rev2 = c.mfa_token(&slug, &e2).await;
    let viewer = c.token(&slug, &ev).await;
    S {
        c,
        t,
        tok,
        rev2,
        viewer,
    }
}

async fn ingest_line(s: &S) {
    s.c.ingest(&s.tok, "po-91.csv", PO_CSV, "purchase_order").await;
    let dw = make_pdf(&[
        "Drawing No: DM-910",
        "Part Number: MX-910",
        "Rev: A",
        "Material: JIS XQ202",
        "Length (mm): 50",
        "Length tolerance: +/-0.1",
        "Tensile strength (MPa): min 400",
        "Approval Status: Released",
    ]);
    s.c.ingest(&s.tok, "dm-910.pdf", &dw, "drawing").await;
    let ms = make_pdf(&[
        "MILL TEST CERTIFICATE",
        "Part Number: MX-910",
        "PO Number: PO-91",
        "PO Line: 1",
        "Heat No: H-77",
        "Certificate type: EN 10204 3.1",
        "Material: IS K9 220",
        "Tensile strength (MPa): 450",
        "Elongation (%): 18",
        "Supplier: Nagoya Forge",
    ]);
    s.c.ingest(&s.tok, "ms-91.pdf", &ms, "inspection_report").await;
    let ir = make_pdf(&[
        "INSPECTION REPORT",
        "Part Number: MX-910",
        "PO Number: PO-91",
        "PO Line: 1",
        "Lot: L-1",
        "Rev: A",
        "Measured length (mm): 50.02",
        "Result: pass",
        "Supplier: Nagoya Forge",
    ]);
    s.c.ingest(&s.tok, "ir-91.pdf", &ir, "inspection_report").await;
}

async fn line(s: &S) -> Value {
    let r = s.c.get("/api/v1/match/line?po=po-91&line=1", &s.viewer).await;
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

fn finding<'a>(v: &'a Value, kind: &str) -> &'a Value {
    v["match"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == kind)
        .unwrap_or_else(|| panic!("{kind} in {v:?}"))
}

async fn accept(s: &S, outcome: &str) -> (StatusCode, Value) {
    let r =
        s.c.post(
            "/api/v1/match/line/accept",
            Some(&s.tok),
            json!({ "po": "PO-91", "line": "1", "outcome": outcome, "rationale": "reviewed" }),
        )
        .await;
    (r.status, r.body)
}

async fn decision_status(s: &S, id: &str) -> String {
    s.c.get(&format!("/api/v1/decisions/{id}"), &s.tok).await.body["decision"]["status"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn cross_standard_substitution_is_a_recorded_decision_on_an_unverified_tenant_entry() {
    let s = setup().await;
    ingest_line(&s).await;

    // 1. KIZUNA ships an empty table: a certificate grade from another standard is a substitution to decide.
    let l = line(&s).await;
    assert_eq!(kinds(&l), ["substitution_requires_decision"], "{l:?}");
    let f = finding(&l, "substitution_requires_decision");
    assert_eq!(f["severity"], "critical");
    assert_eq!(f["explanation"]["detail"]["equivalence"], Value::Null);
    assert_eq!(f["explanation"]["detail"]["certificate_grade"]["standard"], "IS");
    assert_eq!(f["explanation"]["detail"]["required_grade"]["standard"], "JIS");
    assert!(!f["po_claims"].as_array().unwrap().is_empty() && !f["drawing_claims"].as_array().unwrap().is_empty());
    let cert_claim = f["certificate_claims"][0].as_str().unwrap().to_owned();
    assert_eq!(accept(&s, "approved").await.0, StatusCode::CONFLICT);
    // Without a tenant entry there is nothing a substitution decision could rest on.
    let sub = json!({ "po": "PO-91", "line": "1", "certificate_claim": cert_claim, "outcome": "approved",
                      "rationale": "impact test report attached" });
    let r =
        s.c.post("/api/v1/match/line/substitution", Some(&s.rev2), sub.clone())
            .await;
    assert_eq!(
        (r.status, r.body["error"].as_str()),
        (StatusCode::CONFLICT, Some("no_equivalence_entry"))
    );

    // 2. A tenant entry: proposed by one reviewer, labelled unverified, approved by another (four-eyes).
    let viewer_try =
        s.c.post(
            "/api/v1/grades/equivalences",
            Some(&s.viewer),
            json!({ "change": "create" }),
        )
        .await;
    assert_eq!(viewer_try.status, StatusCode::FORBIDDEN);
    let r =
        s.c.post(
            "/api/v1/grades/equivalences",
            Some(&s.tok),
            json!({ "change": "create", "a_standard": "JIS", "a_grade": "XQ202", "b_standard": "IS", "b_grade": "K9 220",
                    "equivalence": "acceptable_with_conditions", "conditions": "supplementary impact test",
                    "reference": "internal memo QA-1 (fictional)" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "proposed");
    assert_eq!(r.body["verification"], "unverified");
    assert!(r.body["note"].as_str().unwrap().contains("check against the standard"));
    let eq1 = r.body["id"].as_str().unwrap().to_owned();
    let entry = r.body["entry_id"].as_str().unwrap().to_owned();
    // Marking an entry as checked needs a reference to the standard document.
    let r =
        s.c.post(
            "/api/v1/grades/equivalences",
            Some(&s.tok),
            json!({ "change": "create", "a_grade": "XQ1", "b_grade": "ZR1", "equivalence": "identical", "source_checked": true }),
        )
        .await;
    assert_eq!(r.body["error"], "reference_required_when_checked");
    // A proposal is not effective.
    assert_eq!(kinds(&line(&s).await), ["substitution_requires_decision"]);
    let r =
        s.c.post(
            &format!("/api/v1/grades/equivalences/{eq1}/approve"),
            Some(&s.tok),
            json!({}),
        )
        .await;
    assert_eq!(
        (r.status, r.body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("four_eyes_required"))
    );
    let r =
        s.c.post(
            &format!("/api/v1/grades/equivalences/{eq1}/approve"),
            Some(&s.rev2),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "approved");

    // 3. Conditional entry: still a decision, now naming the entry, its conditions and its unverified status.
    let l = line(&s).await;
    let f = finding(&l, "substitution_requires_decision");
    assert_eq!(f["explanation"]["msg"]["code"], "match.substitution_conditional");
    let e = &f["explanation"]["detail"]["equivalence"];
    assert_eq!(e["id"], eq1.as_str());
    assert_eq!(e["conditions"], "supplementary impact test");
    assert_eq!(e["verification"], "unverified");
    assert!(e["note"].as_str().unwrap().contains("not an authoritative"));

    // 4. The substitution decision rests on both claims and the entry version.
    let r =
        s.c.post("/api/v1/match/line/substitution", Some(&s.rev2), sub.clone())
            .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let sub_id = r.body["decision"]["id"].as_str().unwrap().to_owned();
    assert_eq!(r.body["decision"]["kind"], "material_substitution");
    let l = line(&s).await;
    assert!(kinds(&l).is_empty(), "{:?}", l["match"]["findings"]);
    let b = &l["match"]["material_basis"][0];
    assert_eq!(b["substitution_decision"], sub_id.as_str());
    assert_eq!(b["verification"], "unverified");
    let (st, body) = accept(&s, "approved").await;
    assert_eq!(st, StatusCode::OK, "{body:?}");
    let acc = body["decision"]["id"].as_str().unwrap().to_owned();
    let tr = s.c.get(&format!("/api/v1/decisions/{acc}/trace"), &s.tok).await.body;
    assert!(
        tr["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["upstream_decision"] == sub_id.as_str()),
        "acceptance depends on the substitution decision: {tr:?}"
    );
    let tr = s.c.get(&format!("/api/v1/decisions/{sub_id}/trace"), &s.tok).await.body;
    assert!(
        tr["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["grade_equivalence"] == eq1.as_str()),
        "{tr:?}"
    );

    // 5. Names equal but values fail the required grade's tenant limit (labelled unverified).
    let r =
        s.c.post(
            "/api/v1/grades/limits",
            Some(&s.tok),
            json!({ "change": "create", "standard": "JIS", "grade": "XQ202", "property": "elongation", "min": "25", "unit": "%" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["verification"], "unverified");
    let lim = r.body["id"].as_str().unwrap().to_owned();
    let r =
        s.c.post(
            &format!("/api/v1/grades/limits/{lim}/approve"),
            Some(&s.rev2),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let l = line(&s).await;
    assert_eq!(kinds(&l), ["grade_property_out_of_limits"]);
    let f = finding(&l, "grade_property_out_of_limits");
    assert_eq!(f["severity"], "major");
    assert_eq!(f["explanation"]["detail"]["limit"]["verification"], "unverified");
    assert_eq!(f["explanation"]["detail"]["value"], "18");

    // 6. The tenant changes its statement: every decision resting on the old version is flagged.
    let r =
        s.c.post(
            "/api/v1/grades/equivalences",
            Some(&s.tok),
            json!({ "change": "edit", "entry_id": entry, "a_standard": "JIS", "a_grade": "XQ202", "b_standard": "IS",
                    "b_grade": "K9 220", "equivalence": "not_equivalent", "note": "re-checked" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let eq2 = r.body["id"].as_str().unwrap().to_owned();
    let r =
        s.c.post(
            &format!("/api/v1/grades/equivalences/{eq2}/approve"),
            Some(&s.rev2),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert!(r.body["decisions_flagged"].as_u64().unwrap() >= 2, "{:?}", r.body);
    assert_eq!(decision_status(&s, &sub_id).await, "potentially_affected");
    assert_eq!(decision_status(&s, &acc).await, "potentially_affected");
    let l = line(&s).await;
    let f = finding(&l, "certificate_material_mismatch");
    assert_eq!(f["explanation"]["msg"]["code"], "match.material_not_equivalent");
    assert_eq!(f["explanation"]["detail"]["equivalence"]["id"], eq2.as_str());
    // A reconfirmation cannot carry a superseded equivalence over.
    let r =
        s.c.post(
            &format!("/api/v1/decisions/{sub_id}/reconfirm"),
            Some(&s.tok),
            json!({ "comment": "still fine" }),
        )
        .await;
    assert_eq!(
        (r.status, r.body["error"].as_str()),
        (StatusCode::CONFLICT, Some("grade_equivalence_changed"))
    );
    // History is kept: v1 superseded, v2 approved.
    let all = s.c.get("/api/v1/grades/equivalences", &s.viewer).await.body;
    let st: Vec<(i64, String)> = all["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["entry_id"] == entry.as_str())
        .map(|e| (e["version"].as_i64().unwrap(), e["status"].as_str().unwrap().to_owned()))
        .collect();
    assert_eq!(st, [(2, "approved".to_owned()), (1, "superseded".to_owned())]);
}

#[tokio::test]
async fn import_is_all_or_nothing_and_the_tables_are_append_only() {
    let s = setup().await;
    let r =
        s.c.post(
            "/api/v1/grades/equivalences/import",
            Some(&s.tok),
            json!({ "csv": EXAMPLE }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["proposed"].as_array().unwrap().len(), 3);
    let all = s.c.get("/api/v1/grades/equivalences", &s.viewer).await.body;
    let rows = all["entries"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter()
            .all(|e| e["status"] == "proposed" && e["verification"] == "unverified" && e["source_checked"] == false)
    );
    // One bad row refuses the whole file.
    let bad = format!("{}\nJIS,XQ9,ASTM,ZR9,maybe,,\n", EXAMPLE.trim_end());
    let r =
        s.c.post(
            "/api/v1/grades/equivalences/import",
            Some(&s.tok),
            json!({ "csv": bad }),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{:?}", r.body);
    assert_eq!(r.body["rows"][0]["code"], "invalid_equivalence");
    assert_eq!(
        s.c.get("/api/v1/grades/equivalences", &s.viewer).await.body["entries"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    // Approving one makes its pair effective: importing it again is refused (an edit is the way to change it).
    let id = rows[0]["id"].as_str().unwrap().to_owned();
    let r =
        s.c.post(
            &format!("/api/v1/grades/equivalences/{id}/approve"),
            Some(&s.rev2),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let r =
        s.c.post(
            "/api/v1/grades/equivalences/import",
            Some(&s.tok),
            json!({ "csv": EXAMPLE }),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.body["existing_pairs"].as_array().unwrap().len(), 1);
    // A rejection needs a reason; a decided row cannot be decided again.
    let other = rows[1]["id"].as_str().unwrap().to_owned();
    let r =
        s.c.post(
            &format!("/api/v1/grades/equivalences/{other}/reject"),
            Some(&s.rev2),
            json!({}),
        )
        .await;
    assert_eq!(r.body["error"], "comment_required");
    let r =
        s.c.post(
            &format!("/api/v1/grades/equivalences/{other}/reject"),
            Some(&s.rev2),
            json!({ "comment": "no source" }),
        )
        .await;
    assert_eq!(r.body["status"], "rejected");
    let r =
        s.c.post(
            &format!("/api/v1/grades/equivalences/{other}/approve"),
            Some(&s.rev2),
            json!({}),
        )
        .await;
    assert_eq!(r.body["error"], "not_proposed");

    // The database refuses edits of content, deletes, and self-approval, whoever writes.
    let mut tx = db::tenant_tx(&s.c.owner, s.t).await.unwrap();
    let e = sqlx::query(&format!(
        "UPDATE grade_equivalences SET a_grade = 'XQ999' WHERE id = '{id}'"
    ))
    .execute(&mut *tx)
    .await;
    assert!(e.is_err(), "content is immutable");
    drop(tx);
    let mut tx = db::tenant_tx(&s.c.owner, s.t).await.unwrap();
    assert!(
        sqlx::query(&format!("DELETE FROM grade_equivalences WHERE id = '{id}'"))
            .execute(&mut *tx)
            .await
            .is_err()
    );
    drop(tx);
    let third = rows[2]["id"].as_str().unwrap().to_owned();
    let mut tx = db::tenant_tx(&s.c.owner, s.t).await.unwrap();
    let e = sqlx::query(&format!(
        "UPDATE grade_equivalences SET status = 'approved', decided_by = proposed_by, decided_at = now() WHERE id = '{third}'"
    ))
    .execute(&mut *tx)
    .await;
    assert!(e.is_err(), "four-eyes is enforced by the database too");
}

#![allow(clippy::unwrap_used)]
//! Certificate integrity signals and revision drift radar through the API: real extraction of CSV/PDF documents,
//! detection, idempotent re-runs, review-only workflow (four-eyes, comment required, never auto-reject), audit of
//! who viewed a signal, selective invalidation on a confirmed drift, settings bounds, tenant isolation, FORCE RLS.
mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};

/// 12 lots of one supplier/part. Lot L1010 repeats every value of L1003 (whole-table copy); L1006 is dated after
/// its shipment; L1008's composition adds up to more than 100 %.
fn certificates_csv() -> Vec<u8> {
    let mut s = String::from(
        "Lot No,Heat No,Part Number,Supplier,Measured length (mm),Measured width (mm),Measured height (mm),Issue date,Ship date,C (%),Si (%),Mn (%)\n",
    );
    for i in 0..12u32 {
        let src = if i == 10 { 3 } else { i };
        let f = f64::from(src);
        let len = 120.0 + ((f * 0.037) % 0.3) - 0.15;
        let wid = 40.0 + ((f * 0.053) % 0.3) - 0.15;
        let hei = 15.0 + ((f * 0.071) % 0.3) - 0.15;
        let c = 0.13 + f * 0.003;
        let si = 0.25 + f * 0.007;
        let mn = if i == 8 { 120.0 } else { 1.1 + f * 0.011 };
        let day = 1 + i * 2;
        let (issued, shipped) = if i == 6 { (day + 12, day) } else { (day, day + 1) };
        s.push_str(&format!(
            "L{},H{},BRK-300,ACME Steel,{len:.3},{wid:.3},{hei:.3},2026-03-{issued:02},2026-03-{shipped:02},{c:.3},{si:.3},{mn:.3}\n",
            1000 + i,
            5000 + i * 7
        ));
    }
    s.into_bytes()
}

fn spec_pdf() -> Vec<u8> {
    make_pdf(&[
        "Part Number: BRK-300",
        "Revision: A",
        "Length (mm): 120",
        "Length tolerance: +/-0.2",
        "Width (mm): 40",
        "Width tolerance: +/-0.2",
    ])
}

fn drawing_pdf(rev: &str, length: &str) -> Vec<u8> {
    make_pdf(&[
        "Drawing No: D-901",
        "Part Number: BRK-400",
        &format!("Rev: {rev}"),
        &format!("Length (mm): {length}"),
        "Length tolerance: +/-0.1",
        "Width (mm): 30",
        "Material: SS400",
    ])
}

/// Count in the tenant's scope (the owner role is subject to FORCE RLS too).
async fn count(c: &Ctx, t: uuid::Uuid, sql: &str, binds: &[&str]) -> i64 {
    let mut tx = kizuna_api::db::tenant_tx(&c.owner, t).await.unwrap();
    let mut q = sqlx::query_scalar::<_, i64>(sql);
    for b in binds {
        q = q.bind(b.to_string());
    }
    let n = q.fetch_one(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    n
}

fn kinds(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["kind"].as_str().unwrap().to_owned())
        .collect();
    k.sort();
    k
}

#[tokio::test]
async fn certificate_signals_end_to_end() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, uploader) = c.user(t, "reviewer").await;
    let (_, other) = c.user(t, "reviewer").await;
    let (_, auditor) = c.user(t, "auditor").await;
    let up = c.mfa_token(&slug, &uploader).await;
    let rev2 = c.mfa_token(&slug, &other).await;
    let aud = c.token(&slug, &auditor).await;

    c.ingest(&up, "spec_brk300.pdf", &spec_pdf(), "spec_document").await;
    let csv = c
        .ingest(&up, "certificates_acme.csv", &certificates_csv(), "inspection_report")
        .await;

    assert_eq!(
        c.post("/api/v1/integrity/run", Some(&aud), json!({})).await.status,
        StatusCode::FORBIDDEN,
        "auditors read, they do not run detection"
    );
    let run = c.post("/api/v1/integrity/run", Some(&up), json!({})).await;
    assert_eq!(run.status, StatusCode::OK, "{:?}", run.body);
    let list = c.get("/api/v1/integrity/signals?status=open", &aud).await;
    let k = kinds(&list.body);
    for want in ["chemistry_sum", "date_order", "table_copy"] {
        assert!(k.iter().any(|x| x == want), "missing {want}: {k:?} {:?}", list.body);
    }
    assert!(
        !k.iter().any(|x| x == "identical_values" || x == "lot_sequence"),
        "{k:?}"
    );
    let rerun = c.post("/api/v1/integrity/run", Some(&up), json!({})).await;
    assert_eq!(rerun.body["signals_new"], 0, "idempotent: {:?}", rerun.body);
    assert_eq!(rerun.body["marked_obsolete"], 0);

    // Detail: explanation, statistic, evidence with source locations, documents with hashes, review required.
    let copy = list
        .body
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "table_copy")
        .unwrap();
    let id = copy["id"].as_str().unwrap();
    let d = c.get(&format!("/api/v1/integrity/signals/{id}"), &aud).await;
    assert_eq!(d.status, StatusCode::OK);
    assert_eq!(d.body["requires_human_review"], true);
    assert_eq!(d.body["never_auto_reject"], true);
    let why = d.body["signal"]["explanation"]["why"].as_str().unwrap();
    assert!(why.contains("L1003") && why.contains("L1010"), "{why}");
    assert!(why.contains("not a finding of wrongdoing"));
    assert!(
        d.body["signal"]["explanation"]["why_ja"]
            .as_str()
            .unwrap()
            .contains("不正の判断ではありません")
    );
    assert_eq!(d.body["signal"]["statistic"]["identical"], 6);
    assert!(d.body["signal"]["evidence"][0]["location"].is_object());
    assert_eq!(d.body["documents"][0]["artifact_id"], csv);
    assert_eq!(d.body["documents"][0]["sha256"].as_str().unwrap().len(), 64);
    assert!(d.body["limitations"].as_str().unwrap().contains("not a probability"));
    let viewed = count(
        &c,
        t,
        "SELECT count(*) FROM audit_events WHERE action = 'integrity_signal.viewed' AND target_id = $1",
        &[id],
    )
    .await;
    assert_eq!(viewed, 1, "who viewed a signal is audited");

    // Review: four-eyes, comment required, decisions limited; nothing is rejected automatically.
    let path = format!("/api/v1/integrity/signals/{id}/review");
    let own = c
        .post(&path, Some(&up), json!({"decision": "explained", "comment": "x"}))
        .await;
    assert_eq!(
        own.status,
        StatusCode::FORBIDDEN,
        "uploader may not review: {:?}",
        own.body
    );
    let no_comment = c.post(&path, Some(&rev2), json!({"decision": "follow_up"})).await;
    assert_eq!(no_comment.body["error"], "comment_required");
    let bad = c
        .post(&path, Some(&rev2), json!({"decision": "reject", "comment": "x"}))
        .await;
    assert_eq!(bad.body["error"], "invalid_decision");
    let ok = c
        .post(
            &path,
            Some(&rev2),
            json!({"decision": "follow_up", "comment": "Ask ACME for the L1010 test record"}),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.body);
    assert_eq!(ok.body["status"], "follow_up");
    let again = c
        .post(&path, Some(&rev2), json!({"decision": "explained", "comment": "x"}))
        .await;
    assert_eq!(again.body["error"], "signal_not_open");
    let rejected = count(
        &c,
        t,
        "SELECT count(*) FROM claims WHERE review_status = 'rejected'",
        &[],
    )
    .await;
    let total = count(&c, t, "SELECT count(*) FROM claims", &[]).await;
    assert!(total > 50, "{total}");
    assert_eq!(rejected, 0, "signals never reject evidence");
    // A reviewed signal keeps its status on re-runs.
    c.post("/api/v1/integrity/run", Some(&up), json!({})).await;
    let d = c.get(&format!("/api/v1/integrity/signals/{id}"), &aud).await;
    assert_eq!(d.body["signal"]["status"], "follow_up");
    assert_eq!(d.body["reviews"].as_array().unwrap().len(), 1);

    // Tenant isolation.
    let (_, slug2, _, admin2) = c.admin().await;
    let _ = slug2;
    assert_eq!(c.get("/api/v1/integrity/signals", &admin2).await.body, json!([]));
    assert_eq!(
        c.get(&format!("/api/v1/integrity/signals/{id}"), &admin2).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        c.post(&path, Some(&admin2), json!({"decision": "explained", "comment": "x"}))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn revision_drift_end_to_end_feeds_selective_invalidation() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, uploader) = c.user(t, "reviewer").await;
    let (_, other) = c.user(t, "reviewer").await;
    let up = c.mfa_token(&slug, &uploader).await;
    let rev2 = c.mfa_token(&slug, &other).await;

    let v1 = c.ingest(&up, "D-901_B.pdf", &drawing_pdf("B", "120"), "drawing").await;
    // A release decision that relies on the drawing's length at revision B.
    let claims = c.get(&format!("/api/v1/claims?artifact_id={v1}"), &up).await;
    let length = claims
        .body
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["predicate"] == "mfg.dimension.length")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let dec = c
        .post("/api/v1/decisions", Some(&up), json!({
            "kind": "supplier_part_release", "title": "Release BRK-400 rev B", "subject_type": "part", "subject_key": "BRK-400",
            "outcome": "approved", "rationale": "Drawing D-901 rev B checked", "evidence": [{"claim_id": length}], "depends_on": []
        }))
        .await;
    assert_eq!(dec.status, StatusCode::OK, "{:?}", dec.body);
    let dec_id = dec.body["id"].as_str().unwrap().to_owned();

    // Same revision, different length (silent change); then revision C with identical content.
    let v2 = c
        .ingest(&up, "D-901_B_resent.pdf", &drawing_pdf("B", "121"), "drawing")
        .await;
    let v3 = c.ingest(&up, "D-901_C.pdf", &drawing_pdf("C", "121"), "drawing").await;
    // A re-scan of the same content under revision C is not a finding.
    c.ingest(&up, "D-901_C_scan.pdf", &drawing_pdf("C", "121.0"), "drawing")
        .await;

    let run = c.post("/api/v1/integrity/run", Some(&up), json!({})).await;
    assert_eq!(run.status, StatusCode::OK, "{:?}", run.body);
    assert_eq!(run.body["drift_total"], 2, "{:?}", run.body);
    let list = c.get("/api/v1/integrity/drift?status=open", &up).await;
    assert_eq!(kinds(&list.body), vec!["revision_without_change", "silent_change"]);
    let silent = list
        .body
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["kind"] == "silent_change")
        .unwrap();
    assert_eq!(silent["drawing_key"], "D-901");
    assert_eq!(silent["earlier_artifact"], v1);
    assert_eq!(silent["later_artifact"], v2);
    assert_eq!(silent["strength"], "strong");
    let change = &silent["changes"][0];
    assert_eq!(change["predicate"], "mfg.dimension.length");
    assert_eq!(change["before"][0]["value"], "120 mm");
    assert_eq!(change["after"][0]["value"], "121 mm");
    assert!(change["after"][0]["location"].is_object());
    let without = list
        .body
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["kind"] == "revision_without_change")
        .unwrap();
    assert_eq!(
        (without["earlier_artifact"].as_str(), without["later_artifact"].as_str()),
        (Some(v2.as_str()), Some(v3.as_str()))
    );

    let sid = silent["id"].as_str().unwrap();
    let d = c.get(&format!("/api/v1/integrity/drift/{sid}"), &up).await;
    assert_eq!(d.body["requires_human_review"], true);
    let affected: Vec<&str> = d.body["potentially_affected_decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap())
        .collect();
    assert_eq!(affected, vec![dec_id.as_str()]);
    assert_eq!(d.body["documents"].as_array().unwrap().len(), 2);

    // Confirming the drift runs selective invalidation for the changed claim of the later version.
    let path = format!("/api/v1/integrity/drift/{sid}/review");
    assert_eq!(
        c.post(&path, Some(&up), json!({"decision": "confirm_drift", "comment": "x"}))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let r = c
        .post(
            &path,
            Some(&rev2),
            json!({"decision": "confirm_drift", "comment": "Supplier changed length without a new revision"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["drift"]["status"], "confirmed");
    let st = c.get(&format!("/api/v1/decisions/{dec_id}"), &up).await;
    assert_eq!(st.body["decision"]["status"], "potentially_affected");
    let later_length: String = change["after"][0]["claim"].as_str().unwrap().to_owned();
    let events = count(
        &c,
        t,
        "SELECT count(*) FROM invalidation_events WHERE decision_id = $1::uuid AND cause_claim = $2::uuid",
        &[&dec_id, &later_length],
    )
    .await;
    assert!(events >= 1, "invalidation recorded with the later claim as cause");

    let wid = without["id"].as_str().unwrap();
    let r = c
        .post(
            &format!("/api/v1/integrity/drift/{wid}/review"),
            Some(&rev2),
            json!({"decision": "accept_change", "comment": "Rev C changed a view only"}),
        )
        .await;
    assert_eq!(r.body["drift"]["status"], "accepted_change");
    let rerun = c.post("/api/v1/integrity/run", Some(&up), json!({})).await;
    assert_eq!(rerun.body["drift_new"], 0);
}

#[tokio::test]
async fn settings_are_bounded_versioned_and_audited() {
    let c = ctx().await;
    let (t, _slug, _, admin) = c.admin().await;
    let g = c.get("/api/v1/integrity/settings", &admin).await;
    assert_eq!(g.status, StatusCode::OK);
    assert_eq!(g.body["version"], 0);
    assert_eq!(g.body["bounds"]["at_limit_pct"], json!([10, 90]));
    let mut s = g.body["defaults"].clone();
    s["at_limit_pct"] = json!(95);
    let bad = c
        .call(
            axum::http::Method::PUT,
            "/api/v1/integrity/settings",
            Some(&admin),
            Some(json!({"expected_version": 0, "settings": s})),
        )
        .await;
    assert_eq!(bad.body["error"], "invalid_thresholds");
    let unknown = c
        .call(
            axum::http::Method::PUT,
            "/api/v1/integrity/settings",
            Some(&admin),
            Some(json!({"expected_version": 0, "settings": {"disable": true}})),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
    s["at_limit_pct"] = json!(60);
    let ok = c
        .call(
            axum::http::Method::PUT,
            "/api/v1/integrity/settings",
            Some(&admin),
            Some(json!({"expected_version": 0, "settings": s})),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.body);
    assert_eq!(ok.body["version"], 1);
    let stale = c
        .call(
            axum::http::Method::PUT,
            "/api/v1/integrity/settings",
            Some(&admin),
            Some(json!({"expected_version": 0, "settings": s})),
        )
        .await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(
        c.get("/api/v1/integrity/settings", &admin).await.body["effective"]["at_limit_pct"],
        60
    );
    // Less sensitive thresholds are a security-relevant settings change.
    let weakening: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events WHERE tenant_id = $1 AND kind = 'admin.settings_changed' AND (details->>'weakening')::boolean",
    )
    .bind(t)
    .fetch_one(&c.owner)
    .await
    .unwrap();
    assert_eq!(weakening, 1);
    // Reviewers cannot change thresholds.
    let (_, e) = c.user(t, "reviewer").await;
    let slug: String = sqlx::query_scalar("SELECT slug FROM tenants WHERE id = $1")
        .bind(t)
        .fetch_one(&c.owner)
        .await
        .unwrap();
    let r = c.mfa_token(&slug, &e).await;
    assert_eq!(
        c.call(
            axum::http::Method::PUT,
            "/api/v1/integrity/settings",
            Some(&r),
            Some(json!({"expected_version": 1, "settings": s}))
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn tables_force_row_level_security_and_findings_are_append_only() {
    let c = ctx().await;
    for table in [
        "integrity_signal_settings",
        "integrity_signals",
        "revision_drift_findings",
        "integrity_reviews",
    ] {
        let (rls, force): (bool, bool) =
            sqlx::query_as("SELECT relrowsecurity, relforcerowsecurity FROM pg_class WHERE relname = $1")
                .bind(table)
                .fetch_one(&c.owner)
                .await
                .unwrap();
        assert!(rls && force, "{table}");
    }
    // The runtime role cannot delete findings or rewrite reviews, even in its own tenant.
    let (t, _) = c.tenant().await;
    let mut tx = kizuna_api::db::tenant_tx(&c.st.pool, t).await.unwrap();
    let del = sqlx::query("DELETE FROM integrity_signals").execute(&mut *tx).await;
    assert!(del.is_err(), "no DELETE grant");
    drop(tx);
    let mut tx = kizuna_api::db::tenant_tx(&c.st.pool, t).await.unwrap();
    let upd = sqlx::query("UPDATE integrity_reviews SET comment = 'x'")
        .execute(&mut *tx)
        .await;
    assert!(upd.is_err(), "reviews are append-only");
}

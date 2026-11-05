#![allow(clippy::unwrap_used)]
//! Evidence pack export: round trip through the offline verifier, hash-only redaction, checkpoint reuse,
//! permissions and validation.
mod common;

use std::io::{Cursor, Read};

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use common::*;
use http_body_util::BodyExt;
use kizuna_verify_pack::{Lang, VerifyOptions, key_id, render, verify};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

/// The test configuration's signing key is 32 bytes of 0x07.
fn test_key_id() -> String {
    key_id(&audit_engine::checkpoint::public_key(&[7u8; 32]))
}

async fn export(c: &Ctx, tok: &str, decision: &str, body: Value) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/decisions/{decision}/evidence-pack"))
        .header("authorization", format!("Bearer {tok}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(c.ip));
    let res = c.app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    if status == StatusCode::OK {
        assert_eq!(res.headers()["content-type"], "application/zip");
        assert!(res.headers()["content-disposition"].to_str().unwrap().contains(".zip"));
    }
    (status, res.into_body().collect().await.unwrap().to_bytes().to_vec())
}

fn names(zip: &[u8]) -> Vec<String> {
    let za = zip::ZipArchive::new(Cursor::new(zip)).unwrap();
    za.file_names().map(|n| n.unwrap().into_owned()).collect()
}

fn manifest(zip: &[u8]) -> Value {
    let mut za = zip::ZipArchive::new(Cursor::new(zip)).unwrap();
    let mut s = String::new();
    za.by_name("manifest.json").unwrap().read_to_string(&mut s).unwrap();
    serde_json::from_str(&s).unwrap()
}

fn code(b: &[u8]) -> Value {
    serde_json::from_slice::<Value>(b).unwrap()["error"].clone()
}

struct Setup {
    c: Ctx,
    t: Uuid,
    slug: String,
    admin: String,
    decision: String,
    quote: String,
}

async fn upload(c: &Ctx, tok: &str, name: &str, lines: &[&str]) -> String {
    let r = c.upload(tok, name, &make_pdf(lines), None).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["artifact"]["status"], "accepted");
    r.body["artifact"]["id"].as_str().unwrap().to_owned()
}

async fn claim(c: &Ctx, tok: &str, body: Value) -> String {
    let r = c.post("/api/v1/claims", Some(tok), body).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body["id"].as_str().unwrap().to_owned()
}

async fn setup() -> Setup {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let cert = upload(
        &c,
        &admin,
        "mill-cert.pdf",
        &["Mill certificate BRK-100", "Tensile 520 MPa"],
    )
    .await;
    let quote = upload(&c, &admin, "見積書.pdf", &["Quote BRK-100", "Unit price 1200 JPY"]).await;
    let a = claim(&c, &admin, json!({"subject_type":"part","subject_key":"BRK-100","predicate":"mfg.dimension.length","value_num":"120","value_unit":"mm","revision":"C","artifact_id": cert, "source_location": {"page": 1, "line": 2}})).await;
    let b = claim(&c, &admin, json!({"subject_type":"part","subject_key":"BRK-100","predicate":"mfg.approval.status","value_text":"approved","revision":"C","artifact_id": quote, "source_location": {"page": 1}})).await;
    let r = c
        .post("/api/v1/decisions", Some(&admin), json!({
            "kind": "supplier_part_release", "title": "Release BRK-100", "subject_type": "part", "subject_key": "BRK-100",
            "outcome": "approved", "rationale": "Certificate and quote checked",
            "evidence": [{"claim_id": a}, {"claim_id": b}], "depends_on": []
        }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let decision = r.body["id"].as_str().unwrap().to_owned();
    Setup {
        c,
        t,
        slug,
        admin,
        decision,
        quote,
    }
}

async fn checkpoints(s: &Setup) -> i64 {
    let mut tx = kizuna_api::db::tenant_tx(&s.c.owner, s.t).await.unwrap();
    let n = sqlx::query_scalar("SELECT count(*) FROM audit_checkpoints WHERE tenant_id = $1")
        .bind(s.t)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    n
}

#[tokio::test]
async fn export_round_trips_through_the_offline_verifier() {
    let s = setup().await;
    let (st, zip) = export(&s.c, &s.admin, &s.decision, json!({})).await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&zip));
    let opts = VerifyOptions {
        trusted_key: Some(test_key_id()),
    };
    let r = verify(Cursor::new(&zip), &opts);
    assert!(r.ok, "{}", render(&r, Lang::En));
    let text = render(&r, Lang::Both);
    assert!(text.contains("byte-identical"));
    assert!(text.contains("the key you trust"));
    assert!(text.contains("That any document's content is true"));
    let n = names(&zip);
    for f in [
        "manifest.json",
        "manifest.sig.json",
        "decision/derivation.json",
        "decision/record.json",
        "decision/trace.json",
        "decision/report.json",
        "README.txt",
    ] {
        assert!(n.iter().any(|x| x == f), "{f} missing in {n:?}");
    }
    assert_eq!(n.iter().filter(|x| x.starts_with("artifacts/")).count(), 2);
    let m = manifest(&zip);
    assert_eq!(m["subject"]["id"], s.decision);
    assert_eq!(m["checkpoint"]["message_version"], "KZCP3");
    let actions: Vec<&str> = m["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert!(
        actions.contains(&"decision.recorded") && actions.iter().filter(|a| **a == "artifact.uploaded").count() == 2
    );
    // The derivation in the pack carries the claims' source locations.
    let mut za = zip::ZipArchive::new(Cursor::new(&zip)).unwrap();
    let mut d = String::new();
    za.by_name("decision/derivation.json")
        .unwrap()
        .read_to_string(&mut d)
        .unwrap();
    assert!(d.contains("\"source_location\":{\"line\":2,\"page\":1}"), "{d}");

    // The export is audited; the KZCP3 checkpoint it created still verifies in-app and as an external copy.
    let ev = s.c.get("/api/v1/audit/events?limit=500", &s.admin).await;
    assert!(
        ev.body
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "decision.evidence_pack_exported")
    );
    let v = s.c.get("/api/v1/audit/checkpoints/verify", &s.admin).await;
    assert_eq!(v.body["intact"], true, "{:?}", v.body);
    let cp = &m["checkpoint"];
    let ext =
        s.c.post(
            "/api/v1/audit/checkpoints/verify-external",
            Some(&s.admin),
            json!({
                "tenant_id": s.t, "seq": cp["seq"], "head_hash": cp["head_hash"], "signed_at_us": cp["signed_at_us"],
                "key_id": cp["key_id"], "public_key": cp["public_key"], "signature": cp["signature"],
                "prev_signature_sha256": cp["prev_signature_sha256"], "merkle_root": cp["merkle_root"],
            }),
        )
        .await;
    assert_eq!(ext.status, StatusCode::OK, "{:?}", ext.body);
    assert_eq!(ext.body["intact"], true);

    // A second export reuses the covering checkpoint; hash-only leaves the file out but keeps its proof.
    let before = checkpoints(&s).await;
    assert_eq!(before, 1, "the first export created exactly one checkpoint");
    let (st, zip2) = export(&s.c, &s.admin, &s.decision, json!({ "hash_only": [s.quote] })).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(checkpoints(&s).await, before);
    assert!(!names(&zip2).iter().any(|x| x.contains(&s.quote)));
    let r = verify(Cursor::new(&zip2), &opts);
    assert!(r.ok, "{}", render(&r, Lang::En));
    let m2 = manifest(&zip2);
    let q = m2["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == s.quote)
        .unwrap();
    assert_eq!(q["included"], false);
    assert_eq!(q["excluded_reason"], "requested");
    assert!(render(&r, Lang::En).contains("hash only"));
}

#[tokio::test]
async fn export_permissions_and_validation() {
    let s = setup().await;
    let (_, viewer_email) = s.c.user(s.t, "viewer").await;
    let viewer = s.c.token(&s.slug, &viewer_email).await;
    // A viewer may export but not create a checkpoint: without a covering KZCP3 checkpoint the export is refused.
    let (st, b) = export(&s.c, &viewer, &s.decision, json!({})).await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(code(&b), "checkpoint_required");
    let cp = s.c.post("/api/v1/audit/checkpoints", Some(&s.admin), json!({})).await;
    assert_eq!(cp.status, StatusCode::OK, "{:?}", cp.body);
    assert_eq!(cp.body["merkle_root"].as_str().unwrap().len(), 64);
    let (st, zip) = export(&s.c, &viewer, &s.decision, json!({})).await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&zip));
    assert!(verify(Cursor::new(&zip), &VerifyOptions::default()).ok);
    // Earlier exports are audited but are not evidence: a repeat export still fits the same checkpoint.
    let (st, _) = export(&s.c, &viewer, &s.decision, json!({})).await;
    assert_eq!(st, StatusCode::OK);

    // Validation.
    let (st, b) = export(&s.c, &s.admin, &s.decision, json!({ "hash_only": [Uuid::now_v7()] })).await;
    assert_eq!((st, code(&b)), (StatusCode::BAD_REQUEST, json!("unknown_artifact")));
    let (st, _) = export(&s.c, &s.admin, &s.decision, json!({ "everything": true })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = export(&s.c, &s.admin, &Uuid::now_v7().to_string(), json!({})).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // Unauthenticated and cross-tenant.
    let r =
        s.c.post(
            &format!("/api/v1/decisions/{}/evidence-pack", s.decision),
            None,
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let (_, _, _, other) = s.c.admin().await;
    let (st, _) = export(&s.c, &other, &s.decision, json!({})).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

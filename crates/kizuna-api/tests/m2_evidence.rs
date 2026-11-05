#![allow(clippy::unwrap_used)]
mod common;

use axum::http::{Method, StatusCode};
use common::*;
use kizuna_api::db;
use serde_json::json;

#[tokio::test]
async fn upload_dedup_capture_and_download() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "analyst").await;
    let tok = c.token(&slug, &email).await;
    let r = c.upload(&tok, "../../spec.pdf", CLEAN_PDF, Some("spec_document")).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let a = &r.body["artifact"];
    assert_eq!(a["status"], "accepted");
    assert_eq!(a["media_type"], "application/pdf");
    assert_eq!(a["display_name"], "spec.pdf");
    assert_eq!(a["sha256"], hex::encode(kizuna_api::crypto::sha256(CLEAN_PDF)));
    let id = a["id"].as_str().unwrap().to_owned();

    let r2 = c.upload(&tok, "copy.pdf", CLEAN_PDF, None).await;
    assert_eq!(
        (r2.body["duplicate"].as_bool(), r2.body["artifact"]["id"].as_str()),
        (Some(true), Some(id.as_str()))
    );
    let d = c.get(&format!("/api/v1/artifacts/{id}"), &tok).await;
    assert_eq!(
        d.body["captures"].as_array().unwrap().len(),
        2,
        "every capture recorded"
    );

    let r = c.get(&format!("/api/v1/artifacts/{id}/content"), &tok).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers["content-type"], "application/pdf");
    assert!(
        r.headers["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );
}

#[tokio::test]
async fn rejects_disallowed_types_and_oversize() {
    let c = ctx_with(|cfg| cfg.max_upload_bytes = 4096).await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "analyst").await;
    let tok = c.token(&slug, &email).await;
    let r = c.upload(&tok, "x.pdf", b"\x7fELF\x02\x01\x01\0payload", None).await;
    assert_eq!(r.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert!(
        c.security_kinds(t)
            .await
            .contains(&"ingest.upload_rejected".to_string())
    );
    let r = c.upload(&tok, "big.csv", &vec![b'a'; 5000], None).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    let list = c.get("/api/v1/artifacts", &tok).await;
    assert_eq!(list.body.as_array().unwrap().len(), 0, "nothing stored");
    let r = c.upload(&tok, "x.pdf", CLEAN_PDF, Some("../../etc")).await;
    assert_eq!(r.body["error"], "invalid_kind");
}

#[tokio::test]
async fn active_pdf_is_quarantined_until_privileged_release() {
    let c = ctx().await;
    let (t, slug, _, admin) = c.admin().await;
    let (_, email) = c.user(t, "reviewer").await;
    let rev = c.token(&slug, &email).await;
    let pdf = b"%PDF-1.7\n1 0 obj << /OpenAction << /S /JavaScript /JS (app.alert(1)) >> >> endobj\n%%EOF\n";
    let r = c.upload(&rev, "evil.pdf", pdf, None).await;
    assert_eq!(r.body["artifact"]["status"], "quarantined");
    let id = r.body["artifact"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        c.get(&format!("/api/v1/artifacts/{id}/content"), &rev).await.body["error"],
        "artifact_not_released"
    );
    assert!(
        c.security_kinds(t)
            .await
            .contains(&"ingest.artifact_quarantined".to_string())
    );
    assert_eq!(
        c.post(&format!("/api/v1/artifacts/{id}/release"), Some(&rev), json!({}))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let r = c
        .post(&format!("/api/v1/artifacts/{id}/release"), Some(&admin), json!({}))
        .await;
    assert_eq!(r.body["status"], "accepted", "{:?}", r.body);
}

#[tokio::test]
async fn stored_artifact_tampering_is_detected() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "analyst").await;
    let tok = c.token(&slug, &email).await;
    let r = c.upload(&tok, "s.csv", b"part,rev\nBRK-1,C\n", None).await;
    let id = r.body["artifact"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        c.post(&format!("/api/v1/artifacts/{id}/verify"), Some(&tok), json!({}))
            .await
            .body["intact"],
        true
    );
    let key = kizuna_api::storage::ObjectKey {
        tenant: t,
        sha256: kizuna_api::crypto::sha256(b"part,rev\nBRK-1,C\n"),
    };
    let path = kizuna_api::storage::FsStore::new(c.st.cfg.storage_dir.clone()).path(&key);
    std::fs::write(&path, b"part,rev\nBRK-1,D\n").unwrap();
    assert_eq!(
        c.post(&format!("/api/v1/artifacts/{id}/verify"), Some(&tok), json!({}))
            .await
            .body["intact"],
        false
    );
    assert_eq!(
        c.get(&format!("/api/v1/artifacts/{id}/content"), &tok).await.body["error"],
        "artifact_not_released",
        "M5: integrity failure auto-quarantines the artifact"
    );
    assert!(
        c.security_kinds(t)
            .await
            .contains(&"integrity.artifact_mismatch".to_string())
    );
}

#[tokio::test]
async fn artifacts_and_claims_are_tenant_isolated() {
    let c = ctx().await;
    let (ta, sa) = c.tenant().await;
    let (_, ea) = c.user(ta, "analyst").await;
    let a = c.token(&sa, &ea).await;
    let (tb, sb) = c.tenant().await;
    let (_, eb) = c.user(tb, "analyst").await;
    let b = c.token(&sb, &eb).await;
    let id = c.upload(&a, "s.pdf", CLEAN_PDF, None).await.body["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        c.get(&format!("/api/v1/artifacts/{id}"), &b).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        c.get(&format!("/api/v1/artifacts/{id}/content"), &b).await.status,
        StatusCode::NOT_FOUND
    );
    let r = c
        .post("/api/v1/claims", Some(&b), json!({"subject_type":"part","subject_key":"X","predicate":"mfg.material","value_text":"steel","artifact_id": id}))
        .await;
    assert_eq!(r.body["error"], "unknown_artifact");
    // Same bytes in tenant B are a separate artifact (no cross-tenant dedup signal).
    let rb = c.upload(&b, "s.pdf", CLEAN_PDF, None).await;
    assert_eq!(rb.body["duplicate"], false);
}

#[tokio::test]
async fn claims_supersession_and_bitemporal_queries() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "reviewer").await;
    let tok = c.token(&slug, &email).await;
    let art = c.upload(&tok, "spec.pdf", CLEAN_PDF, None).await.body["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = c
        .post(
            "/api/v1/claims",
            Some(&tok),
            json!({
                "subject_type": "part", "subject_key": "BRK-100", "predicate": "mfg.dimension.length",
                "value_num": "120.50", "value_unit": "mm", "revision": "B", "artifact_id": art,
                "source_location": {"page": 2, "table": 1, "cell": "C7"}, "valid_from": "2026-01-01T00:00:00Z"
            }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["value_num"], "120.5");
    assert_eq!(r.body["review_status"], "unreviewed");
    let old = r.body["id"].as_str().unwrap().to_owned();
    let before = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let bad = c
        .post(
            "/api/v1/claims",
            Some(&tok),
            json!({"subject_type":"part","subject_key":"BRK-100","predicate":"Bad","value_text":"x"}),
        )
        .await;
    assert_eq!(bad.body["error"], "invalid_predicate");
    let bad = c
        .post(
            "/api/v1/claims",
            Some(&tok),
            json!({"subject_type":"part","subject_key":"BRK-100","predicate":"mfg.x","value_num":"1e9"}),
        )
        .await;
    assert_eq!(bad.body["error"], "invalid_decimal");

    let s = c
        .post(&format!("/api/v1/claims/{old}/supersede"), Some(&tok), json!({
            "subject_type": "part", "subject_key": "BRK-100", "predicate": "mfg.dimension.length",
            "value_num": "121", "value_unit": "mm", "revision": "C", "artifact_id": art, "reason": "revision C drawing"
        }))
        .await;
    assert_eq!(s.status, StatusCode::OK, "{:?}", s.body);
    let new = s.body["id"].as_str().unwrap().to_owned();
    let again = c
        .post(&format!("/api/v1/claims/{old}/supersede"), Some(&tok), json!({"subject_type":"part","subject_key":"BRK-100","predicate":"mfg.dimension.length","value_num":"1","reason":"x"}))
        .await;
    assert_eq!(again.status, StatusCode::CONFLICT);

    let now = c.get("/api/v1/claims?subject_key=BRK-100", &tok).await;
    let ids: Vec<&str> = now
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [new.as_str()]);
    let then = c
        .get(
            &format!(
                "/api/v1/claims?subject_key=BRK-100&known_at={}",
                before.replace('+', "%2B")
            ),
            &tok,
        )
        .await;
    let ids: Vec<&str> = then
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [old.as_str()], "what KIZUNA knew before the correction");
    let valid = c
        .get("/api/v1/claims?subject_key=BRK-100&valid_at=2025-06-01T00:00:00Z", &tok)
        .await;
    assert_eq!(
        valid.body.as_array().unwrap().len(),
        1,
        "unknown valid_from of new claim counts as open"
    );
    let all = c
        .get("/api/v1/claims?subject_key=BRK-100&include_superseded=true", &tok)
        .await;
    assert_eq!(all.body.as_array().unwrap().len(), 2);
    let d = c.get(&format!("/api/v1/claims/{new}"), &tok).await;
    assert_eq!(d.body["history"][0]["id"], old);
    assert_eq!(d.body["artifact"]["id"], art);

    // Immutability at the database level.
    let mut tx = db::tenant_tx(&c.st.pool, t).await.unwrap();
    let e = sqlx::query("UPDATE claims SET value_text = 'x'")
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("permission denied"), "{e}");
    tx.rollback().await.unwrap();
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    let e = sqlx::query("UPDATE claims SET value_num = 1 WHERE id = $1::uuid")
        .bind(&old)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("immutable"), "{e}");
    tx.rollback().await.unwrap();
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    let e = sqlx::query("UPDATE claims SET superseded_at = NULL, superseded_by = NULL WHERE id = $1::uuid")
        .bind(&old)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("final"), "{e}");
}

#[tokio::test]
async fn evidence_view_shows_disagreement_and_rbac() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, e1) = c.user(t, "analyst").await;
    let (_, e2) = c.user(t, "viewer").await;
    let a = c.token(&slug, &e1).await;
    let v = c.token(&slug, &e2).await;
    for val in ["S355", "S275"] {
        let r = c
            .post(
                "/api/v1/claims",
                Some(&a),
                json!({"subject_type":"part","subject_key":"BRK-7","predicate":"mfg.material","value_text":val}),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK);
    }
    let ev = c.get("/api/v1/evidence/part/BRK-7", &v).await;
    assert_eq!(ev.status, StatusCode::OK);
    assert_eq!(ev.body["predicates"][0]["disagreement"], true);
    assert_eq!(
        ev.body["predicates"][0]["current"].as_array().unwrap().len(),
        2,
        "both kept"
    );
    let r = c
        .post(
            "/api/v1/claims",
            Some(&v),
            json!({"subject_type":"part","subject_key":"BRK-7","predicate":"mfg.material","value_text":"x"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(
        c.call(
            Method::POST,
            "/api/v1/artifacts/00000000-0000-0000-0000-000000000000/verify",
            Some(&v),
            None
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}

#![allow(clippy::unwrap_used)]
//! Innovation I7: sealed delivery receipts. A portal upload is receipted in the same transaction as its capture; the
//! supplier downloads a canonical receipt that the offline verifier (`kizuna-verify-pack receipt`) checks: pending
//! until a signed checkpoint covers it, final after. Out-of-scope downloads are denied and recorded.

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::*;
use kizuna_api::crypto::totp_now;
use kizuna_api::db;
use kizuna_verify_pack::VerifyOptions;
use kizuna_verify_pack::receipt::{FileDigest, ReceiptReport, verify_receipt};
use serde_json::{Value, json};
use uuid::Uuid;

async fn otx(c: &Ctx, tenant: Uuid) -> sqlx::Transaction<'static, sqlx::Postgres> {
    db::tenant_tx(&c.owner, tenant).await.unwrap()
}

/// An approved evidence request (inserted as the owner).
async fn request(c: &Ctx, tenant: Uuid, supplier: &str, by: Uuid) -> Uuid {
    let id = Uuid::now_v7();
    let docs = json!([{ "doc": Uuid::now_v7().to_string(), "name": "mill-cert.pdf", "sha256": "ab".repeat(32),
                        "items": [], "reasons": [] }]);
    let mut tx = otx(c, tenant).await;
    sqlx::query(
        "INSERT INTO evidence_requests (id, tenant_id, reference, supplier, engine, decision_ids, documents, requested_count, \
         baseline_count, exact, body_ja, body_en, content_sha256, created_by, status, reviewed_by, reviewed_at, due_on) \
         VALUES ($1,$2,$3,$4,'mfg-resupply@1',$5,$6,1,2,true,'再提出のお願い','Please resend',$7,$8,'approved',$8,now(), \
                 current_date + 14)",
    )
    .bind(id)
    .bind(tenant)
    .bind(format!("RR-{}", &id.simple().to_string()[..12].to_uppercase()))
    .bind(supplier)
    .bind(vec![Uuid::now_v7()])
    .bind(docs)
    .bind(vec![7u8; 32])
    .bind(by)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

/// Invites a supplier user, completes the invitation and attaches `request` to its supplier record.
async fn onboard(c: &Ctx, admin: &str, supplier: &str, request: Uuid) -> (Uuid, Uuid, String) {
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
    let user: Uuid = r.body["user"]["id"].as_str().unwrap().parse().unwrap();
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
    let token = r.body["access_token"].as_str().unwrap().to_owned();
    let r = c
        .post(
            &format!("/api/v1/evidence-requests/{request}/attach-supplier"),
            Some(admin),
            json!({ "supplier_id": id }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    (id, user, token)
}

async fn portal_upload(c: &Ctx, token: &str, request: Uuid, name: &str, data: &[u8]) -> Resp {
    let boundary = "kizunareceiptboundary7";
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

async fn download(c: &Ctx, token: &str, upload: &str) -> (StatusCode, Vec<u8>) {
    let (s, h, b) = c
        .get_bytes(&format!("/api/v1/portal/uploads/{upload}/receipt"), token)
        .await;
    if s == StatusCode::OK {
        assert_eq!(h["content-type"], "application/json");
        assert_eq!(h["cache-control"], "no-store");
        assert!(
            h["content-disposition"]
                .to_str()
                .unwrap()
                .contains("kizuna-delivery-receipt-")
        );
    }
    (s, b)
}

fn check(b: &[u8], file: Option<&[u8]>, key: Option<&str>) -> ReceiptReport {
    verify_receipt(
        b,
        file.map(FileDigest::of),
        &VerifyOptions {
            trusted_key: key.map(String::from),
        },
    )
}

struct W {
    c: Ctx,
    tenant: Uuid,
    admin: String,
    req_a: Uuid,
    a: (Uuid, Uuid, String),
    b: (Uuid, Uuid, String),
}

async fn world() -> W {
    let c = ctx().await;
    let (tenant, _slug, admin_id, admin) = c.admin().await;
    let req_a = request(&c, tenant, "Acme Steel K.K.", admin_id).await;
    let req_b = request(&c, tenant, "Borealis Forge", admin_id).await;
    let a = onboard(&c, &admin, "Acme Steel K.K.", req_a).await;
    let b = onboard(&c, &admin, "Borealis Forge", req_b).await;
    W {
        c,
        tenant,
        admin,
        req_a,
        a,
        b,
    }
}

#[tokio::test]
async fn portal_upload_gets_a_receipt_pending_then_final() {
    let w = world().await;
    let file = make_pdf(&["Mill certificate BRK-100", "Tensile 520 MPa"]);
    let r = portal_upload(&w.c, &w.a.2, w.req_a, "ミルシート BRK-100.pdf", &file).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let upload = r.body["id"].as_str().unwrap().to_owned();
    let receipt_id = r.body["receipt_id"]
        .as_str()
        .expect("receipt id on the upload")
        .to_owned();
    let listed =
        w.c.get(&format!("/api/v1/portal/requests/{}/uploads", w.req_a), &w.a.2)
            .await;
    assert_eq!(listed.body[0]["receipt_id"], receipt_id.as_str());

    // Before any checkpoint: consistent, pending, proves nothing.
    let (s, pending) = download(&w.c, &w.a.2, &upload).await;
    assert_eq!(s, StatusCode::OK);
    let p = check(&pending, Some(&file), None);
    assert_eq!(
        p.status,
        "pending",
        "{:?}",
        p.report.failures().map(|c| c.code).collect::<Vec<_>>()
    );
    let sum = p.summary.as_ref().unwrap();
    assert_eq!(sum.receipt_id.to_string(), receipt_id);
    assert_eq!(sum.supplier_id, w.a.0);
    assert_eq!(sum.request_id, Some(w.req_a));
    assert_eq!(sum.size, Some(file.len() as u64));
    assert_eq!(sum.filename.as_deref(), Some("ミルシート BRK-100.pdf"));
    assert_eq!(sum.channel.as_deref(), Some("supplier_portal"));
    assert_eq!(sum.uploader.as_ref().unwrap().id, w.a.1);
    assert!(p.report.proven.is_empty());

    // The capture event the receipt points at is the artifact's capture, earlier in the same chain.
    let (cap_action, cap_sha): (String, String) = sqlx::query_as(
        "SELECT action, details->>'sha256' FROM audit_events WHERE tenant_id = $1 AND seq = \
         (SELECT (details->>'capture_event_seq')::bigint FROM audit_events WHERE tenant_id = $1 AND target_id = $2 \
          AND action = 'delivery_receipt.issued')",
    )
    .bind(w.tenant)
    .bind(&receipt_id)
    .fetch_one(&mut *otx(&w.c, w.tenant).await)
    .await
    .unwrap();
    assert_eq!(cap_action, "artifact.uploaded");
    assert_eq!(cap_sha, sum.sha256);

    // A tenant admin signs a checkpoint; the same receipt downloads as final.
    let cp = w.c.post("/api/v1/audit/checkpoints", Some(&w.admin), json!({})).await;
    assert_eq!(cp.status, StatusCode::OK, "{:?}", cp.body);
    let kid = cp.body["key_id"].as_str().unwrap().to_owned();
    let (s, fin) = download(&w.c, &w.a.2, &upload).await;
    assert_eq!(s, StatusCode::OK);
    let f = check(&fin, Some(&file), Some(&kid));
    assert_eq!(
        f.status,
        "final",
        "{:?}",
        f.report.failures().map(|c| c.code).collect::<Vec<_>>()
    );
    let fs = f.summary.as_ref().unwrap();
    assert_eq!(
        (fs.event_seq, &fs.received_at, &fs.sha256),
        (sum.event_seq, &sum.received_at, &sum.sha256)
    );
    assert!(fs.checkpoint_seq.unwrap() >= fs.event_seq);
    assert!(
        f.report.proven[0].en.contains("against request"),
        "{:?}",
        f.report.proven
    );
    // Another file, another key, or one changed byte: refused.
    assert!(
        check(&fin, Some(b"%PDF-1.7 other"), None)
            .report
            .failed("file_mismatch")
    );
    assert!(
        check(&fin, None, Some("0011223344556677"))
            .report
            .failed("trusted_key_mismatch")
    );
    let mut t = fin.clone();
    let i = t.len() / 3;
    t[i] ^= 0x04;
    assert_eq!(check(&t, None, None).status, "failed");

    // Issue and download are both in the audit chain, with the supplier user as actor.
    let acts: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_events WHERE tenant_id = $1 AND actor_kind = 'supplier_user' AND actor_user_id = $2",
    )
    .bind(w.tenant)
    .bind(w.a.1)
    .fetch_all(&mut *otx(&w.c, w.tenant).await)
    .await
    .unwrap();
    for a in ["delivery_receipt.issued", "portal.receipt_downloaded"] {
        assert!(acts.iter().any(|x| x == a), "{a} missing from {acts:?}");
    }
    // The chain still verifies with the receipt events in it.
    let v = w.c.get("/api/v1/audit/verify", &w.admin).await;
    assert_eq!(v.body["intact"], true, "{:?}", v.body);
}

#[tokio::test]
async fn quarantined_and_duplicate_uploads_are_receipted_refused_types_are_not() {
    let w = world().await;
    let q = portal_upload(
        &w.c,
        &w.a.2,
        w.req_a,
        "macro.pdf",
        b"%PDF-1.7\n1 0 obj << /OpenAction << /S /JavaScript /JS (app.alert(1)) >> >> endobj\n%%EOF\n",
    )
    .await;
    assert_eq!(q.body["status"], "quarantined");
    assert!(
        q.body["receipt_id"].is_string(),
        "received is received, whatever the scan says"
    );
    let one = portal_upload(&w.c, &w.a.2, w.req_a, "a.pdf", CLEAN_PDF).await;
    let two = portal_upload(&w.c, &w.a.2, w.req_a, "a-again.pdf", CLEAN_PDF).await;
    assert_ne!(
        one.body["receipt_id"], two.body["receipt_id"],
        "each delivery has its own receipt"
    );
    let (_, b2) = download(&w.c, &w.a.2, two.body["id"].as_str().unwrap()).await;
    let r2 = check(&b2, Some(CLEAN_PDF), None);
    assert_eq!(r2.status, "pending");
    assert_eq!(r2.summary.unwrap().filename.as_deref(), Some("a-again.pdf"));
    // A refused file type never became an artifact: no receipt, and no download.
    let x = portal_upload(&w.c, &w.a.2, w.req_a, "run.exe", b"\x7fELF\x02\x01\x01\0payload").await;
    assert_eq!(x.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let listed =
        w.c.get(&format!("/api/v1/portal/requests/{}/uploads", w.req_a), &w.a.2)
            .await;
    let refused: Vec<&Value> = listed
        .body
        .as_array()
        .unwrap()
        .iter()
        .filter(|u| u["status"] == "not_accepted")
        .collect();
    assert_eq!(refused.len(), 1);
    assert!(refused[0]["receipt_id"].is_null());
    let (s, _) = download(&w.c, &w.a.2, refused[0]["id"].as_str().unwrap()).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn receipts_are_scoped_append_only_and_portal_only() {
    let w = world().await;
    let r = portal_upload(&w.c, &w.a.2, w.req_a, "a.pdf", CLEAN_PDF).await;
    let upload = r.body["id"].as_str().unwrap().to_owned();
    // Supplier B, an unknown id, and a tenant token are all refused; B's attempt is recorded.
    let (s, _) = download(&w.c, &w.b.2, &upload).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = download(&w.c, &w.a.2, &Uuid::now_v7().to_string()).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = download(&w.c, &w.admin, &upload).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let denied: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events WHERE tenant_id = $1 AND kind = 'portal.denied' \
         AND details->>'reason' = 'receipt_out_of_scope' AND details->>'supplier_user' = $2",
    )
    .bind(w.tenant)
    .bind(w.b.1.to_string())
    .fetch_one(&w.c.owner)
    .await
    .unwrap();
    assert_eq!(denied, 1);
    // Under B's portal scope, A's receipt row does not exist; nobody can change or delete it.
    let rt = runtime_pool().await;
    let mut tx = db::portal_tx(&rt, w.tenant, w.b.0).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM delivery_receipts")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 0);
    tx.rollback().await.unwrap();
    for sql in [
        "UPDATE delivery_receipts SET event_seq = event_seq + 1",
        "DELETE FROM delivery_receipts",
    ] {
        let mut tx = otx(&w.c, w.tenant).await;
        assert!(sqlx::query(sql).execute(&mut *tx).await.is_err(), "{sql}");
    }
}

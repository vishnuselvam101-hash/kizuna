#![allow(clippy::unwrap_used)]
//! Supplier portal (ADR 0023): invitation + MFA, supplier-scoped reads and uploads, revocation, limits, and proof that
//! a supplier principal reaches nothing else — every tenant API route is enumerated from the router source, and every
//! table is counted under the portal RLS scope.

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::*;
use kizuna_api::crypto::totp_now;
use kizuna_api::db;
use serde_json::{Value, json};
use uuid::Uuid;

/// Owner-role transaction in a tenant context (tables FORCE row-level security, the owner included).
async fn otx(c: &Ctx, tenant: Uuid) -> sqlx::Transaction<'static, sqlx::Postgres> {
    db::tenant_tx(&c.owner, tenant).await.unwrap()
}

/// Runs one statement as the owner in a tenant context and commits.
async fn exec(c: &Ctx, tenant: Uuid, sql: &str) {
    let mut tx = otx(c, tenant).await;
    sqlx::query(sql).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
}

const APP_RS: &str = include_str!("../src/app.rs");

fn docs(name: &str) -> Value {
    json!([{ "doc": Uuid::now_v7().to_string(), "name": name, "sha256": "ab".repeat(32),
             "items": [{ "part": "BRK-100", "predicate": "qa.material", "purchase_order": "PO-1", "po_line": "1",
                         "lot": "L-7", "heat": null }],
             "reasons": [{ "predicate": "spec.material_grade", "from": "S275", "to": "S355", "cause": "superseded" }] }])
}

/// Inserts an evidence request (as the owner) for `supplier` text; `status` draft/approved/cancelled.
async fn request(c: &Ctx, tenant: Uuid, supplier: &str, status: &str, by: Uuid) -> Uuid {
    let id = Uuid::now_v7();
    let reviewed = status != "draft";
    let mut tx = otx(c, tenant).await;
    sqlx::query(
        "INSERT INTO evidence_requests (id, tenant_id, reference, supplier, engine, decision_ids, documents, requested_count, \
         baseline_count, exact, body_ja, body_en, content_sha256, created_by, status, reviewed_by, reviewed_at, due_on) \
         VALUES ($1,$2,$3,$4,'mfg-resupply@1',$5,$6,1,2,true,'再提出のお願い','Please resend',$7,$8,$9,$10, \
                 CASE WHEN $11 THEN now() END, CASE WHEN $11 THEN current_date + 14 END)",
    )
    .bind(id)
    .bind(tenant)
    .bind(format!("RR-{}", &id.simple().to_string()[..12].to_uppercase()))
    .bind(supplier)
    .bind(vec![Uuid::now_v7()])
    .bind(docs(&format!("{supplier}-mill-cert.pdf")))
    .bind(vec![7u8; 32])
    .bind(by)
    .bind(status)
    .bind(reviewed.then_some(by))
    .bind(reviewed)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

struct Supplier {
    id: Uuid,
    user: Uuid,
    token: String,
    email: String,
    secret: Vec<u8>,
}

/// Invites `email` for `supplier` as the admin and completes the invitation (password + TOTP).
async fn onboard(c: &Ctx, admin: &str, supplier: &str) -> Supplier {
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
    assert!(invite.starts_with("kzi."));
    let user: Uuid = r.body["user"]["id"].as_str().unwrap().parse().unwrap();
    let id: Uuid = r.body["user"]["supplier_id"].as_str().unwrap().parse().unwrap();
    let r = c
        .post(
            "/api/v1/portal/invite/start",
            None,
            json!({ "token": invite, "password": PW }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
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
    assert!(token.starts_with("kzp."));
    // Single use: the same link never works again.
    let again = c
        .post(
            "/api/v1/portal/invite/start",
            None,
            json!({ "token": invite, "password": PW }),
        )
        .await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED);
    Supplier {
        id,
        user,
        token,
        email,
        secret,
    }
}

async fn attach(c: &Ctx, admin: &str, request: Uuid, supplier: Uuid) {
    let r = c
        .post(
            &format!("/api/v1/evidence-requests/{request}/attach-supplier"),
            Some(admin),
            json!({ "supplier_id": supplier }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
}

async fn portal_upload(c: &Ctx, token: &str, request: Uuid, name: &str, data: &[u8]) -> Resp {
    let boundary = "kizunaportalboundary91";
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

struct World {
    c: Ctx,
    tenant: Uuid,
    admin: String,
    admin_id: Uuid,
    a: Supplier,
    b: Supplier,
    a_open: Uuid,
    a_draft: Uuid,
    a_cancelled: Uuid,
    b_open: Uuid,
    other_open: Uuid,
    other_tenant: Uuid,
}

async fn world() -> World {
    let c = ctx().await;
    let (tenant, _slug, admin_id, admin) = c.admin().await;
    // Requests exist before the supplier is invited.
    let a_open = request(&c, tenant, "Acme Steel K.K.", "approved", admin_id).await;
    let a_draft = request(&c, tenant, "ACME STEEL K.K.", "draft", admin_id).await;
    let a_cancelled = request(&c, tenant, "Acme Steel K.K.", "cancelled", admin_id).await;
    let b_open = request(&c, tenant, "Borealis Forge", "approved", admin_id).await;
    let a = onboard(&c, &admin, "Acme Steel K.K.").await;
    let b = onboard(&c, &admin, "Borealis Forge").await;
    // Nothing is linked by name: the admin attaches each request explicitly.
    for (r, s) in [(a_open, a.id), (a_draft, a.id), (a_cancelled, a.id), (b_open, b.id)] {
        attach(&c, &admin, r, s).await;
    }
    // Another tenant with a supplier of the same name and an approved request.
    let (t2, _s2, admin2_id, admin2) = c.admin().await;
    let other_open = request(&c, t2, "Acme Steel K.K.", "approved", admin2_id).await;
    let o = onboard(&c, &admin2, "Acme Steel K.K.").await;
    attach(&c, &admin2, other_open, o.id).await;
    World {
        c,
        tenant,
        admin,
        admin_id,
        a,
        b,
        a_open,
        a_draft,
        a_cancelled,
        b_open,
        other_open,
        other_tenant: t2,
    }
}

#[tokio::test]
async fn supplier_sees_only_its_open_requests_and_minimal_fields() {
    let w = world().await;
    let r = w.c.get("/api/v1/portal/requests", &w.a.token).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let ids: Vec<&str> = r
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec![w.a_open.to_string().as_str()],
        "only the approved request of supplier A"
    );
    let item = &r.body[0];
    assert!(item["due_on"].as_str().is_some());
    assert_eq!(item["documents"][0]["items"][0]["what_en"], "material grade");
    // Internal data never leaves: no decisions, document ids, hashes, change reasons, reviewers or supplier ids.
    let text = r.body.to_string();
    for forbidden in [
        "decision",
        "sha256",
        "reasons",
        "S275",
        "reviewed_by",
        "created_by",
        "supplier_id",
        "\"doc\"",
    ] {
        assert!(!text.contains(forbidden), "leaked {forbidden}: {text}");
    }
    let r =
        w.c.get(&format!("/api/v1/portal/requests/{}", w.a_open), &w.a.token)
            .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["body_en"], "Please resend");
    assert_eq!(r.body["body_ja"], "再提出のお願い");
    // Out of scope: other supplier, draft, cancelled, other tenant, unknown — all 404, all recorded.
    for id in [w.b_open, w.a_draft, w.a_cancelled, w.other_open, Uuid::now_v7()] {
        for path in [
            format!("/api/v1/portal/requests/{id}"),
            format!("/api/v1/portal/requests/{id}/uploads"),
        ] {
            let r = w.c.get(&path, &w.a.token).await;
            assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
        }
        let r = portal_upload(&w.c, &w.a.token, id, "x.pdf", CLEAN_PDF).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
    }
    let denials: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events WHERE tenant_id = $1 AND kind = 'portal.denied' \
         AND details->>'reason' = 'request_out_of_scope'",
    )
    .bind(w.tenant)
    .fetch_one(&w.c.owner)
    .await
    .unwrap();
    assert!(denials >= 4, "denials recorded: {denials}");
    // Every portal read is audited with the supplier user as actor.
    let acts: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_events WHERE tenant_id = $1 AND actor_kind = 'supplier_user' AND actor_user_id = $2",
    )
    .bind(w.tenant)
    .bind(w.a.user)
    .fetch_all(&mut *otx(&w.c, w.tenant).await)
    .await
    .unwrap();
    for a in [
        "portal_user.activated",
        "portal.login",
        "portal.requests_listed",
        "portal.request_viewed",
    ] {
        assert!(acts.iter().any(|x| x == a), "{a} missing from {acts:?}");
    }
    let _ = (&w.admin, w.admin_id, &w.b.email, w.b.user, &w.a.secret);
}

#[tokio::test]
async fn upload_goes_through_pipeline_and_approves_nothing() {
    let w = world().await;
    let r = portal_upload(&w.c, &w.a.token, w.a_open, "mill-cert.pdf", CLEAN_PDF).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "received_for_review");
    assert!(r.body.get("artifact_id").is_none(), "no internal ids to the supplier");
    let r =
        w.c.get(&format!("/api/v1/portal/requests/{}/uploads", w.a_open), &w.a.token)
            .await;
    assert_eq!(r.body.as_array().unwrap().len(), 1);
    // Supplier B never sees A's upload, even through its own request.
    let r =
        w.c.get(&format!("/api/v1/portal/requests/{}/uploads", w.b_open), &w.b.token)
            .await;
    assert_eq!(r.body.as_array().unwrap().len(), 0);
    // Internally: same pipeline (scanned artifact, capture), marked as supplier portal origin; request unchanged.
    let (origin, status, kind, cap_origin): (String, String, String, String) = sqlx::query_as(
        "SELECT a.origin, a.status, a.kind, c.origin FROM portal_uploads p JOIN artifacts a ON a.id = p.artifact_id \
         JOIN artifact_captures c ON c.artifact_id = a.id WHERE p.request_id = $1",
    )
    .bind(w.a_open)
    .fetch_one(&mut *otx(&w.c, w.tenant).await)
    .await
    .unwrap();
    assert_eq!(
        (origin.as_str(), status.as_str(), kind.as_str(), cap_origin.as_str()),
        ("supplier_portal", "accepted", "inspection_report", "supplier_portal")
    );
    let claims: i64 = sqlx::query_scalar("SELECT count(*) FROM claims WHERE tenant_id = $1")
        .bind(w.tenant)
        .fetch_one(&mut *otx(&w.c, w.tenant).await)
        .await
        .unwrap();
    assert_eq!(claims, 0, "nothing extracted or accepted on the supplier's behalf");
    let r =
        w.c.get(
            &format!("/api/v1/evidence-requests/{}/portal-uploads", w.a_open),
            &w.admin,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body[0]["uploaded_by"], w.a.email.as_str());
    let r = w.c.get("/api/v1/artifacts", &w.admin).await;
    assert_eq!(r.body[0]["origin"], "supplier_portal");
    // A quarantined file is reported as such; a refused type as rejected.
    let r = portal_upload(
        &w.c,
        &w.a.token,
        w.a_open,
        "macro.pdf",
        b"%PDF-1.7\n1 0 obj << /OpenAction << /S /JavaScript /JS (app.alert(1)) >> >> endobj\n%%EOF\n",
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "quarantined");
    let r = portal_upload(&w.c, &w.a.token, w.a_open, "run.exe", b"\x7fELF\x02\x01\x01\0payload").await;
    assert_eq!(r.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let st: Vec<String> =
        sqlx::query_scalar("SELECT status FROM portal_uploads WHERE request_id = $1 ORDER BY created_at")
            .bind(w.a_open)
            .fetch_all(&mut *otx(&w.c, w.tenant).await)
            .await
            .unwrap();
    assert_eq!(st, ["received_for_review", "quarantined", "not_accepted"]);
    let r =
        w.c.get(&format!("/api/v1/portal/requests/{}/uploads", w.a_open), &w.a.token)
            .await;
    assert_eq!(r.body[0]["reason_code"], "file_type_not_accepted", "{:?}", r.body);
}

#[tokio::test]
async fn per_supplier_size_and_daily_limits() {
    let w = world().await;
    exec(
        &w.c,
        w.tenant,
        &format!(
            "UPDATE suppliers SET max_upload_mb = 1, max_uploads_per_day = 2 WHERE id = '{}'",
            w.a.id
        ),
    )
    .await;
    let mut big = b"%PDF-1.7\n".to_vec();
    big.resize(1024 * 1024 + 10, b' ');
    let r = portal_upload(&w.c, &w.a.token, w.a_open, "big.pdf", &big).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    for i in 0..2 {
        let mut pdf = CLEAN_PDF.to_vec();
        pdf.extend_from_slice(format!("% {i}\n").as_bytes());
        let r = portal_upload(&w.c, &w.a.token, w.a_open, "c.pdf", &pdf).await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    }
    let r = portal_upload(&w.c, &w.a.token, w.a_open, "c.pdf", CLEAN_PDF).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    // Supplier B is unaffected by A's limits.
    let r = portal_upload(&w.c, &w.b.token, w.b_open, "b.pdf", b"%PDF-1.7\n% b\n%%EOF\n").await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
}

#[tokio::test]
async fn mfa_is_required_and_revocation_is_immediate() {
    let w = world().await;
    let slug: String = sqlx::query_scalar("SELECT slug FROM tenants WHERE id = $1")
        .bind(w.tenant)
        .fetch_one(&w.c.owner)
        .await
        .unwrap();
    let login = |code: Value| json!({ "tenant": slug, "email": w.a.email, "password": PW, "totp_code": code });
    // No code, wrong code: no session.
    let r =
        w.c.post(
            "/api/v1/portal/login",
            None,
            json!({ "tenant": slug, "email": w.a.email, "password": PW }),
        )
        .await;
    assert!(r.status.is_client_error() && r.body.get("access_token").is_none());
    let r = w.c.post("/api/v1/portal/login", None, login(json!("000000"))).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r =
        w.c.post(
            "/api/v1/portal/login",
            None,
            login(json!(totp_now(&w.a.secret, now() + 30))),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let second = r.body["access_token"].as_str().unwrap().to_owned();
    // The supplier cannot use the tenant login, nor the tenant user the portal.
    let r = w.c.login(&slug, &w.a.email, PW, None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = w.c.get("/api/v1/portal/requests", &w.admin).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Revoke: every session ends at once; the account cannot sign in again.
    let r =
        w.c.post(
            &format!("/api/v1/portal-users/{}/revoke", w.a.user),
            Some(&w.admin),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "revoked");
    for t in [&w.a.token, &second] {
        assert_eq!(
            w.c.get("/api/v1/portal/requests", t).await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    let r =
        w.c.post(
            "/api/v1/portal/login",
            None,
            login(json!(totp_now(&w.a.secret, now() + 60))),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // Only an MFA-verified admin invites; a reviewer cannot.
    let (rev, rev_email) = w.c.user(w.tenant, "reviewer").await;
    let rt = w.c.token(&slug, &rev_email).await;
    let r =
        w.c.post(
            "/api/v1/portal-users",
            Some(&rt),
            json!({ "supplier": "X", "email": "x@y.test", "display_name": "X" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let _ = rev;
    // Expired invitations are refused.
    let r =
        w.c.post(
            "/api/v1/portal-users",
            Some(&w.admin),
            json!({ "supplier_id": w.a.id, "email": "late@s.test", "display_name": "L" }),
        )
        .await;
    let inv = r.body["invite_token"].as_str().unwrap().to_owned();
    exec(
        &w.c,
        w.tenant,
        "UPDATE supplier_users SET invite_expires_at = now() - interval '1 minute' WHERE email = 'late@s.test'",
    )
    .await;
    let r =
        w.c.post(
            "/api/v1/portal/invite/start",
            None,
            json!({ "token": inv, "password": PW }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let kinds = w.c.security_kinds(w.tenant).await;
    for k in ["portal.denied", "auth.sessions_revoked", "auth.login_failed"] {
        assert!(kinds.iter().any(|x| x == k), "{k} missing from {kinds:?}");
    }
}

/// Every (path, method) registered on the router, read from the router source so a new route is covered by default.
fn routes() -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = APP_RS;
    while let Some(i) = rest.find(".route(") {
        rest = &rest[i + 7..];
        let q = rest.find('"').unwrap();
        let tail = &rest[q + 1..];
        out.push(tail[..tail.find('"').unwrap()].to_string());
    }
    assert!(out.len() > 80, "router parse found only {} routes", out.len());
    out
}

#[tokio::test]
async fn supplier_token_is_refused_on_every_tenant_route() {
    let w = world().await;
    let mut checked = 0;
    for path in routes() {
        if path.starts_with("/portal/") || path == "/healthz" {
            continue;
        }
        // Real identifiers of other rows, so a scope bug would surface as data.
        let concrete = path
            .replace("{id}", &w.b_open.to_string())
            .replace("{subject_type}", "part")
            .replace("{subject_key}", "BRK-100")
            .replace("{key}", "BRK-100")
            .replace("{action}", "x")
            .replace("{verb}", "approve");
        for m in [Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            let r =
                w.c.call(
                    m.clone(),
                    &format!("/api/v1{concrete}"),
                    Some(&w.a.token),
                    Some(json!({})),
                )
                .await;
            assert!(
                !r.status.is_success(),
                "{m} {concrete} answered {} to a supplier token: {:?}",
                r.status,
                r.body
            );
            let text = r.body.to_string();
            assert!(
                !text.contains(&w.b_open.to_string()) && !text.contains("Borealis"),
                "{m} {concrete} leaked: {text}"
            );
            checked += 1;
        }
        // Authenticated tenant routes refuse the supplier principal explicitly.
        if !path.starts_with("/auth/login") && !path.starts_with("/auth/refresh") && !path.starts_with("/auth/webauthn")
        {
            let r = w.c.get(&format!("/api/v1{concrete}"), &w.a.token).await;
            assert!(
                matches!(r.status, StatusCode::FORBIDDEN | StatusCode::METHOD_NOT_ALLOWED),
                "GET {concrete}: {}",
                r.status
            );
        }
    }
    assert!(checked > 400);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events WHERE kind = 'portal.denied' AND details->>'reason' = 'supplier_token_on_tenant_api' \
         AND tenant_id = $1",
    )
    .bind(w.tenant)
    .fetch_one(&w.c.owner)
    .await
    .unwrap();
    assert!(n > 0);
}

#[tokio::test]
async fn rls_portal_scope_denies_every_other_row() {
    let w = world().await;
    // Something in every corner of the tenant: an internal upload, a portal upload of B, audit and security events.
    let _ =
        w.c.upload(
            &w.admin,
            "internal.pdf",
            b"%PDF-1.7\n% internal\n%%EOF\n",
            Some("drawing"),
        )
        .await;
    let r = portal_upload(&w.c, &w.b.token, w.b_open, "b.pdf", b"%PDF-1.7\n% bb\n%%EOF\n").await;
    assert_eq!(r.status, StatusCode::OK);
    let rt = runtime_pool().await;
    let tables: Vec<(String, bool)> = sqlx::query_as(
        "SELECT c.relname::text, c.relrowsecurity FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relkind = 'r' AND has_table_privilege(current_user, c.oid, 'SELECT') ORDER BY 1",
    )
    .fetch_all(&rt)
    .await
    .unwrap();
    // Tables without RLS readable by the runtime role are platform tables only (no tenant data).
    let open: Vec<&str> = tables.iter().filter(|t| !t.1).map(|t| t.0.as_str()).collect();
    assert_eq!(open, ["tenants"], "a readable table without RLS");
    let expect = |t: &str| match t {
        "suppliers" | "evidence_requests" => Some(1),
        "portal_uploads" => Some(0),
        _ => Some(0),
    };
    let mut tx = db::portal_tx(&rt, w.tenant, w.a.id).await.unwrap();
    for (t, _) in tables.iter().filter(|t| t.1) {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {t}"))
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(Some(n), expect(t), "table {t} under supplier A's portal scope");
    }
    tx.rollback().await.unwrap();
    // Cross-tenant: A's supplier id with another tenant (or B's id with A's tenant mix-up) sees nothing.
    let mut tx = db::portal_tx(&rt, w.other_tenant, w.a.id).await.unwrap();
    for t in ["suppliers", "evidence_requests", "portal_uploads"] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {t}"))
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(n, 0, "{t} across tenants");
    }
    tx.rollback().await.unwrap();
    // Writes are refused by RLS in the portal scope even outside a read-only transaction, and with a tenant set too.
    let mut tx = rt.begin().await.unwrap();
    sqlx::query(
        "SELECT set_config('kizuna.portal_tenant_id', $1, true), set_config('kizuna.portal_supplier_id', $2, true), \
                 set_config('kizuna.tenant_id', $1, true)",
    )
    .bind(w.tenant.to_string())
    .bind(w.a.id.to_string())
    .execute(&mut *tx)
    .await
    .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM evidence_requests")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 1, "the restrictive policy holds even with a tenant context");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 0);
    let e = sqlx::query("UPDATE evidence_requests SET status = 'cancelled' WHERE id = $1")
        .bind(w.a_open)
        .execute(&mut *tx)
        .await;
    assert!(
        e.is_err() || e.unwrap().rows_affected() == 0,
        "supplier scope cannot change a request"
    );
    tx.rollback().await.unwrap();
    let st: String = sqlx::query_scalar("SELECT status FROM evidence_requests WHERE id = $1")
        .bind(w.a_open)
        .fetch_one(&mut *otx(&w.c, w.tenant).await)
        .await
        .unwrap();
    assert_eq!(st, "approved");
}

#[tokio::test]
async fn requests_attach_only_explicitly_and_similar_names_see_nothing() {
    let w = world().await;
    // A request for "Acme Steel K.K." drafted before anything is attached stays invisible to supplier A...
    let id = request(&w.c, w.tenant, "acme steel k.k.", "approved", w.admin_id).await;
    let n = |b: &Value| b.as_array().unwrap().len();
    assert_eq!(n(&w.c.get("/api/v1/portal/requests", &w.a.token).await.body), 1);
    // ...and to a similarly named supplier with its own record.
    let sim = onboard(&w.c, &w.admin, "Acme Steel").await;
    assert_ne!(sim.id, w.a.id);
    assert_eq!(n(&w.c.get("/api/v1/portal/requests", &sim.token).await.body), 0);
    assert_eq!(
        w.c.get(&format!("/api/v1/portal/requests/{id}"), &sim.token)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // A new supplier whose normalised name collides is refused (the admin must pick the existing record by id).
    let r =
        w.c.post(
            "/api/v1/portal-users",
            Some(&w.admin),
            json!({ "supplier": "ACME STEEL K.K.", "email": "z@z.test", "display_name": "Z" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{:?}", r.body);
    // Only a tenant admin attaches; the attach is audited and final.
    let slug: String = sqlx::query_scalar("SELECT slug FROM tenants WHERE id = $1")
        .bind(w.tenant)
        .fetch_one(&w.c.owner)
        .await
        .unwrap();
    let (_, rev) = w.c.user(w.tenant, "reviewer").await;
    let rt = w.c.mfa_token(&slug, &rev).await;
    let path = format!("/api/v1/evidence-requests/{id}/attach-supplier");
    assert_eq!(
        w.c.post(&path, Some(&rt), json!({ "supplier_id": w.a.id }))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    attach(&w.c, &w.admin, id, w.a.id).await;
    assert_eq!(n(&w.c.get("/api/v1/portal/requests", &w.a.token).await.body), 2);
    assert_eq!(n(&w.c.get("/api/v1/portal/requests", &sim.token).await.body), 0);
    let again = w.c.post(&path, Some(&w.admin), json!({ "supplier_id": sim.id })).await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'evidence_request.supplier_attached' AND target_id = $1",
    )
    .bind(id.to_string())
    .fetch_one(&mut *otx(&w.c, w.tenant).await)
    .await
    .unwrap();
    assert_eq!(audited, 1);
    // Four-eyes approval of a draft with an explicit due date.
    let draft = request(&w.c, w.tenant, "Acme Steel K.K.", "draft", w.admin_id).await;
    let due = (time::OffsetDateTime::now_utc().date() + time::Duration::days(20)).to_string();
    let r =
        w.c.post(
            &format!("/api/v1/evidence-requests/{draft}/approve"),
            Some(&rt),
            json!({ "comment": "ok", "due_on": due }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["due_on"], due.as_str());
    assert!(r.body["supplier_id"].is_null(), "approval never links by name");
}

#[tokio::test]
async fn upload_status_follows_release_and_rejection() {
    let w = world().await;
    let active = b"%PDF-1.7\n1 0 obj << /OpenAction << /S /JavaScript /JS (app.alert(2)) >> >> endobj\n%%EOF\n";
    let r = portal_upload(&w.c, &w.a.token, w.a_open, "q.pdf", active).await;
    assert_eq!(r.body["status"], "quarantined", "{:?}", r.body);
    let art: Uuid = sqlx::query_scalar("SELECT artifact_id FROM portal_uploads WHERE request_id = $1")
        .bind(w.a_open)
        .fetch_one(&mut *otx(&w.c, w.tenant).await)
        .await
        .unwrap();
    let rel =
        w.c.post(
            &format!("/api/v1/artifacts/{art}/release"),
            Some(&w.admin),
            json!({ "reason": "checked: harmless" }),
        )
        .await;
    assert_eq!(rel.status, StatusCode::OK, "{:?}", rel.body);
    let list = format!("/api/v1/portal/requests/{}/uploads", w.a_open);
    let r = w.c.get(&list, &w.a.token).await;
    assert_eq!(r.body[0]["status"], "received_for_review");
    assert!(r.body[0]["reason_code"].is_null());
    // Rejected later: "not accepted" with a neutral reason, never the internal finding or reviewer text.
    exec(
        &w.c,
        w.tenant,
        &format!("UPDATE artifacts SET status = 'rejected' WHERE id = '{art}'"),
    )
    .await;
    let r = w.c.get(&list, &w.a.token).await;
    assert_eq!(r.body[0]["status"], "not_accepted");
    assert_eq!(r.body[0]["reason_code"], "not_accepted_after_review");
    let text = r.body.to_string();
    assert!(!text.contains("JavaScript") && !text.contains("harmless"), "{text}");
    // Final: a later status change does not bring it back.
    exec(
        &w.c,
        w.tenant,
        &format!("UPDATE artifacts SET status = 'accepted' WHERE id = '{art}'"),
    )
    .await;
    assert_eq!(w.c.get(&list, &w.a.token).await.body[0]["status"], "not_accepted");
}

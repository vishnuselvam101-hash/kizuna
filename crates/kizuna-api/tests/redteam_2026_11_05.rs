#![allow(clippy::unwrap_used, clippy::too_many_lines)]
//! Red-team reproductions (2026-11-05, docs/security/REDTEAM_2026-11-05.md). Every test is an attack against the
//! in-process app on a real PostgreSQL; a passing test means the attack was refused. Each attempt prints one
//! `RT|<attack>|<expected>|<actual>` line (run with `--nocapture` to collect the attempts table).

mod common;

use std::time::Instant;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::*;
use serde_json::{Value, json};
use uuid::Uuid;

fn rt(attack: &str, expected: &str, actual: impl std::fmt::Display) {
    println!("RT|{attack}|{expected}|{actual}");
}

/// Higher per-IP login budget: one context logs in many seeded users from a single test IP.
async fn rctx() -> Ctx {
    ctx_with(|cfg| {
        cfg.rate.login_per_ip_per_min = 10_000;
    })
    .await
}

struct Seed {
    t: Uuid,
    slug: String,
    admin: String,
    key: String,
    art: String,
    sha: String,
    claim: String,
    dec: String,
    conf: String,
    user: Uuid,
}

async fn seed(c: &Ctx, tag: &str) -> Seed {
    let (t, slug, _uid, admin) = c.admin().await;
    let key = format!("RT-{tag}-{}", &Uuid::new_v4().simple().to_string()[..6]).to_uppercase();
    let pdf = make_pdf(&[
        "SUPPLIER SPECIFICATION SHEET",
        &format!("Part Number: {key}"),
        "Revision: C",
        "Material: S355JR",
        "Length (mm): 120",
    ]);
    let art = c.ingest(&admin, "seed.pdf", &pdf, "spec_document").await;
    let ab = c.get(&format!("/api/v1/artifacts/{art}"), &admin).await.body;
    let sha = ab["sha256"]
        .as_str()
        .or(ab["artifact"]["sha256"].as_str())
        .unwrap()
        .to_owned();
    let mk = |v: &str| json!({"subject_type":"part","subject_key":key,"predicate":"mfg.material","value_text":v});
    let r = c.post("/api/v1/claims", Some(&admin), mk("C45")).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let claim = r.body["id"].as_str().unwrap().to_owned();
    c.post("/api/v1/claims", Some(&admin), mk("S355")).await;
    c.post("/api/v1/verification/run", Some(&admin), json!({})).await;
    let conf = c
        .get("/api/v1/conflicts", &admin)
        .await
        .body
        .as_array()
        .and_then(|a| a.first().cloned());
    let conf = conf
        .and_then(|v| v["id"].as_str().map(str::to_owned))
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let r = c
        .post(
            "/api/v1/decisions",
            Some(&admin),
            json!({"kind":"supplier_part_release","title":"rt","subject_type":"part","subject_key":key,
                   "outcome":"approved","rationale":"rt seed","evidence":[{"claim_id": claim}],"depends_on":[]}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let dec = r.body["id"].as_str().unwrap().to_owned();
    let (user, _) = c.user(t, "viewer").await;
    Seed {
        t,
        slug,
        admin,
        key,
        art,
        sha,
        claim,
        dec,
        conf,
        user,
    }
}

/// (method, path template, body, permission name or "" for authenticated-only)
fn routes() -> Vec<(Method, &'static str, Value, &'static str)> {
    let claim = json!({"subject_type":"part","subject_key":"{key}","predicate":"mfg.material","value_text":"X5"});
    vec![
        (Method::GET, "/auth/me", Value::Null, ""),
        (Method::GET, "/users", Value::Null, "UserRead"),
        (
            Method::POST,
            "/users",
            json!({"email":"rt-{rand}@example.test","display_name":"rt","role":"tenant_admin","password":"Zq8#unique-rt-pass-phrase"}),
            "UserManage",
        ),
        (
            Method::PATCH,
            "/users/{user}",
            json!({"role":"tenant_admin"}),
            "UserManage",
        ),
        (Method::POST, "/users/{user}/revoke-sessions", json!({}), "UserManage"),
        (Method::POST, "/users/{user}/reset-mfa", json!({}), "UserManage"),
        (Method::GET, "/users/{user}/sessions", Value::Null, "UserRead"),
        (Method::GET, "/audit/events", Value::Null, "AuditRead"),
        (Method::GET, "/audit/verify", Value::Null, "AuditVerify"),
        (Method::GET, "/security-events", Value::Null, "SecurityEventRead"),
        (Method::GET, "/artifacts", Value::Null, "ArtifactRead"),
        (Method::GET, "/artifacts/{art}", Value::Null, "ArtifactRead"),
        (Method::GET, "/artifacts/{art}/content", Value::Null, "ArtifactRead"),
        (Method::POST, "/artifacts/{art}/verify", json!({}), "ArtifactRead"),
        (
            Method::POST,
            "/artifacts/{art}/release",
            json!({"comment":"rt"}),
            "IncidentManage",
        ),
        (Method::GET, "/claims", Value::Null, "ClaimRead"),
        (Method::POST, "/claims", claim.clone(), "ClaimWrite"),
        (Method::GET, "/claims/{claim}", Value::Null, "ClaimRead"),
        (
            Method::POST,
            "/claims/{claim}/supersede",
            json!({"create": claim, "reason":"rt"}),
            "ClaimWrite",
        ),
        (Method::GET, "/evidence/part/{key}", Value::Null, "ClaimRead"),
        (
            Method::POST,
            "/artifacts/{art}/reextract",
            json!({"reason":"rt"}),
            "ClaimWrite",
        ),
        (
            Method::POST,
            "/claims/{claim}/review",
            json!({"decision":"accept","comment":"rt"}),
            "ReviewDecide",
        ),
        (Method::POST, "/verification/run", json!({}), "ClaimWrite"),
        (Method::GET, "/conflicts", Value::Null, "ClaimRead"),
        (Method::GET, "/conflicts/{conf}", Value::Null, "ClaimRead"),
        (
            Method::POST,
            "/conflicts/{conf}/review",
            json!({"decision":"waive","comment":"rt","faulty_claims":["{claim}"]}),
            "ReviewDecide",
        ),
        (Method::GET, "/entities/candidates", Value::Null, "ClaimRead"),
        (
            Method::POST,
            "/entities/decisions",
            json!({"subject_type":"part","left_key":"{key}","right_key":"{key}X","outcome":"same"}),
            "ReviewDecide",
        ),
        (Method::GET, "/compare/part/{key}", Value::Null, "ClaimRead"),
        (Method::GET, "/decisions", Value::Null, "DecisionRead"),
        (
            Method::POST,
            "/decisions",
            json!({"kind":"supplier_part_release","title":"rt2","subject_type":"part","subject_key":"{key}","outcome":"approved","rationale":"rt","evidence":[{"claim_id":"{claim}"}],"depends_on":[{"decision_id":"{dec}"}]}),
            "DecisionWrite",
        ),
        (Method::GET, "/decisions/{dec}", Value::Null, "DecisionRead"),
        (Method::GET, "/decisions/{dec}/trace", Value::Null, "DecisionRead"),
        (Method::GET, "/decisions/{dec}/report", Value::Null, "DecisionRead"),
        (
            Method::POST,
            "/decisions/{dec}/reconfirm",
            json!({"comment":"rt"}),
            "ReviewDecide",
        ),
        (Method::GET, "/security/summary", Value::Null, "SecurityEventRead"),
        (Method::GET, "/security/status", Value::Null, "SecurityEventRead"),
        (Method::GET, "/dashboard/summary", Value::Null, "ClaimRead"),
        (Method::GET, "/subjects", Value::Null, "ClaimRead"),
        (Method::POST, "/reports", json!({"scope":"all"}), "AuditRead"),
        (Method::GET, "/integrations/import-settings", Value::Null, "UserRead"),
        (
            Method::PUT,
            "/integrations/import-settings",
            json!({"expected_version":0,"settings":{"csv":{"header_row":1}}}),
            "UserManage",
        ),
        (Method::GET, "/integrations/ai-extraction", Value::Null, "UserRead"),
        (
            Method::PUT,
            "/integrations/ai-extraction",
            json!({"expected_version":0,"enabled":false}),
            "UserManage",
        ),
        (
            Method::GET,
            "/security/detection-settings",
            Value::Null,
            "SecurityEventRead",
        ),
        (
            Method::PUT,
            "/security/detection-settings",
            json!({"expected_version":0,"settings":{}}),
            "UserManage",
        ),
        (Method::GET, "/security/incidents", Value::Null, "SecurityEventRead"),
        (
            Method::GET,
            "/security/incidents/{rand}",
            Value::Null,
            "SecurityEventRead",
        ),
        (
            Method::POST,
            "/security/incidents/{rand}/resolve",
            json!({"status":"resolved","comment":"rt"}),
            "IncidentManage",
        ),
        (
            Method::POST,
            "/security/incidents/{rand}/actions/{rand}/approve",
            json!({"comment":"rt"}),
            "IncidentManage",
        ),
        (Method::POST, "/security/detect", json!({}), "IncidentManage"),
        (Method::POST, "/audit/checkpoints", json!({}), "AuditVerify"),
        (Method::GET, "/audit/checkpoints/verify", Value::Null, "AuditVerify"),
        (
            Method::POST,
            "/audit/checkpoints/verify-external",
            json!({"checkpoints":[]}),
            "AuditVerify",
        ),
        (Method::POST, "/artifacts/{art}/extract", json!({}), "ClaimWrite"),
        (
            Method::POST,
            "/decisions/{dec}/revoke",
            json!({"comment":"rt"}),
            "ReviewDecide",
        ),
    ]
}

fn fill(s: &str, sd: &Seed) -> String {
    s.replace("{art}", &sd.art)
        .replace("{claim}", &sd.claim)
        .replace("{dec}", &sd.dec)
        .replace("{conf}", &sd.conf)
        .replace("{user}", &sd.user.to_string())
        .replace("{key}", &sd.key)
        .replace("{rand}", &Uuid::new_v4().simple().to_string())
}

async fn send(c: &Ctx, m: &Method, path: &str, tok: &str, body: &Value) -> Resp {
    let b = if body.is_null() { None } else { Some(body.clone()) };
    c.call(m.clone(), &format!("/api/v1{path}"), Some(tok), b).await
}

fn perm_granted(role: &str, perm: &str) -> bool {
    use policy_engine::{Permission::*, Role, role_grants};
    let p = match perm {
        "" => return true,
        "UserRead" => UserRead,
        "UserManage" => UserManage,
        "AuditRead" => AuditRead,
        "AuditVerify" => AuditVerify,
        "SecurityEventRead" => SecurityEventRead,
        "ArtifactRead" => ArtifactRead,
        "ArtifactUpload" => ArtifactUpload,
        "ClaimRead" => ClaimRead,
        "ClaimWrite" => ClaimWrite,
        "ReviewDecide" => ReviewDecide,
        "DecisionRead" => DecisionRead,
        "DecisionWrite" => DecisionWrite,
        "IncidentManage" => IncidentManage,
        _ => panic!("{perm}"),
    };
    role_grants(Role::parse(role).unwrap(), p)
}

/// Cross-tenant reads/writes on every routed endpoint, using the other tenant's real ids (IDOR) as tenant B's admin.
#[tokio::test]
async fn rt_cross_tenant_every_route() {
    let c = rctx().await;
    let a = seed(&c, "A").await;
    let b = seed(&c, "B").await;
    let secrets = [&a.art, &a.claim, &a.dec, &a.sha];
    let mut leaks = Vec::new();
    for (m, tmpl, body, _) in routes() {
        let path = fill(tmpl, &a);
        let body = serde_json::from_str::<Value>(&fill(&body.to_string(), &a)).unwrap();
        let r = send(&c, &m, &path, &b.admin, &body).await;
        let txt = r.body.to_string();
        let leaked: Vec<&&String> = secrets.iter().filter(|s| txt.contains(s.as_str())).collect();
        // The seed conflict id is only meaningful when it exists in A.
        // A 2xx on a route addressing one of A's objects by id is only acceptable when it returns nothing.
        let by_id = ["{art}", "{claim}", "{dec}", "{conf}", "{user}"]
            .iter()
            .any(|k| tmpl.contains(k));
        let empty = r.body == json!([]) || r.body.is_null();
        let ok = leaked.is_empty() && (!r.status.is_success() || !by_id || empty);
        rt(
            &format!("cross-tenant {m} {tmpl} with A ids as B admin"),
            "non-2xx or no A data",
            format!("{} leak={}", r.status.as_u16(), !leaked.is_empty()),
        );
        if !ok {
            leaks.push(format!("{m} {tmpl}: {} {txt:.200}", r.status));
        }
    }
    // Writes in B that reference A objects by id must not bind to them.
    let b_claim_on_a_art = c
        .post(
            "/api/v1/claims",
            Some(&b.admin),
            json!({"subject_type":"part","subject_key":"X","predicate":"mfg.material","value_text":"x","artifact_id": a.art}),
        )
        .await;
    rt("B claim citing A artifact_id", "4xx", b_claim_on_a_art.status.as_u16());
    assert!(b_claim_on_a_art.status.is_client_error());
    let b_dec_on_a = c
        .post(
            "/api/v1/decisions",
            Some(&b.admin),
            json!({"kind":"supplier_part_release","title":"x","subject_type":"part","subject_key":"X","outcome":"approved",
                   "rationale":"x","evidence":[{"claim_id": a.claim}],"depends_on":[{"decision_id": a.dec}]}),
        )
        .await;
    rt(
        "B decision citing A claim + A decision",
        "4xx",
        b_dec_on_a.status.as_u16(),
    );
    assert!(b_dec_on_a.status.is_client_error());
    let b_conf_with_a_claim = c
        .post(
            &format!("/api/v1/conflicts/{}/review", b.conf),
            Some(&b.admin),
            json!({"decision":"confirm","comment":"x","faulty_claims":[a.claim]}),
        )
        .await;
    rt(
        "B conflict review naming A claim as faulty",
        "4xx",
        b_conf_with_a_claim.status.as_u16(),
    );
    assert!(!b_conf_with_a_claim.status.is_success() || !b_conf_with_a_claim.body.to_string().contains(&a.claim));
    // A's objects are unchanged.
    let still = c.get(&format!("/api/v1/claims/{}", a.claim), &a.admin).await;
    assert_eq!(still.status, StatusCode::OK);
    assert!(leaks.is_empty(), "cross-tenant leaks: {leaks:#?}");
    let _ = (a.t, b.t, &a.slug, &b.slug);
}

/// Every role (MFA and password-only sessions) against every route of its own tenant: a role without the
/// permission, or a privileged permission without MFA, must never get a 2xx.
#[tokio::test]
async fn rt_role_matrix_privilege_escalation() {
    let c = rctx().await;
    let s = seed(&c, "R").await;
    let mut toks = Vec::new();
    for role in ["viewer", "analyst", "auditor", "reviewer", "tenant_admin"] {
        let (_, email) = c.user(s.t, role).await;
        let plain = c.token(&s.slug, &email).await;
        let (_, email2) = c.user(s.t, role).await;
        let mfa = c.mfa_token(&s.slug, &email2).await;
        toks.push((role, false, plain));
        toks.push((role, true, mfa));
    }
    let mut bad = Vec::new();
    for (m, tmpl, body, perm) in routes() {
        for (role, mfa, tok) in &toks {
            let granted = perm_granted(role, perm);
            let privileged = matches!(perm, "UserManage" | "IncidentManage" | "ReviewDecide");
            let allowed = granted && (!privileged || *mfa);
            if allowed {
                continue; // only the refusal side is asserted here
            }
            let path = fill(tmpl, &s);
            let body = serde_json::from_str::<Value>(&fill(&body.to_string(), &s)).unwrap();
            let r = send(&c, &m, &path, tok, &body).await;
            rt(
                &format!("escalation {role}{} {m} {tmpl}", if *mfa { "+mfa" } else { "" }),
                "403",
                r.status.as_u16(),
            );
            if r.status.is_success() {
                bad.push(format!("{role} mfa={mfa} {m} {tmpl} -> {}", r.status));
            }
        }
    }
    // Self-promotion through PATCH /users/{self}.
    let (vid, vemail) = c.user(s.t, "analyst").await;
    let vt = c.mfa_token(&s.slug, &vemail).await;
    let r = c
        .call(
            Method::PATCH,
            &format!("/api/v1/users/{vid}"),
            Some(&vt),
            Some(json!({"role":"tenant_admin"})),
        )
        .await;
    rt("analyst PATCH own role to tenant_admin", "403", r.status.as_u16());
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(bad.is_empty(), "privilege escalation: {bad:#?}");
}

/// Malformed / hostile ids in path parameters.
#[tokio::test]
async fn rt_idor_malformed_ids_and_error_leakage() {
    let c = rctx().await;
    let (_, _, _, tok) = c.admin().await;
    let probes = [
        "/api/v1/artifacts/1%20OR%201=1",
        "/api/v1/artifacts/..%2F..%2Fusers",
        "/api/v1/claims/00000000-0000-0000-0000-000000000000",
        "/api/v1/decisions/'%3BDROP%20TABLE%20users--",
        "/api/v1/evidence/part/%00",
        "/api/v1/compare/part/a%00b",
        "/api/v1/compare/part/%27%20OR%20%271%27=%271",
        "/api/v1/users/ffffffff-ffff-ffff-ffff-ffffffffffff/sessions",
        "/api/v1/security/incidents/x/actions/y/z",
        "/api/v1/nope",
    ];
    let leak_words = [
        "sqlx",
        "postgres",
        "panicked",
        "SELECT",
        "syntax error",
        ".rs:",
        "constraint",
        "relation ",
    ];
    for p in probes {
        let r = c.get(p, &tok).await;
        let txt = r.body.to_string();
        let leaked = leak_words.iter().find(|w| txt.contains(*w));
        rt(
            &format!("GET {p}"),
            "4xx, no internals",
            format!("{} leak={leaked:?}", r.status.as_u16()),
        );
        assert!(
            r.status.is_client_error() || r.status == StatusCode::OK,
            "{p}: {}",
            r.status
        );
        assert!(leaked.is_none(), "{p}: {txt}");
    }
    // NUL bytes in JSON strings that reach PostgreSQL text columns.
    for body in [
        json!({"subject_type":"part","subject_key":"K\u{0}1","predicate":"mfg.material","value_text":"x"}),
        json!({"subject_type":"part","subject_key":"K1","predicate":"mfg.material","value_text":"x\u{0}y"}),
    ] {
        let r = c.post("/api/v1/claims", Some(&tok), body).await;
        rt("POST /claims with NUL byte in a string", "4xx", r.status.as_u16());
        assert!(r.status.is_client_error(), "{}", r.status);
    }
    // Malformed JSON bodies.
    for raw in [
        "{",
        "{\"tenant\": 1}",
        "[]",
        "{\"tenant\":\"a\\u0000b\",\"email\":\"b\",\"password\":\"c\"}",
        "{\"tenant\":\"a\",\"email\":\"b\\u0000@x\",\"password\":\"c\"}",
        "{\"tenant\":\"a\",\"email\":\"b\",\"password\":\"c\",\"x\":\"\\ud800\"}",
        &format!("{{\"tenant\":\"{}\"}}", "a".repeat(70_000)),
    ] {
        let req = Request::post("/api/v1/auth/login")
            .header("content-type", "application/json")
            .body(Body::from(raw.to_owned()))
            .unwrap();
        let r = c.raw(req).await;
        let txt = r.body.to_string();
        let leaked = leak_words.iter().find(|w| txt.contains(*w));
        rt(
            &format!("login malformed body {:.30}", raw.replace('|', "/")),
            "4xx, no internals",
            format!("{} leak={leaked:?}", r.status.as_u16()),
        );
        assert!(r.status.is_client_error());
        assert!(leaked.is_none(), "{txt}");
    }
}

/// Session fixation/reuse, refresh replay, revoked-token use, token-kind confusion, token forgery.
#[tokio::test]
async fn rt_session_and_token_attacks() {
    let c = rctx().await;
    let (t, slug) = c.tenant().await;
    let (uid, email) = c.user(t, "analyst").await;
    let l1 = c.login(&slug, &email, PW, None).await;
    let l2 = c.login(&slug, &email, PW, None).await;
    rt(
        "fixation: two logins share tokens",
        "distinct tokens",
        l1.body["access_token"] != l2.body["access_token"],
    );
    assert_ne!(l1.body["access_token"], l2.body["access_token"]);
    let acc = l1.body["access_token"].as_str().unwrap().to_owned();
    let rfr = l1.body["refresh_token"].as_str().unwrap().to_owned();

    // Kind confusion.
    let r = c.get("/api/v1/auth/me", &rfr).await;
    rt("refresh token used as bearer access token", "401", r.status.as_u16());
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = c
        .post("/api/v1/auth/refresh", None, json!({"refresh_token": acc}))
        .await;
    rt("access token used as refresh token", "401", r.status.as_u16());
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    // Forged / tampered tokens.
    let (t2, _) = c.tenant().await;
    let swapped = acc.replacen(&t.simple().to_string(), &t2.simple().to_string(), 1);
    for (name, tok) in [
        ("tampered secret char", {
            let mut b = acc.clone().into_bytes();
            let i = b.len() - 10;
            b[i] = if b[i] == b'A' { b'B' } else { b'A' };
            String::from_utf8(b).unwrap()
        }),
        ("tenant swapped in token", swapped),
        ("empty", String::new()),
        ("garbage", "kz_at_..".into()),
    ] {
        let r = c.get("/api/v1/auth/me", &tok).await;
        rt(&format!("forged access token: {name}"), "401", r.status.as_u16());
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{name}");
    }

    // Refresh replay: first use rotates, second use kills the family (including the new tokens).
    let r1 = c
        .post("/api/v1/auth/refresh", None, json!({"refresh_token": rfr}))
        .await;
    assert_eq!(r1.status, StatusCode::OK);
    let acc2 = r1.body["access_token"].as_str().unwrap().to_owned();
    let rfr2 = r1.body["refresh_token"].as_str().unwrap().to_owned();
    let old = c.get("/api/v1/auth/me", &acc).await;
    rt("old access token after rotation", "401", old.status.as_u16());
    assert_eq!(old.status, StatusCode::UNAUTHORIZED);
    let r2 = c
        .post("/api/v1/auth/refresh", None, json!({"refresh_token": rfr}))
        .await;
    rt("refresh token replay", "401 + session revoked", r2.status.as_u16());
    assert_eq!(r2.status, StatusCode::UNAUTHORIZED);
    let after = c.get("/api/v1/auth/me", &acc2).await;
    let after_r = c
        .post("/api/v1/auth/refresh", None, json!({"refresh_token": rfr2}))
        .await;
    rt(
        "rotated tokens after replay detected",
        "401/401",
        format!("{}/{}", after.status.as_u16(), after_r.status.as_u16()),
    );
    assert_eq!(after.status, StatusCode::UNAUTHORIZED);
    assert_eq!(after_r.status, StatusCode::UNAUTHORIZED);

    // Concurrent double-spend of one refresh token: at most one succeeds.
    let l = c.login(&slug, &email, PW, None).await;
    let rf = l.body["refresh_token"].as_str().unwrap().to_owned();
    let (x, y) = tokio::join!(
        c.post("/api/v1/auth/refresh", None, json!({"refresh_token": rf})),
        c.post("/api/v1/auth/refresh", None, json!({"refresh_token": rf}))
    );
    let wins = [x.status, y.status].iter().filter(|s| s.is_success()).count();
    rt("concurrent refresh double-spend", "<=1 success", wins);
    assert!(wins <= 1);

    // Logout, then reuse.
    let l = c.login(&slug, &email, PW, None).await;
    let (a3, r3) = (
        l.body["access_token"].as_str().unwrap().to_owned(),
        l.body["refresh_token"].as_str().unwrap().to_owned(),
    );
    c.post("/api/v1/auth/logout", Some(&a3), json!({})).await;
    let a = c.get("/api/v1/auth/me", &a3).await;
    let b = c.post("/api/v1/auth/refresh", None, json!({"refresh_token": r3})).await;
    rt(
        "access+refresh after logout",
        "401/401",
        format!("{}/{}", a.status.as_u16(), b.status.as_u16()),
    );
    assert_eq!(a.status, StatusCode::UNAUTHORIZED);
    assert_eq!(b.status, StatusCode::UNAUTHORIZED);

    // Admin revokes sessions / disables user: revoked tokens refused.
    let (_, _, _, admin) = {
        let (aid, aemail) = c.user(t, "tenant_admin").await;
        let tok = c.mfa_token(&slug, &aemail).await;
        (t, slug.clone(), aid, tok)
    };
    let l = c.login(&slug, &email, PW, None).await;
    let (a4, r4) = (
        l.body["access_token"].as_str().unwrap().to_owned(),
        l.body["refresh_token"].as_str().unwrap().to_owned(),
    );
    let rv = c
        .post(&format!("/api/v1/users/{uid}/revoke-sessions"), Some(&admin), json!({}))
        .await;
    assert!(rv.status.is_success(), "{:?}", rv.body);
    let a = c.get("/api/v1/auth/me", &a4).await;
    let b = c.post("/api/v1/auth/refresh", None, json!({"refresh_token": r4})).await;
    rt(
        "tokens after admin revoke-sessions",
        "401/401",
        format!("{}/{}", a.status.as_u16(), b.status.as_u16()),
    );
    assert_eq!(a.status, StatusCode::UNAUTHORIZED);
    assert_eq!(b.status, StatusCode::UNAUTHORIZED);

    // Suspended tenant: neither access nor refresh may keep working.
    let l = c.login(&slug, &email, PW, None).await;
    let r5 = l.body["refresh_token"].as_str().unwrap().to_owned();
    sqlx::query("UPDATE tenants SET status = 'suspended' WHERE id = $1")
        .bind(t)
        .execute(&c.owner)
        .await
        .unwrap();
    let b = c.post("/api/v1/auth/refresh", None, json!({"refresh_token": r5})).await;
    rt("refresh token of a suspended tenant", "401", b.status.as_u16());
    assert_eq!(
        b.status,
        StatusCode::UNAUTHORIZED,
        "refresh issued tokens for a suspended tenant"
    );
}

/// TOTP brute force against a known password: lockout after the threshold, replay of a used code refused.
#[tokio::test]
async fn rt_totp_bruteforce_lockout_and_replay() {
    let c = rctx().await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "analyst").await;
    let tok = c.token(&slug, &email).await;
    let secret = c.enroll_totp(&tok).await;
    let good = kizuna_api::crypto::totp_now(&secret, now() + 30);
    let ok = c.login(&slug, &email, PW, Some(good.clone())).await;
    assert_eq!(ok.status, StatusCode::OK);
    let replay = c.login(&slug, &email, PW, Some(good)).await;
    rt("TOTP code replay", "401", replay.status.as_u16());
    assert_eq!(replay.status, StatusCode::UNAUTHORIZED);
    let mut statuses = Vec::new();
    for i in 0..8 {
        let r = c.login(&slug, &email, PW, Some(format!("{:06}", 123_000 + i))).await;
        statuses.push(r.status.as_u16());
    }
    rt(
        "8 wrong TOTP codes with right password",
        "401 then lock",
        format!("{statuses:?}"),
    );
    let next = kizuna_api::crypto::totp_now(&secret, now() + 60);
    let r = c.login(&slug, &email, PW, Some(next)).await;
    rt("correct TOTP after lockout", "401/429 (locked)", r.status.as_u16());
    assert!(matches!(
        r.status,
        StatusCode::UNAUTHORIZED | StatusCode::TOO_MANY_REQUESTS
    ));
    let kinds = c.security_kinds(t).await;
    assert!(kinds.iter().any(|k| k.ends_with("account_locked")), "{kinds:?}");
    // Non-numeric / oversized codes do not error out.
    let (_, e2) = c.user(t, "analyst").await;
    let tk2 = c.token(&slug, &e2).await;
    c.enroll_totp(&tk2).await;
    for code in ["", "abcdef", "1234567", &"9".repeat(5000), "١٢٣٤٥٦"] {
        let r = c.login(&slug, &e2, PW, Some(code.to_owned())).await;
        assert!(r.status.is_client_error(), "{code:.10}: {}", r.status);
    }
    rt(
        "malformed TOTP codes (empty, alpha, 7 digits, 5000 digits, Arabic digits)",
        "4xx",
        "4xx",
    );
}

/// Rate-limit bypass: spoofed X-Forwarded-For without trusted proxies, and many accounts from one IP.
#[tokio::test]
async fn rt_rate_limit_bypass() {
    let c = ctx().await; // default limits: 20 logins/IP/min
    let (t, slug) = c.tenant().await;
    let mut limited_at = None;
    for i in 0..30u32 {
        let req = Request::post("/api/v1/auth/login")
            .header("content-type", "application/json")
            .header("x-forwarded-for", format!("203.0.113.{}", i + 1))
            .body(Body::from(
                json!({"tenant": slug, "email": format!("u{i}@x.test"), "password": "x"}).to_string(),
            ))
            .unwrap();
        if c.raw(req).await.status == StatusCode::TOO_MANY_REQUESTS {
            limited_at = Some(i);
            break;
        }
    }
    rt(
        "XFF rotation, many accounts, hops=0",
        "429 by ~21st",
        format!("{limited_at:?}"),
    );
    assert!(limited_at.is_some_and(|i| i <= 20));

    // hops=1: only the right-most entry (added by the proxy) counts; spoofed left entries are ignored.
    let c = ctx_with(|cfg| cfg.trusted_proxy_hops = 1).await;
    let mut limited_at = None;
    for i in 0..30u32 {
        let req = Request::post("/api/v1/auth/login")
            .header("content-type", "application/json")
            .header("x-forwarded-for", format!("198.51.100.{}, 192.0.2.77", i + 1))
            .body(Body::from(
                json!({"tenant": slug, "email": "a@x.test", "password": "x"}).to_string(),
            ))
            .unwrap();
        if c.raw(req).await.status == StatusCode::TOO_MANY_REQUESTS {
            limited_at = Some(i);
            break;
        }
    }
    rt(
        "hops=1, spoofed left XFF entries",
        "429 by ~11th (account+IP)",
        format!("{limited_at:?}"),
    );
    assert!(limited_at.is_some_and(|i| i <= 20));
    let _ = t;
}

/// Uploads: polyglots, OOXML with external refs / macros, archive and XML bombs, traversal names, EICAR, size.
#[tokio::test]
async fn rt_upload_attacks() {
    let c = rctx().await;
    let (t, slug, _, tok) = c.admin().await;
    // Zip archives are built with Python's zipfile (no extra Rust dependency).
    let zip = |entries: &[(&str, &[u8])]| -> Vec<u8> { pyzip(entries, false) };
    let ct = br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.as_slice();
    let root_rels = br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.as_slice();
    let wb = br#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="S" sheetId="1" r:id="rId1"/></sheets></workbook>"#.as_slice();
    let wb_rels = br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.as_slice();
    let sheet = br#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Part Number</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>RT-1</t></is></c></row></sheetData></worksheet>"#.as_slice();
    let ext_rels = br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/oleObject" Target="file://203.0.113.9/share/x.xls" TargetMode="External"/></Relationships>"#.as_slice();
    let laughs = br#"<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;"><!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;"><!ENTITY lol4 "&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;"><!ENTITY lol5 "&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;"><!ENTITY lol6 "&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;"><!ENTITY lol7 "&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;"><!ENTITY lol8 "&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;">]><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>&lol8;</t></is></c></row></sheetData></worksheet>"#.as_slice();
    let xxe = br#"<?xml version="1.0"?><!DOCTYPE d [<!ENTITY x SYSTEM "file:///etc/passwd">]><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Part Number</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>&x;</t></is></c></row></sheetData></worksheet>"#.as_slice();
    let xlsx = |sheet_xml: &[u8], wbrels: &[u8], extra: &[(&str, &[u8])]| -> Vec<u8> {
        let mut e: Vec<(&str, &[u8])> = vec![
            ("[Content_Types].xml", ct),
            ("_rels/.rels", root_rels),
            ("xl/workbook.xml", wb),
            ("xl/_rels/workbook.xml.rels", wbrels),
            ("xl/worksheets/sheet1.xml", sheet_xml),
        ];
        e.extend_from_slice(extra);
        zip(&e)
    };
    let big_zero = vec![b' '; 100 * 1024 * 1024];
    let bomb = pyzip(
        &[
            ("[Content_Types].xml", ct),
            ("xl/workbook.xml", wb),
            ("xl/worksheets/sheet1.xml", &big_zero),
        ],
        true,
    );
    drop(big_zero);
    let eicar = br"X5O!P%@AP[4\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*";
    let mut pdf_js = CLEAN_PDF.to_vec();
    pdf_js.extend_from_slice(b"2 0 obj << /S /JavaScript /JS (app.alert(1)) >> endobj\n/OpenAction 2 0 R\n");
    let html_pdf = b"%PDF-1.4\n<html><script>alert(document.cookie)</script></html>\n%%EOF\n".to_vec();
    let mut gif_js = b"GIF89a/*".to_vec();
    gif_js.extend_from_slice(b"*/=alert(1)//");
    let cases: Vec<(&str, &str, Vec<u8>)> = vec![
        ("baseline benign XLSX", "ok.xlsx", xlsx(sheet, wb_rels, &[])),
        ("PDF with /JavaScript /OpenAction", "x.pdf", pdf_js),
        ("PDF/HTML polyglot", "x.pdf", html_pdf),
        ("GIF/JS polyglot", "x.gif", gif_js),
        (
            "XLSX with TargetMode=External oleObject rel (file:// UNC)",
            "x.xlsx",
            xlsx(sheet, ext_rels, &[]),
        ),
        (
            "XLSX with xl/externalLinks part",
            "x.xlsx",
            xlsx(
                sheet,
                wb_rels,
                &[("xl/externalLinks/externalLink1.xml", b"<externalLink/>")],
            ),
        ),
        (
            "XLSM with xl/vbaProject.bin macro",
            "x.xlsx",
            xlsx(sheet, wb_rels, &[("xl/vbaProject.bin", b"\xd0\xcf\x11\xe0macro")]),
        ),
        (
            "XLSX billion laughs (10^8 expansion)",
            "x.xlsx",
            xlsx(laughs, wb_rels, &[]),
        ),
        ("XLSX XXE file:///etc/passwd", "x.xlsx", xlsx(xxe, wb_rels, &[])),
        ("XLSX zip bomb 100 MB deflated", "x.xlsx", bomb),
        (
            "XLSX with ../ entry",
            "x.xlsx",
            xlsx(sheet, wb_rels, &[("../../../../tmp/kz-rt-pwn", b"x")]),
        ),
        ("EICAR test string", "eicar.txt", eicar.to_vec()),
        ("EICAR inside CSV", "e.csv", [b"a,b\n1,".as_slice(), eicar].concat()),
        (
            "ELF executable",
            "x.pdf",
            b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0".to_vec(),
        ),
        ("traversal filename", "../../../etc/passwd", b"a,b\n1,2\n".to_vec()),
        (
            "CRLF/quote filename",
            "a\"\r\nX-Injected: 1\r\n.csv",
            b"a,b\n1,2\n".to_vec(),
        ),
    ];
    let mut bad = Vec::new();
    for (name, fname, data) in cases {
        // A fresh uploader per case: repeated rejections restrict a user's uploads (incident response), which
        // would otherwise mask later cases.
        let (_, ue) = c.user(t, "analyst").await;
        let utok = c.token(&slug, &ue).await;
        let t0 = Instant::now();
        let r = c.upload(&utok, fname, &data, Some("other")).await;
        let status = r.body["artifact"]["status"].as_str().unwrap_or("-").to_owned();
        let disp = r.body["artifact"]["display_name"].as_str().unwrap_or("-").to_owned();
        let actual = format!(
            "{} status={status} name={disp:?} {}ms",
            r.status.as_u16(),
            t0.elapsed().as_millis()
        );
        rt(
            &format!("upload {name}"),
            "reject/quarantine; safe name",
            actual.replace('|', "/"),
        );
        if (name.starts_with("ELF") || name.contains("macro") || name.contains("bomb") || name.contains("External"))
            && status == "accepted"
        {
            bad.push(format!("{name} accepted"));
        }
        if disp.contains('/') || disp.contains('\r') || disp.contains('\n') || disp.contains('"') {
            bad.push(format!("{name}: unsafe display name {disp:?}"));
        }
        // Accepted spreadsheets go through the sandboxed worker: entity expansion / XXE must not hang or leak.
        if status == "accepted" && fname.ends_with(".xlsx") {
            let id = r.body["artifact"]["id"].as_str().unwrap();
            let t1 = Instant::now();
            let x = c
                .post(&format!("/api/v1/artifacts/{id}/extract"), Some(&tok), json!({}))
                .await;
            let txt = x.body.to_string();
            rt(
                &format!("extract {name}"),
                "bounded, no file contents",
                format!(
                    "{} {}ms passwd_leak={}",
                    x.status.as_u16(),
                    t1.elapsed().as_millis(),
                    txt.contains("root:x:0")
                ),
            );
            let claims = c.get("/api/v1/claims", &tok).await.body.to_string();
            if txt.contains("root:x:0") || claims.contains("root:x:0") || t1.elapsed().as_secs() > 55 {
                bad.push(format!("{name}: extraction leak/hang"));
            }
        }
        // Content download of anything stored never renders inline as active content.
        if let Some(id) = r.body["artifact"]["id"].as_str() {
            let d = c.get(&format!("/api/v1/artifacts/{id}/content"), &tok).await;
            let cdisp = d
                .headers
                .get("content-disposition")
                .map(|v| v.to_str().unwrap_or("?").to_owned());
            let ctype = d
                .headers
                .get("content-type")
                .map(|v| v.to_str().unwrap_or("?").to_owned());
            if d.status.is_success()
                && (!cdisp.as_deref().unwrap_or("").starts_with("attachment")
                    || ctype.as_deref().unwrap_or("").contains("html"))
            {
                bad.push(format!("{name}: download headers {cdisp:?} {ctype:?}"));
            }
        }
    }
    assert!(!std::path::Path::new("/tmp/kz-rt-pwn").exists());
    // Oversize body.
    let huge = vec![b'a'; c.st.cfg.max_upload_bytes + 1];
    let (_, ue) = c.user(t, "analyst").await;
    let utok = c.token(&slug, &ue).await;
    let r = c.upload(&utok, "big.csv", &huge, Some("other")).await;
    rt("upload max_upload_bytes+1", "413", r.status.as_u16());
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    // Extraction of a quarantined artifact is refused.
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Audit tamper and RLS bypass attempts with the runtime (app) role directly in SQL.
#[tokio::test]
async fn rt_audit_tamper_and_rls_bypass_as_app_role() {
    let c = rctx().await;
    let (t, _slug, _uid, tok) = c.admin().await;
    let (t2, _) = c.tenant().await;
    let app = runtime_pool().await;
    let try_sql = |sql: &'static str, tenant: Option<Uuid>| {
        let app = app.clone();
        async move {
            let mut tx = app.begin().await.unwrap();
            if let Some(t) = tenant {
                kizuna_api::db::set_tenant(&mut tx, t).await.unwrap();
            }
            let r = sqlx::query(sql).execute(&mut *tx).await;
            let out = match &r {
                Ok(x) => format!("OK rows={}", x.rows_affected()),
                Err(e) => format!("ERR {}", e.to_string().chars().take(60).collect::<String>()),
            };
            let _ = tx.rollback().await;
            out
        }
    };
    let mut bad = Vec::new();
    for (sql, tenant) in [
        ("UPDATE audit_events SET details = '{}'::jsonb", Some(t)),
        ("DELETE FROM audit_events", Some(t)),
        ("TRUNCATE audit_events", Some(t)),
        ("UPDATE security_events SET kind = 'x'", Some(t)),
        ("DELETE FROM security_events", Some(t)),
        ("ALTER TABLE audit_events DISABLE TRIGGER ALL", Some(t)),
        ("ALTER TABLE users DISABLE ROW LEVEL SECURITY", Some(t)),
        ("SET session_replication_role = replica", None),
        ("SET row_security = off", None),
        ("DROP POLICY tenant_isolation ON users", None),
        ("CREATE TABLE rt_pwn (x int)", None),
        ("CREATE FUNCTION rt_f() RETURNS int LANGUAGE sql AS 'select 1'", None),
        ("UPDATE users SET role = 'tenant_admin'", None),
        ("DELETE FROM sessions", None),
        ("UPDATE tenants SET status = 'active'", None),
        (
            "INSERT INTO tenants (id, slug, name) VALUES (gen_random_uuid(), 'rt-evil', 'x')",
            None,
        ),
    ] {
        let out = try_sql(sql, tenant).await;
        rt(&format!("app role SQL: {sql}"), "ERR or 0 rows", &out);
        let rows_ok = out == "OK rows=0" || out.starts_with("ERR");
        let set_ok = sql.starts_with("SET row_security") && out.starts_with("OK"); // allowed but must not bypass
        if !rows_ok && !set_ok {
            bad.push(format!("{sql}: {out}"));
        }
    }
    // row_security=off must error on RLS tables, not silently bypass.
    {
        let mut tx = app.begin().await.unwrap();
        sqlx::query("SET LOCAL row_security = off")
            .execute(&mut *tx)
            .await
            .unwrap();
        let r = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users")
            .fetch_one(&mut *tx)
            .await;
        rt(
            "app role SELECT users with row_security=off",
            "ERR",
            format!("{:?}", r.as_ref().map_err(|_| "ERR")),
        );
        if r.is_ok_and(|n| n > 0) {
            bad.push("row_security=off bypassed RLS".into());
        }
    }
    // Without a tenant set, nothing is visible; with tenant t, nothing of t2.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&app)
        .await
        .unwrap();
    rt("app role SELECT users with no tenant", "0", n);
    if n != 0 {
        bad.push(format!("no-tenant users visible: {n}"));
    }
    let mut tx = kizuna_api::db::tenant_tx(&app, t).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE tenant_id <> $1")
        .bind(t)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    // INSERT a row for another tenant while tenant t is set (WITH CHECK).
    let ins = sqlx::query("INSERT INTO sessions (id, tenant_id, user_id, access_hash, access_expires_at, expires_at, mfa_verified, client_ip) VALUES (gen_random_uuid(), $1, gen_random_uuid(), '\\x00', now(), now(), true, 'x')")
        .bind(t2)
        .execute(&mut *tx)
        .await;
    rt(
        "app role (tenant t) INSERT session for tenant t2",
        "ERR",
        format!("{:?}", ins.as_ref().map(|_| "OK").map_err(|_| "ERR")),
    );
    if ins.is_ok() {
        bad.push("cross-tenant INSERT".into());
    }
    drop(tx);
    rt("app role (tenant t) SELECT other tenants' audit", "0", n);
    if n != 0 {
        bad.push("cross-tenant audit visible".into());
    }
    // Forged audit row appended directly: verification must notice.
    let mut tx = kizuna_api::db::tenant_tx(&app, t).await.unwrap();
    let forged = sqlx::query(
        "INSERT INTO audit_events (tenant_id, seq, actor_kind, action, target_type, details, prev_hash, hash, occurred_at) \
         SELECT tenant_id, seq + 1, 'user', 'auth.login', 'session', '{}'::jsonb, hash, hash, now() \
         FROM audit_events WHERE tenant_id = $1 ORDER BY seq DESC LIMIT 1",
    )
    .bind(t)
    .execute(&mut *tx)
    .await;
    let forged_ok = forged.is_ok();
    if forged_ok {
        tx.commit().await.unwrap();
    } else {
        tx.rollback().await.unwrap();
    }
    let v = c.get("/api/v1/audit/verify", &tok).await;
    rt(
        "app role INSERT forged audit row, then /audit/verify",
        "insert refused or verify fails",
        format!(
            "insert={} http={} intact={} problem={}",
            if forged_ok { "OK" } else { "ERR" },
            v.status.as_u16(),
            v.body["intact"],
            v.body["problem"]
        ),
    );
    if forged_ok && v.body["intact"] != false {
        bad.push(format!("forged audit row not detected: {}", v.body));
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Expensive endpoints: bounded latency and concurrency for report / verify / detection.
#[tokio::test]
async fn rt_expensive_endpoints_bounded() {
    let c = rctx().await;
    let (_, _, _, tok) = c.admin().await;
    for i in 0..40 {
        c.post(
            "/api/v1/claims",
            Some(&tok),
            json!({"subject_type":"part","subject_key":format!("P{}", i % 5),"predicate":"mfg.material","value_text":format!("M{i}")}),
        )
        .await;
    }
    for (m, p, b) in [
        (Method::POST, "/api/v1/reports", Some(json!({"scope":"all"}))),
        (Method::GET, "/api/v1/audit/verify", None),
        (Method::GET, "/api/v1/audit/checkpoints/verify", None),
        (Method::POST, "/api/v1/verification/run", Some(json!({}))),
        (Method::POST, "/api/v1/security/detect", Some(json!({}))),
    ] {
        let t0 = Instant::now();
        let futs: Vec<_> = (0..8).map(|_| c.call(m.clone(), p, Some(&tok), b.clone())).collect();
        let rs = futures_join_all(futs).await;
        let codes: Vec<u16> = rs.iter().map(|r| r.status.as_u16()).collect();
        let ms = t0.elapsed().as_millis();
        rt(
            &format!("8 concurrent {m} {p}"),
            "all answered < 30 s",
            format!("{ms}ms {codes:?}"),
        );
        assert!(ms < 30_000);
        assert!(
            rs.iter().all(|r| r.status != StatusCode::INTERNAL_SERVER_ERROR),
            "{p}: {codes:?}"
        );
    }
}

/// Zip via `python3 -I` (entries as name/bytes; `deflate` = compressed).
fn pyzip(entries: &[(&str, &[u8])], deflate: bool) -> Vec<u8> {
    use std::io::Write;
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("rtzip-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut names = Vec::new();
    for (i, (n, d)) in entries.iter().enumerate() {
        std::fs::File::create(dir.join(i.to_string()))
            .unwrap()
            .write_all(d)
            .unwrap();
        names.push(n.to_string());
    }
    let out = dir.join("out.zip");
    let script = "import sys,zipfile,json\nd,o,c,names=sys.argv[1],sys.argv[2],sys.argv[3]=='1',json.loads(sys.argv[4])\n\
        z=zipfile.ZipFile(o,'w',zipfile.ZIP_DEFLATED if c else zipfile.ZIP_STORED)\n\
        [z.writestr(zipfile.ZipInfo(n),open(d+'/'+str(i),'rb').read(),zipfile.ZIP_DEFLATED if c else zipfile.ZIP_STORED) for i,n in enumerate(names)]\nz.close()\n";
    let st = std::process::Command::new("python3")
        .args([
            "-I",
            "-c",
            script,
            dir.to_str().unwrap(),
            out.to_str().unwrap(),
            if deflate { "1" } else { "0" },
        ])
        .arg(serde_json::to_string(&names).unwrap())
        .status()
        .unwrap();
    assert!(st.success());
    let v = std::fs::read(&out).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    v
}

async fn futures_join_all<F: std::future::Future<Output = Resp>>(v: Vec<F>) -> Vec<Resp> {
    // Poll all concurrently on this task.
    let mut v: Vec<_> = v.into_iter().map(Box::pin).collect();
    let mut out: Vec<Option<Resp>> = (0..v.len()).map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut done = true;
        for (i, f) in v.iter_mut().enumerate() {
            if out[i].is_none() {
                match f.as_mut().poll(cx) {
                    std::task::Poll::Ready(r) => out[i] = Some(r),
                    std::task::Poll::Pending => done = false,
                }
            }
        }
        if done {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
    .await;
    out.into_iter().map(Option::unwrap).collect()
}

/// CORS / CSRF: no CORS grant for foreign origins; cookie-less bearer auth only.
#[tokio::test]
async fn rt_cors_and_csrf() {
    let c = rctx().await;
    let (_, _, _, tok) = c.admin().await;
    let req = Request::builder()
        .method(Method::OPTIONS)
        .uri("/api/v1/users")
        .header("origin", "https://evil.example")
        .header("access-control-request-method", "POST")
        .body(Body::empty())
        .unwrap();
    let r = c.raw(req).await;
    let acao = r.headers.get("access-control-allow-origin").cloned();
    rt(
        "CORS preflight from evil origin",
        "no ACAO",
        format!("{} acao={acao:?}", r.status.as_u16()),
    );
    assert!(acao.is_none());
    let req = Request::post("/api/v1/users")
        .header("origin", "https://evil.example")
        .header("content-type", "text/plain")
        .header("cookie", format!("token={tok}"))
        .body(Body::from(
            json!({"email":"x@x.test","display_name":"x","role":"viewer","password":"x"}).to_string(),
        ))
        .unwrap();
    let r = c.raw(req).await;
    rt("CSRF simple POST text/plain with cookie only", "401", r.status.as_u16());
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let login = c.login("nope", "x@x.test", "x", None).await;
    assert!(login.headers.get("set-cookie").is_none());
    rt("login sets cookies", "none", "none");
}

/// WebAuthn challenge replay / cross-user / cross-tenant and recovery-code reuse.
#[tokio::test]
async fn rt_webauthn_and_recovery() {
    use webauthn_authenticator_rs::WebauthnAuthenticator;
    use webauthn_authenticator_rs::softpasskey::SoftPasskey;
    use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse, Url};
    let origin = Url::parse("http://localhost:8080").unwrap();
    let c = rctx().await;
    let (t, slug) = c.tenant().await;
    let (t2, slug2) = c.tenant().await;
    let mut keys = Vec::new();
    for _ in 0..2 {
        let (_, email) = c.user(t, "analyst").await;
        let tok = c.token(&slug, &email).await;
        let mut key = SoftPasskey::new(true);
        let r = c
            .post(
                "/api/v1/auth/mfa/webauthn/register/start",
                Some(&tok),
                json!({"password": PW}),
            )
            .await;
        let ccr: CreationChallengeResponse = serde_json::from_value(r.body["public_key"].clone()).unwrap();
        let cred = key.do_registration(origin.clone(), ccr).unwrap();
        let f = c
            .post(
                "/api/v1/auth/mfa/webauthn/register/finish",
                Some(&tok),
                json!({"challenge_id": r.body["challenge_id"], "credential": cred, "label": "k"}),
            )
            .await;
        assert_eq!(f.status, StatusCode::OK, "{:?}", f.body);
        let codes: Vec<String> = serde_json::from_value(f.body["recovery_codes"].clone()).unwrap();
        keys.push((email, key, codes));
    }
    let start = |email: String, slug: String| {
        let c = &c;
        async move {
            c.post(
                "/api/v1/auth/webauthn/login/start",
                None,
                json!({"tenant": slug, "email": email, "password": PW}),
            )
            .await
        }
    };
    // Replay of a used assertion / challenge.
    let s = start(keys[0].0.clone(), slug.clone()).await;
    let rcr: RequestChallengeResponse = serde_json::from_value(s.body["public_key"].clone()).unwrap();
    let assertion = keys[0].1.do_authentication(origin.clone(), rcr).unwrap();
    let body = json!({"tenant": slug, "challenge_id": s.body["challenge_id"], "credential": assertion});
    let ok = c.post("/api/v1/auth/webauthn/login/finish", None, body.clone()).await;
    assert_eq!(ok.status, StatusCode::OK);
    let replay = c.post("/api/v1/auth/webauthn/login/finish", None, body).await;
    rt("WebAuthn assertion + challenge replay", "401", replay.status.as_u16());
    assert_eq!(replay.status, StatusCode::UNAUTHORIZED);
    // Old assertion against a fresh challenge.
    let s2 = start(keys[0].0.clone(), slug.clone()).await;
    let r = c
        .post(
            "/api/v1/auth/webauthn/login/finish",
            None,
            json!({"tenant": slug, "challenge_id": s2.body["challenge_id"], "credential": assertion}),
        )
        .await;
    rt("old WebAuthn assertion on a new challenge", "401", r.status.as_u16());
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // Cross-user: user0's challenge answered with user1's key.
    let s3 = start(keys[0].0.clone(), slug.clone()).await;
    let rcr: RequestChallengeResponse = serde_json::from_value(s3.body["public_key"].clone()).unwrap();
    let other = keys[1].1.do_authentication(origin.clone(), rcr);
    let st = match other {
        Ok(a) => {
            c.post(
                "/api/v1/auth/webauthn/login/finish",
                None,
                json!({"tenant": slug, "challenge_id": s3.body["challenge_id"], "credential": a}),
            )
            .await
            .status
        }
        Err(_) => StatusCode::UNAUTHORIZED, // authenticator refused (credential not allowed)
    };
    rt("WebAuthn cross-user (user0 challenge, user1 key)", "401", st.as_u16());
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // Cross-tenant: challenge from tenant t presented under tenant t2's slug.
    let s4 = start(keys[1].0.clone(), slug.clone()).await;
    let rcr: RequestChallengeResponse = serde_json::from_value(s4.body["public_key"].clone()).unwrap();
    let a4 = keys[1].1.do_authentication(origin.clone(), rcr).unwrap();
    let r = c
        .post(
            "/api/v1/auth/webauthn/login/finish",
            None,
            json!({"tenant": slug2, "challenge_id": s4.body["challenge_id"], "credential": a4}),
        )
        .await;
    rt(
        "WebAuthn challenge presented under another tenant slug",
        "401",
        r.status.as_u16(),
    );
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // Password login is refused once a key is enrolled (no downgrade).
    let r = c.login(&slug, &keys[0].0, PW, None).await;
    rt(
        "password-only login for key-protected user",
        "401 webauthn_required",
        r.status.as_u16(),
    );
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // Recovery-code reuse and cross-user use.
    let code = keys[0].2[0].clone();
    let rec = |email: String, code: String| {
        let c = &c;
        let slug = slug.clone();
        async move {
            c.post(
                "/api/v1/auth/webauthn/recover",
                None,
                json!({"tenant": slug, "email": email, "password": PW, "recovery_code": code}),
            )
            .await
        }
    };
    let x = rec(keys[1].0.clone(), code.clone()).await;
    rt("user0 recovery code used for user1", "401", x.status.as_u16());
    assert_eq!(x.status, StatusCode::UNAUTHORIZED);
    let ok = rec(keys[0].0.clone(), code.clone()).await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.body);
    assert_eq!(ok.body["mfa_verified"], false);
    rt("recovery session is MFA-verified", "false", &ok.body["mfa_verified"]);
    // Re-enrol a key, then try the used code and an unused code of the old set again.
    let tok = ok.body["access_token"].as_str().unwrap().to_owned();
    let mut k2 = SoftPasskey::new(true);
    let r = c
        .post(
            "/api/v1/auth/mfa/webauthn/register/start",
            Some(&tok),
            json!({"password": PW}),
        )
        .await;
    let ccr: CreationChallengeResponse = serde_json::from_value(r.body["public_key"].clone()).unwrap();
    let cred = k2.do_registration(origin.clone(), ccr).unwrap();
    let f = c
        .post(
            "/api/v1/auth/mfa/webauthn/register/finish",
            Some(&tok),
            json!({"challenge_id": r.body["challenge_id"], "credential": cred, "label": "k2"}),
        )
        .await;
    assert_eq!(f.status, StatusCode::OK, "{:?}", f.body);
    let reuse = rec(keys[0].0.clone(), code).await;
    rt("recovery code reuse after re-enrolment", "401", reuse.status.as_u16());
    assert_eq!(reuse.status, StatusCode::UNAUTHORIZED);
    let old_unused = rec(keys[0].0.clone(), keys[0].2[1].clone()).await;
    rt("unused code from the superseded set", "401", old_unused.status.as_u16());
    assert_eq!(old_unused.status, StatusCode::UNAUTHORIZED);
    let _ = t2;
}

/// Slow-header (slowloris) and idle keep-alive connections are closed by the server's accept loop (F3).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rt_slow_headers_are_cut_off() {
    use std::io::{Read, Write};
    let c = rctx().await;
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let limits = kizuna_api::serve::ServeLimits {
        header_read_timeout: std::time::Duration::from_secs(2),
        max_connections: 16,
        shutdown_grace: std::time::Duration::from_secs(1),
    };
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let srv = tokio::spawn(kizuna_api::serve::serve(l, c.app.clone(), limits, async {
        let _ = stop_rx.await;
    }));
    let probe = move |payload: &'static [u8]| {
        tokio::task::spawn_blocking(move || {
            let mut s = std::net::TcpStream::connect(addr).unwrap();
            s.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
            s.write_all(payload).unwrap();
            let t0 = Instant::now();
            let mut buf = Vec::new();
            let r = s.read_to_end(&mut buf);
            (r.is_ok(), t0.elapsed(), String::from_utf8_lossy(&buf).into_owned())
        })
    };
    let (closed, el, _) = probe(b"GET /healthz HTTP/1.1\r\nHost: x\r\n").await.unwrap();
    rt(
        "in-process slowloris (header timeout 2 s)",
        "closed within ~2 s",
        format!("closed={closed} {}ms", el.as_millis()),
    );
    assert!(closed && el.as_secs() < 6, "connection held {el:?}");
    // A complete request still works and carries the peer address (rate limits key on it).
    let (_, _, out) = probe(
        b"POST /api/v1/auth/login HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: 45\r\nConnection: close\r\n\r\n{\"tenant\":\"x\",\"email\":\"a@b.c\",\"password\":\"p\"}",
    )
    .await
    .unwrap();
    rt(
        "complete request through the accept loop",
        "401 (served)",
        out.lines().next().unwrap_or(""),
    );
    assert!(out.starts_with("HTTP/1.1 401"), "{out}");
    // Idle keep-alive after a response is also closed.
    let (closed, el, out) = probe(b"GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
    rt(
        "in-process idle keep-alive",
        "closed within ~2 s",
        format!("closed={closed} {}ms", el.as_millis()),
    );
    assert!(out.starts_with("HTTP/1.1 200") && closed && el.as_secs() < 6);
    let _ = stop_tx.send(());
    srv.await.unwrap().unwrap();
}

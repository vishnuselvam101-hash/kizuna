#![allow(clippy::unwrap_used)]
//! Evidence debt ledger (I10, `GET /evidence-debt*`) end to end on real PostgreSQL: missing, unconfirmed,
//! re-requested, superseded, rejected and expired evidence per decision, part and supplier; age and the decisions
//! each piece of debt blocks; "as known on" past dates; read-only (no decision is changed); tenant isolation.
mod common;

use std::time::Duration;

use axum::http::{Method, StatusCode};
use common::*;
use kizuna_api::db;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

struct T {
    c: Ctx,
    t: Uuid,
    /// Enters claims and decisions.
    tok: String,
    /// Reviews claims (nobody reviews their own).
    tok2: String,
    viewer: String,
}

async fn tenant(c: Ctx) -> T {
    let (t, slug) = c.tenant().await;
    let (_, e1) = c.user(t, "reviewer").await;
    let (_, e2) = c.user(t, "reviewer").await;
    let (_, e3) = c.user(t, "viewer").await;
    let tok = c.mfa_token(&slug, &e1).await;
    let tok2 = c.mfa_token(&slug, &e2).await;
    let viewer = c.token(&slug, &e3).await;
    T {
        c,
        t,
        tok,
        tok2,
        viewer,
    }
}

async fn claim(s: &T, part: &str, predicate: &str, value: &str) -> String {
    let r =
        s.c.post(
            "/api/v1/claims",
            Some(&s.tok),
            json!({ "subject_type": "part", "subject_key": part, "predicate": predicate, "value_text": value }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body["id"].as_str().unwrap().to_owned()
}

async fn review(s: &T, claim: &str, decision: &str) {
    let r =
        s.c.post(
            &format!("/api/v1/claims/{claim}/review"),
            Some(&s.tok2),
            json!({ "decision": decision, "comment": "checked against the paper original" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
}

async fn decide(s: &T, part: &str, evidence: &[&str]) -> String {
    let r = s
        .c
        .post(
            "/api/v1/decisions",
            Some(&s.tok),
            json!({
                "kind": "part_release", "title": format!("Release {part}"), "subject_type": "part", "subject_key": part,
                "outcome": "approved", "rationale": "evidence checked",
                "evidence": evidence.iter().map(|c| json!({ "claim_id": c, "materiality": { "type": "any_change" } })).collect::<Vec<_>>(),
            }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body["id"].as_str().unwrap().to_owned()
}

/// The five claims a release needs, all confirmed by the second reviewer. Returns them in requirement order.
async fn full_release(s: &T, part: &str, supplier: &str) -> (String, Vec<String>) {
    let mut ids = Vec::new();
    for (p, v) in [
        ("mfg.revision", "C"),
        ("mfg.material", "SUS304"),
        ("qa.material", "SUS304"),
        ("qa.result", "pass"),
        ("po.supplier", supplier),
    ] {
        let id = claim(s, part, p, v).await;
        review(s, &id, "approve").await;
        ids.push(id);
    }
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    (decide(s, part, &refs).await, ids)
}

fn stamp() -> String {
    OffsetDateTime::now_utc().format(&Rfc3339).unwrap()
}

async fn tick() {
    tokio::time::sleep(Duration::from_millis(30)).await;
}

async fn get(s: &T, path: &str) -> Value {
    let r = s.c.get(path, &s.viewer).await;
    assert_eq!(r.status, StatusCode::OK, "{path}: {:?}", r.body);
    r.body
}

fn row<'a>(body: &'a Value, key: &str) -> &'a Value {
    body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == key)
        .unwrap_or_else(|| panic!("no row {key} in {body}"))
}

fn kinds_of(items: &Value, target_id: &str) -> Vec<String> {
    let mut v: Vec<String> = items["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["target"]["id"] == target_id)
        .map(|r| r["kind"].as_str().unwrap().to_owned())
        .collect();
    v.sort();
    v
}

#[tokio::test]
async fn clear_release_has_no_debt_and_components_appear_as_evidence_weakens() {
    let s = tenant(ctx().await).await;
    let (dec, ids) = full_release(&s, "BRK-1", "ACME").await;

    let l = get(&s, "/api/v1/evidence-debt").await;
    assert_eq!(l["version"], "mfg-debt@1");
    assert_eq!(l["totals"]["decisions"], 1);
    assert_eq!(l["totals"]["with_debt"], 0, "{l}");
    assert_eq!(row(&l, "BRK-1")["totals"]["with_debt"], 0);
    assert!(
        get(&s, "/api/v1/evidence-debt/items").await["rows"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    tick().await;
    let before = stamp();
    tick().await;

    // Reviewer asks for more evidence on the material certificate: pending re-request.
    let r =
        s.c.post(
            &format!("/api/v1/claims/{}/review", ids[2]),
            Some(&s.tok2),
            json!({ "decision": "request_evidence" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    // The drawing revision is corrected: the old claim is superseded.
    let r =
        s.c.post(
            &format!("/api/v1/claims/{}/supersede", ids[0]),
            Some(&s.tok),
            json!({ "subject_type": "part", "subject_key": "BRK-1", "predicate": "mfg.revision", "value_text": "D",
                    "reason": "drawing revised" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    // The inspection result is rejected: debt, and the requirement is now missing.
    review(&s, &ids[3], "reject").await;

    let l = get(&s, "/api/v1/evidence-debt").await;
    let t = &l["totals"];
    assert_eq!(
        (t["with_debt"].as_u64(), t["re_requested"].as_u64()),
        (Some(1), Some(1)),
        "{t}"
    );
    assert_eq!(
        (t["superseded"].as_u64(), t["rejected"].as_u64()),
        (Some(1), Some(1)),
        "{t}"
    );
    assert_eq!(t["missing_required"], 1, "{t}");

    // As known before those reviews, nothing had changed (bitemporal: later history does not rewrite the past).
    let past = get(&s, &format!("/api/v1/evidence-debt?as_of={}", urlencode(&before))).await;
    assert_eq!(past["totals"]["with_debt"], 0, "{past}");

    // Per decision: each component names its evidence.
    let d = get(&s, &format!("/api/v1/evidence-debt/decisions/{dec}")).await;
    assert_eq!(d["accepted"], true);
    let comp = &d["components"];
    assert_eq!(comp["re_requested"][0]["id"], ids[2].as_str());
    assert_eq!(comp["superseded"][0]["id"], ids[0].as_str());
    assert!(comp["superseded"][0]["superseded_by"].is_string());
    assert_eq!(comp["rejected"][0]["id"], ids[3].as_str());
    assert_eq!(comp["missing_required"][0]["name"], "inspection_result");
    let debts = d["debts"].as_array().unwrap();
    assert_eq!(debts.len(), 4, "{debts:?}");
    assert!(debts.iter().all(|x| x["age_days"] == 0 && x["since"].is_string()));

    // Items: one row per piece of debt with the decisions it blocks.
    let items = get(&s, "/api/v1/evidence-debt/items").await;
    assert_eq!(items["total_rows"], 4, "{items}");
    assert_eq!(kinds_of(&items, &ids[2]), vec!["re_requested"]);
    assert_eq!(kinds_of(&items, &ids[0]), vec!["superseded"]);
    let missing = items["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "missing_required")
        .unwrap();
    assert_eq!(missing["target"]["name"], "inspection_result");
    assert_eq!(
        (missing["blocks"].as_u64(), missing["decisions"][0]["id"].as_str()),
        (Some(1), Some(dec.as_str()))
    );
    assert_eq!(missing["parts"], json!(["BRK-1"]));
    assert_eq!(missing["suppliers"], json!(["ACME"]));
    let only = get(&s, "/api/v1/evidence-debt/items?kind=superseded").await;
    assert_eq!(only["total_rows"], 1);

    // Read-only: humans decide. The ledger never revokes or changes a decision.
    let st = s.c.get(&format!("/api/v1/decisions/{dec}"), &s.tok).await.body["decision"]["status"].clone();
    assert_ne!(st, "revoked");
    for _ in 0..2 {
        get(&s, "/api/v1/evidence-debt?group=decision").await;
    }
    assert_eq!(
        s.c.get(&format!("/api/v1/decisions/{dec}"), &s.tok).await.body["decision"]["status"],
        st
    );
}

#[tokio::test]
async fn shared_evidence_blocks_several_decisions_grouped_by_part_and_supplier() {
    let s = tenant(ctx().await).await;
    // Two decisions on one part cite the same unconfirmed (never reviewed) certificate claim.
    let mut ids = Vec::new();
    for (p, v) in [
        ("mfg.revision", "A"),
        ("mfg.material", "S45C"),
        ("qa.result", "pass"),
        ("po.supplier", "Kanto Seiki"),
    ] {
        let id = claim(&s, "SH-9", p, v).await;
        review(&s, &id, "approve").await;
        ids.push(id);
    }
    let cert = claim(&s, "SH-9", "qa.material", "S45C").await;
    let mut refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    refs.push(&cert);
    let d1 = decide(&s, "SH-9", &refs).await;
    let d2 = decide(&s, "SH-9", &refs).await;
    // A third decision on another part from another supplier is missing its supplier identity entirely.
    let mut other = Vec::new();
    for (p, v) in [
        ("mfg.revision", "B"),
        ("mfg.material", "A5052"),
        ("qa.material", "A5052"),
        ("qa.result", "pass"),
    ] {
        let id = claim(&s, "PL-2", p, v).await;
        review(&s, &id, "approve").await;
        other.push(id);
    }
    let r: Vec<&str> = other.iter().map(String::as_str).collect();
    let d3 = decide(&s, "PL-2", &r).await;

    let items = get(&s, "/api/v1/evidence-debt/items?sort=blocks").await;
    let first = &items["rows"][0];
    assert_eq!(
        (first["kind"].as_str(), first["target"]["id"].as_str()),
        (Some("unconfirmed"), Some(cert.as_str()))
    );
    assert_eq!(first["blocks"], 2);
    let mut blocked: Vec<&str> = first["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap())
        .collect();
    blocked.sort_unstable();
    let mut want = vec![d1.as_str(), d2.as_str()];
    want.sort_unstable();
    assert_eq!(blocked, want);
    assert_eq!(first["detail"]["predicate"], "qa.material");
    let miss = items["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "missing_required")
        .unwrap();
    assert_eq!(
        (miss["target"]["name"].as_str(), miss["decisions"][0]["id"].as_str()),
        (Some("supplier_identity"), Some(d3.as_str()))
    );

    let by_part = get(&s, "/api/v1/evidence-debt?group=part").await;
    // Shared claims are counted once per group.
    assert_eq!(row(&by_part, "SH-9")["totals"]["unconfirmed"], 1);
    assert_eq!(row(&by_part, "SH-9")["totals"]["with_debt"], 2);
    assert_eq!(row(&by_part, "PL-2")["totals"]["missing_required"], 1);
    let by_sup = get(&s, "/api/v1/evidence-debt?group=supplier").await;
    assert_eq!(row(&by_sup, "KANTO SEIKI")["totals"]["decisions"], 2, "{by_sup}");
    let filtered = get(&s, "/api/v1/evidence-debt/items?part=PL-2").await;
    assert_eq!(filtered["total_rows"], 1);
    let trend = &row(&by_part, "SH-9")["trend"]["unconfirmed"];
    assert_eq!(trend.as_array().unwrap().len(), 12);

    // Tenant isolation: another tenant sees nothing.
    let other = tenant(ctx().await).await;
    assert_eq!(get(&other, "/api/v1/evidence-debt").await["totals"]["decisions"], 0);
    assert!(
        get(&other, "/api/v1/evidence-debt/items").await["rows"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let r = other
        .c
        .get(&format!("/api/v1/evidence-debt/decisions/{d1}"), &other.viewer)
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn expired_certificate_and_supplier_re_request_are_debt_with_age() {
    let s = tenant(ctx().await).await;
    let (dec, ids) = full_release(&s, "VLV-3", "Tokai").await;
    let mut refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let exp = claim(&s, "VLV-3", "mfg.certificate.expiry", "2020-01-31").await;
    review(&s, &exp, "approve").await;
    refs.push(&exp);
    let dec2 = decide(&s, "VLV-3", &refs).await;

    let l = get(&s, "/api/v1/evidence-debt").await;
    assert_eq!(
        (l["totals"]["expired"].as_u64(), l["totals"]["expiring"].as_u64()),
        (Some(1), Some(1)),
        "{l}"
    );
    let items = get(&s, "/api/v1/evidence-debt/items?kind=expired").await;
    let r = &items["rows"][0];
    assert_eq!(r["target"]["id"], exp.as_str());
    assert_eq!(r["since"], "2020-02-01T00:00:00Z");
    assert!(r["age_days"].as_i64().unwrap() > 365 * 5, "{r}");
    assert_eq!(r["decisions"][0]["id"], dec2.as_str());

    // A supplier re-request covering the first decision is pending debt until a person cancels it.
    let req = Uuid::now_v7();
    let mut tx = db::tenant_tx(&s.c.owner, s.t).await.unwrap();
    let creator: Uuid = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO evidence_requests (id, tenant_id, reference, supplier, engine, decision_ids, documents, requested_count, \
         baseline_count, exact, body_ja, body_en, content_sha256, created_by) \
         VALUES ($1, $2, 'RR-1', 'tokai', 'mfg-resupply@1', $3, '[]', 1, 1, true, 'ja', 'en', $4, $5)",
    )
    .bind(req)
    .bind(s.t)
    .bind(vec![Uuid::parse_str(&dec).unwrap()])
    .bind(vec![7u8; 32])
    .bind(creator)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let d = get(&s, &format!("/api/v1/evidence-debt/decisions/{dec}")).await;
    assert_eq!(d["components"]["re_requested"][0]["type"], "evidence_request");
    assert_eq!(d["components"]["re_requested"][0]["label"], "RR-1");
    let items = get(&s, "/api/v1/evidence-debt/items?kind=re_requested").await;
    assert_eq!(items["rows"][0]["target"]["type"], "evidence_request");
    let r =
        s.c.post(
            &format!("/api/v1/evidence-requests/{req}/cancel"),
            Some(&s.tok2),
            json!({ "comment": "not needed" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    tick().await;
    let d = get(&s, &format!("/api/v1/evidence-debt/decisions/{dec}")).await;
    assert!(d["components"]["re_requested"].as_array().unwrap().is_empty(), "{d}");
}

#[tokio::test]
async fn bad_parameters_and_auth() {
    let s = tenant(ctx().await).await;
    for q in [
        "/api/v1/evidence-debt?group=x",
        "/api/v1/evidence-debt?sort=x",
        "/api/v1/evidence-debt?points=0",
        "/api/v1/evidence-debt?as_of=yesterday",
        "/api/v1/evidence-debt/items?kind=x",
        "/api/v1/evidence-debt/items?sort=x",
    ] {
        assert_eq!(s.c.get(q, &s.viewer).await.status, StatusCode::BAD_REQUEST, "{q}");
    }
    let r = s.c.call(Method::GET, "/api/v1/evidence-debt/items", None, None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r =
        s.c.get(
            &format!("/api/v1/evidence-debt/decisions/{}", Uuid::now_v7()),
            &s.viewer,
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

fn urlencode(s: &str) -> String {
    s.replace(':', "%3A").replace('+', "%2B")
}

#![allow(clippy::unwrap_used)]
//! What-if simulation (`POST /simulations`): the preview equals what really happens when the change is then
//! applied (property test over generated tenants), a simulation writes nothing, tenants are isolated, and a
//! 100k-edge dependency graph is simulated within bounds.
mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use axum::http::StatusCode;
use common::*;
use kizuna_api::db;
use kizuna_api::simulations::SIM_CLAIM;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::{Value, json};
use uuid::Uuid;

struct T {
    c: Ctx,
    t: Uuid,
    /// Enters claims and decisions.
    tok: String,
    /// A second reviewer: rejects claims (nobody reviews their own claims).
    tok2: String,
    uid: Uuid,
}

async fn tenant(c: Ctx) -> T {
    let (t, slug) = c.tenant().await;
    let (uid, e1) = c.user(t, "reviewer").await;
    let (_, e2) = c.user(t, "reviewer").await;
    let tok = c.mfa_token(&slug, &e1).await;
    let tok2 = c.mfa_token(&slug, &e2).await;
    T { c, t, tok, tok2, uid }
}

/// A released document of `kind` (inserted directly: extraction is not under test here).
async fn doc(s: &T, kind: &str, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    let mut tx = db::tenant_tx(&s.c.owner, s.t).await.unwrap();
    sqlx::query(
        "INSERT INTO artifacts (id, tenant_id, sha256, size_bytes, media_type, kind, status, storage_key, display_name, created_by) \
         VALUES ($1, $2, $3, 1, 'application/pdf', $4, 'accepted', 'sim', $5, $6)",
    )
    .bind(id)
    .bind(s.t)
    .bind(kizuna_api::crypto::random_bytes::<32>().to_vec())
    .bind(kind)
    .bind(name)
    .bind(s.uid)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

/// A claim. With a document it is stored as extracted by the rules extractor (so the match engine sees its
/// document kind); without one it is entered by hand through the API.
async fn claim(s: &T, body: Value) -> String {
    if let Some(a) = body["artifact_id"].as_str() {
        let id = Uuid::now_v7();
        let mut tx = db::tenant_tx(&s.c.owner, s.t).await.unwrap();
        sqlx::query(
            "INSERT INTO claims (id, tenant_id, subject_type, subject_key, predicate, value_text, value_num, value_unit, scope, \
             revision, artifact_id, extraction_method, confidence_bps) \
             VALUES ($1, $2, $3, $4, $5, $6, $7::numeric, $8, $9, $10, $11::uuid, 'rules:test', 9000)",
        )
        .bind(id)
        .bind(s.t)
        .bind(body["subject_type"].as_str().unwrap())
        .bind(entity_resolution::safe_normalize(body["subject_key"].as_str().unwrap()))
        .bind(body["predicate"].as_str().unwrap())
        .bind(body["value_text"].as_str())
        .bind(body["value_num"].as_str())
        .bind(body["value_unit"].as_str())
        .bind(if body["scope"].is_object() { body["scope"].clone() } else { json!({}) })
        .bind(body["revision"].as_str())
        .bind(a)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        return id.to_string();
    }
    let r = s.c.post("/api/v1/claims", Some(&s.tok), body).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body["id"].as_str().unwrap().to_owned()
}

async fn decide(s: &T, part: &str, evidence: Vec<(String, Value)>, upstream: Vec<String>) -> Option<String> {
    let r = s
        .c
        .post(
            "/api/v1/decisions",
            Some(&s.tok),
            json!({
                "kind": "po_line_acceptance", "title": format!("Accept {part}"), "subject_type": "part", "subject_key": part,
                "outcome": "approved", "rationale": "evidence checked",
                "evidence": evidence.iter().map(|(c, m)| json!({"claim_id": c, "materiality": m})).collect::<Vec<_>>(),
                "depends_on": upstream.iter().map(|d| json!({"decision_id": d})).collect::<Vec<_>>(),
            }),
        )
        .await;
    (r.status == StatusCode::OK).then(|| r.body["id"].as_str().unwrap().to_owned())
}

async fn simulate(s: &T, tok: &str, change: Value) -> (StatusCode, Value) {
    let r =
        s.c.post(
            "/api/v1/simulations",
            Some(tok),
            json!({ "change": change, "sample": 500 }),
        )
        .await;
    (r.status, r.body)
}

/// Row count of every table, as the owner sees it inside the tenant (RLS applies), plus the audit chain head.
async fn counts(s: &T) -> BTreeMap<String, i64> {
    let mut tx = db::tenant_tx(&s.c.owner, s.t).await.unwrap();
    // Every table holding tenant data (other tests run in parallel, so global tables are not compared).
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.columns WHERE table_schema = 'public' AND column_name = 'tenant_id' \
         AND table_name IN (SELECT tablename FROM pg_tables WHERE schemaname = 'public') ORDER BY 1",
    )
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    let mut out = BTreeMap::new();
    for t in tables {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM \"{t}\" WHERE tenant_id = $1"))
            .bind(s.t)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        out.insert(t, n);
    }
    let head: Option<i64> = sqlx::query_scalar("SELECT last_seq FROM audit_heads WHERE tenant_id = $1")
        .bind(s.t)
        .fetch_optional(&mut *tx)
        .await
        .unwrap();
    out.insert("audit_heads.last_seq".into(), head.unwrap_or(0));
    out
}

async fn statuses(s: &T) -> BTreeMap<String, String> {
    let mut tx = db::tenant_tx(&s.c.owner, s.t).await.unwrap();
    let rows: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, status FROM decisions")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    rows.into_iter().map(|(i, st)| (i.to_string(), st)).collect()
}

type Event = (Uuid, String, bool, String, String, Value);

async fn events(s: &T) -> Vec<Event> {
    let mut tx = db::tenant_tx(&s.c.owner, s.t).await.unwrap();
    sqlx::query_as(
        "SELECT id, decision_id::text, material, cause_kind, reason, path FROM invalidation_events ORDER BY recorded_at, id",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap()
}

async fn engine_state(s: &T) -> Value {
    let mut tx = db::tenant_snapshot_tx(&s.c.st.pool, s.t).await.unwrap();
    kizuna_api::simulations::engine_state(&mut tx).await.unwrap()
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_owned())
        .collect()
}

fn match_key(f: &Value) -> String {
    json!([
        f["purchase_order"],
        f["po_line"],
        f["part"],
        f["kind"],
        f["check"],
        f["certificate_doc"]
    ])
    .to_string()
}

/// `before` plus what the simulation says appears, minus what it says clears.
fn predicted(before: &Value, d: &Value, key: impl Fn(&Value) -> String) -> BTreeSet<String> {
    assert!(d["appear"].as_array().unwrap().len() == d["appear_total"].as_u64().unwrap() as usize);
    let mut k = keys(before);
    for a in d["appear"].as_array().unwrap() {
        assert!(k.insert(key(a)), "appearing entry already present: {a}");
    }
    for c in d["clear"].as_array().unwrap() {
        assert!(k.remove(&key(c)), "clearing entry not present: {c}");
    }
    k
}

/// Simulates `change`, checks nothing was written, applies it for real with `apply` and checks the outcome is
/// exactly what the simulation said. `apply` returns the id of the claim a correction created.
async fn check<F, Fut>(s: &T, change: Value, apply: F) -> Value
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Option<String>>,
{
    let rows0 = counts(s).await;
    let st0 = statuses(s).await;
    let ev0: BTreeSet<Uuid> = events(s).await.into_iter().map(|e| e.0).collect();
    let eng0 = engine_state(s).await;
    let (code, sim) = simulate(s, &s.tok, change).await;
    assert_eq!(code, StatusCode::OK, "{sim:?}");
    assert_eq!(sim["read_only"], true);
    assert_eq!(counts(s).await, rows0, "a simulation must not write anything");
    assert_eq!(sim["decisions"]["bounded"], false);

    eprintln!(
        "whatif check: affected {} material {} match +{}/-{} signals +{}/-{} drift +{}/-{} resupply {}",
        sim["decisions"]["affected_total"],
        sim["decisions"]["material_total"],
        sim["matching"]["appear_total"],
        sim["matching"]["clear_total"],
        sim["integrity"]["signals"]["appear_total"],
        sim["integrity"]["signals"]["clear_total"],
        sim["integrity"]["drift"]["appear_total"],
        sim["integrity"]["drift"]["clear_total"],
        sim["resupply"]["suppliers"].as_array().map_or(0, Vec::len)
    );
    let new_claim = apply().await;

    // Decisions: the predicted status of every decision, and the recorded explanation of each affected one.
    let sample = sim["decisions"]["sample"].as_array().unwrap();
    assert_eq!(
        sample.len(),
        sim["decisions"]["affected_total"].as_u64().unwrap() as usize
    );
    let st1 = statuses(s).await;
    let mut expect = st0.clone();
    for i in sample {
        expect.insert(
            i["decision_id"].as_str().unwrap().into(),
            i["status_after"].as_str().unwrap().into(),
        );
        assert_eq!(
            st0[i["decision_id"].as_str().unwrap()],
            i["status_before"].as_str().unwrap()
        );
    }
    assert_eq!(st1, expect, "decision statuses after the real change");
    let fresh: Vec<Event> = events(s).await.into_iter().filter(|e| !ev0.contains(&e.0)).collect();
    let real_ids: BTreeSet<&str> = fresh.iter().map(|e| e.1.as_str()).collect();
    let sim_ids: BTreeSet<&str> = sample.iter().map(|i| i["decision_id"].as_str().unwrap()).collect();
    assert_eq!(real_ids, sim_ids, "affected decisions");
    let map_path = |p: &Value| -> Value {
        Value::Array(
            p.as_array()
                .unwrap()
                .iter()
                .map(|x| match (&new_claim, x.as_str()) {
                    (Some(n), Some(v)) if v == n => json!(SIM_CLAIM.to_string()),
                    _ => x.clone(),
                })
                .collect(),
        )
    };
    for i in sample {
        let d = i["decision_id"].as_str().unwrap();
        let mine: Vec<&Event> = fresh.iter().filter(|e| e.1 == d).collect();
        assert_eq!(
            i["material"].as_bool().unwrap(),
            mine.iter().any(|e| e.2),
            "materiality of {d}"
        );
        assert!(
            mine.iter().any(|e| e.2 == i["material"].as_bool().unwrap()
                && e.3 == i["cause_kind"].as_str().unwrap()
                && e.4 == i["reason"].as_str().unwrap()
                && map_path(&e.5) == i["path"]),
            "explanation of {d}: simulated {i}, recorded {mine:?}"
        );
    }

    // Match findings, integrity signals and drift: the engines over the applied state.
    let eng1 = engine_state(s).await;
    assert_eq!(
        keys(&eng1["match"]),
        predicted(&eng0["match"], &sim["matching"], match_key),
        "match findings"
    );
    let fp = |v: &Value| v["fingerprint"].as_str().unwrap().to_owned();
    assert_eq!(
        keys(&eng1["signals"]),
        predicted(&eng0["signals"], &sim["integrity"]["signals"], fp),
        "signals"
    );
    assert_eq!(
        keys(&eng1["drift"]),
        predicted(&eng0["drift"], &sim["integrity"]["drift"], fp),
        "drift"
    );

    // The minimal supplier re-request for the decisions the change left potentially affected.
    let mut material: Vec<Uuid> = sample
        .iter()
        .filter(|i| i["material"] == true)
        .map(|i| Uuid::parse_str(i["decision_id"].as_str().unwrap()).unwrap())
        .collect();
    material.sort();
    let real_plan = if material.is_empty() {
        json!({ "engine": sim["resupply"]["engine"], "decisions_considered": 0, "suppliers": [], "no_supplier_action": [] })
    } else {
        let mut tx = db::tenant_tx(&s.c.st.pool, s.t).await.unwrap();
        json!(
            kizuna_api::evidence_requests::compute(&mut tx, &material)
                .await
                .unwrap()
        )
    };
    assert_eq!(sim["resupply"], real_plan, "supplier re-request");
    sim
}

async fn supersede(s: &T, id: &str, body: Value) -> Option<String> {
    let r =
        s.c.post(&format!("/api/v1/claims/{id}/supersede"), Some(&s.tok), body)
            .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    Some(r.body["id"].as_str().unwrap().to_owned())
}

async fn reject(s: &T, ids: &[String]) -> Option<String> {
    for id in ids {
        let r =
            s.c.post(
                &format!("/api/v1/claims/{id}/review"),
                Some(&s.tok2),
                json!({ "decision": "reject", "comment": "wrong document" }),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    }
    None
}

/// A PO line, its drawing and an inspection report for part `p` from `supplier`.
struct Docs {
    po: Uuid,
    drawing: Uuid,
    report: Uuid,
}

async fn docs(s: &T, p: &str) -> Docs {
    Docs {
        po: doc(s, "purchase_order", &format!("PO {p}.pdf")).await,
        drawing: doc(s, "drawing", &format!("Drawing {p}.pdf")).await,
        report: doc(s, "inspection_report", &format!("Inspection {p}.pdf")).await,
    }
}

fn body(part: &str, pred: &str, value: Value, extra: Value) -> Value {
    let mut b = json!({ "subject_type": "part", "subject_key": part, "predicate": pred });
    for (k, v) in value.as_object().unwrap().iter().chain(extra.as_object().unwrap()) {
        b[k] = v.clone();
    }
    b
}

#[tokio::test]
async fn preview_of_a_spec_change_matches_the_applied_change() {
    let s = tenant(ctx().await).await;
    let d = docs(&s, "BRK-1").await;
    let po_scope = json!({"purchase_order": "PO-1", "po_line": "1", "supplier": "ACME"});
    let ordered = claim(
        &s,
        body(
            "BRK-1",
            "po.revision_ordered",
            json!({"value_text": "B"}),
            json!({"artifact_id": d.po, "scope": po_scope}),
        ),
    )
    .await;
    claim(
        &s,
        body(
            "BRK-1",
            "po.supplier",
            json!({"value_text": "ACME"}),
            json!({"artifact_id": d.po, "scope": po_scope}),
        ),
    )
    .await;
    let rev = claim(
        &s,
        body(
            "BRK-1",
            "mfg.revision",
            json!({"value_text": "B"}),
            json!({"artifact_id": d.drawing, "revision": "B"}),
        ),
    )
    .await;
    let len = claim(
        &s,
        body(
            "BRK-1",
            "mfg.dimension.length",
            json!({"value_num": "100", "value_unit": "mm"}),
            json!({"artifact_id": d.drawing, "revision": "B"}),
        ),
    )
    .await;
    claim(
        &s,
        body(
            "BRK-1",
            "mfg.tolerance.length.plus",
            json!({"value_num": "0.1", "value_unit": "mm"}),
            json!({"artifact_id": d.drawing}),
        ),
    )
    .await;
    claim(
        &s,
        body(
            "BRK-1",
            "mfg.tolerance.length.minus",
            json!({"value_num": "0.1", "value_unit": "mm"}),
            json!({"artifact_id": d.drawing}),
        ),
    )
    .await;
    let rep_scope = json!({"purchase_order": "PO-1", "po_line": "1", "lot": "L1", "supplier": "ACME"});
    let measured = claim(
        &s,
        body(
            "BRK-1",
            "qa.measured.length",
            json!({"value_num": "100.05", "value_unit": "mm"}),
            json!({"artifact_id": d.report, "scope": rep_scope}),
        ),
    )
    .await;
    let a = decide(
        &s,
        "BRK-1",
        vec![
            (len.clone(), json!({"type": "numeric_abs", "threshold": "0.5"})),
            (measured.clone(), json!({"type": "any_change"})),
            (ordered.clone(), json!({"type": "any_change"})),
        ],
        vec![],
    )
    .await
    .unwrap();
    let b = decide(&s, "BRK-1", vec![], vec![a.clone()]).await.unwrap();
    let info = decide(
        &s,
        "BRK-1",
        vec![(len.clone(), json!({"type": "informational"}))],
        vec![],
    )
    .await
    .unwrap();
    let small = decide(
        &s,
        "BRK-1",
        vec![(rev.clone(), json!({"type": "informational"}))],
        vec![],
    )
    .await
    .unwrap();

    // A within-threshold correction flags nothing material.
    let corr = |v: &str| {
        json!({"subject_type": "part", "subject_key": "BRK-1", "predicate": "mfg.dimension.length",
        "value_num": v, "value_unit": "mm", "revision": "B", "artifact_id": d.drawing, "reason": "drawing re-read"})
    };
    let (code, sim) = simulate(
        &s,
        &s.tok,
        json!({"type": "supersede", "claim_id": len, "correction": corr("100.2")}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{sim:?}");
    assert_eq!(sim["decisions"]["material_total"], 0);
    assert_eq!(sim["change"]["after"]["value_num"], "100.2");
    assert!(sim["change"]["after"]["id"].is_null());

    // 100 mm → 101 mm: A is material (beyond 0.5), B follows A, the informational link is recorded only.
    let (len2, s2) = (len.clone(), &s);
    let sim = check(
        &s,
        json!({"type": "supersede", "claim_id": len, "correction": corr("101")}),
        || async move { supersede(s2, &len2, corr("101")).await },
    )
    .await;
    let by: BTreeMap<&str, &Value> = sim["decisions"]["sample"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["decision_id"].as_str().unwrap(), i))
        .collect();
    assert_eq!(by[a.as_str()]["status_after"], "potentially_affected");
    assert_eq!(by[a.as_str()]["semantic_change"]["from"], "100 mm");
    assert_eq!(by[b.as_str()]["path"].as_array().unwrap().last().unwrap(), &json!(b));
    assert_eq!(by[b.as_str()]["cause_kind"], "upstream_decision");
    assert_eq!(by[info.as_str()]["material"], false);
    assert!(!by.contains_key(small.as_str()));
    assert_eq!(sim["decisions"]["newly_flagged_total"], 2);
    // The graph links the old value to every affected decision along its path.
    let nodes: BTreeSet<&str> = sim["graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap())
        .collect();
    assert!(nodes.contains(a.as_str()) && nodes.contains(b.as_str()));

    // Errors are those of the real endpoint; the corrected claim cannot be simulated again.
    let (code, e) = simulate(
        &s,
        &s.tok,
        json!({"type": "supersede", "claim_id": len, "correction": corr("102")}),
    )
    .await;
    assert_eq!(
        (code, e["error"].as_str()),
        (StatusCode::CONFLICT, Some("already_superseded"))
    );
    let mut wrong = corr("102");
    wrong["predicate"] = json!("mfg.material");
    let (code, e) = simulate(
        &s,
        &s.tok,
        json!({"type": "supersede", "claim_id": rev, "correction": wrong}),
    )
    .await;
    assert_eq!(
        (code, e["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("supersede_changes_identity"))
    );

    // Withdrawing the inspection report (every claim of the document) at once.
    let rep_claims: Vec<String> = vec![measured.clone()];
    check(
        &s,
        json!({"type": "withdraw", "artifact_id": d.report}),
        || async move { reject(s2, &rep_claims).await },
    )
    .await;
}

#[tokio::test]
async fn recorded_simulation_is_audited_and_viewers_may_simulate() {
    let s = tenant(ctx().await).await;
    let c1 = claim(
        &s,
        body("SFT-9", "mfg.material", json!({"value_text": "C45"}), json!({})),
    )
    .await;
    decide(&s, "SFT-9", vec![(c1.clone(), json!({"type": "any_change"}))], vec![])
        .await
        .unwrap();
    let before = counts(&s).await;
    let r =
        s.c.post(
            "/api/v1/simulations",
            Some(&s.tok),
            json!({"change": {"type": "withdraw", "claim_ids": [c1]}, "record": true}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["decisions"]["material_total"], 1);
    let after = counts(&s).await;
    // Exactly one audit event, nothing else.
    for (k, v) in &after {
        let d = v - before[k];
        match k.as_str() {
            "audit_events" | "audit_heads.last_seq" => assert_eq!(d, 1, "{k}"),
            _ => assert_eq!(d, 0, "{k} changed"),
        }
    }
    let digest: String = {
        let mut tx = db::tenant_tx(&s.c.owner, s.t).await.unwrap();
        sqlx::query_scalar("SELECT details->>'result_sha256' FROM audit_events WHERE action = 'simulation.run'")
            .fetch_one(&mut *tx)
            .await
            .unwrap()
    };
    assert_eq!(json!(digest), r.body["result_sha256"]);
    assert!(r.body["run_id"].is_string());

    let (slug, viewer) = {
        let row: (String,) = sqlx::query_as("SELECT slug FROM tenants WHERE id = $1")
            .bind(s.t)
            .fetch_one(&s.c.owner)
            .await
            .unwrap();
        let (_, e) = s.c.user(s.t, "viewer").await;
        (row.0, e)
    };
    let vt = s.c.token(&slug, &viewer).await;
    let (code, _) = simulate(&s, &vt, json!({"type": "withdraw", "claim_ids": [c1]})).await;
    assert_eq!(code, StatusCode::OK);
    let (code, _) = simulate(&s, &vt, json!({"type": "withdraw", "claim_ids": []})).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn tenants_are_isolated() {
    let c = ctx().await;
    let a = tenant(c).await;
    let b = tenant(ctx().await).await;
    let ca = claim(
        &a,
        body("ISO-1", "mfg.material", json!({"value_text": "C45"}), json!({})),
    )
    .await;
    let cb = claim(
        &b,
        body("ISO-1", "mfg.material", json!({"value_text": "C45"}), json!({})),
    )
    .await;
    let da = decide(&a, "ISO-1", vec![(ca.clone(), json!({"type": "any_change"}))], vec![])
        .await
        .unwrap();
    decide(&b, "ISO-1", vec![(cb.clone(), json!({"type": "any_change"}))], vec![])
        .await
        .unwrap();
    // B cannot simulate A's claim (it does not exist for B).
    let (code, _) = simulate(&b, &b.tok, json!({"type": "withdraw", "claim_ids": [ca]})).await;
    assert_eq!(code, StatusCode::NOT_FOUND);
    let corr = json!({"subject_type": "part", "subject_key": "ISO-1", "predicate": "mfg.material", "value_text": "S45C", "reason": "x"});
    let (code, _) = simulate(
        &b,
        &b.tok,
        json!({"type": "supersede", "claim_id": ca, "correction": corr}),
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND);
    // B's own change about the same subject key never reaches A's decisions.
    let (code, sim) = simulate(
        &b,
        &b.tok,
        json!({"type": "supersede", "claim_id": cb, "correction": corr}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{sim:?}");
    assert_eq!(sim["decisions"]["affected_total"], 1);
    assert_ne!(sim["decisions"]["sample"][0]["decision_id"], json!(da));
}

// ---- Property: simulation == real outcome, on generated tenants -----------------------------------------------

const PARTS: [&str; 2] = ["PRT-1", "PRT-2"];
const SUPPLIERS: [&str; 2] = ["ACME", "BETA"];

fn materiality(r: &mut StdRng) -> Value {
    match r.random_range(0..4) {
        0 => json!({"type": "informational"}),
        1 => {
            let th = ["0.05", "0.5", "2"][r.random_range(0..3)];
            json!({"type": "numeric_abs", "threshold": th})
        }
        _ => json!({"type": "any_change"}),
    }
}

/// A random variation of claim `c` (same identity): value, unit, revision or scope change.
fn variation(r: &mut StdRng, c: &Value) -> Value {
    let mut b = json!({ "subject_type": c["subject_type"], "subject_key": c["subject_key"], "predicate": c["predicate"],
        "value_text": c["value_text"], "value_num": c["value_num"], "value_unit": c["value_unit"], "revision": c["revision"],
        "scope": c["scope"], "artifact_id": c["artifact_id"], "reason": "simulated correction" });
    match r.random_range(0..5) {
        0 if c["value_num"].is_string() => {
            let v: f64 = c["value_num"].as_str().unwrap().parse().unwrap();
            b["value_num"] = json!(format!("{:.2}", v + [0.01, 0.3, 1.0, 5.0][r.random_range(0..4)]));
        }
        1 if c["value_unit"] == "mm" => {
            // Same quantity in another unit (immaterial) or a different one.
            let v: f64 = c["value_num"].as_str().unwrap().parse().unwrap();
            b["value_unit"] = json!("in");
            b["value_num"] = json!(if r.random_bool(0.5) {
                format!("{:.4}", v / 25.4)
            } else {
                "3".into()
            });
        }
        2 => b["revision"] = json!(["A", "B", "C"][r.random_range(0..3)]),
        3 if c["value_text"].is_string() => b["value_text"] = json!(["B", "C", "SUS304", "c45"][r.random_range(0..4)]),
        _ => {
            if c["value_num"].is_string() {
                b["value_num"] = json!("150");
            } else {
                b["value_text"] = json!("X");
            }
        }
    }
    b
}

async fn generated_case(seed: u64) {
    let mut r = StdRng::seed_from_u64(seed);
    let s = tenant(ctx().await).await;
    let mut claims: Vec<(String, Value)> = Vec::new();
    for (pi, p) in PARTS.iter().enumerate() {
        let d = docs(&s, p).await;
        let sup = SUPPLIERS[(pi + r.random_range(0..2)) % 2];
        let line = format!("{}", pi + 1);
        let po_scope = json!({"purchase_order": "PO-9", "po_line": line, "supplier": sup});
        let rep_scope = json!({"purchase_order": "PO-9", "po_line": line, "lot": format!("L{pi}"), "supplier": sup});
        let rev = ["A", "B"][r.random_range(0..2)];
        let nominal = 100 + 10 * pi;
        let drev = ["A", "B"][r.random_range(0..2)];
        let mut templates = vec![
            body(
                p,
                "po.revision_ordered",
                json!({"value_text": rev}),
                json!({"artifact_id": d.po, "scope": po_scope}),
            ),
            body(
                p,
                "po.supplier",
                json!({"value_text": sup}),
                json!({"artifact_id": d.po, "scope": po_scope}),
            ),
            body(
                p,
                "mfg.revision",
                json!({"value_text": drev}),
                json!({"artifact_id": d.drawing}),
            ),
            body(
                p,
                "mfg.dimension.length",
                json!({"value_num": format!("{nominal}"), "value_unit": "mm"}),
                json!({"artifact_id": d.drawing, "revision": rev}),
            ),
            body(
                p,
                "mfg.tolerance.length.plus",
                json!({"value_num": "0.1", "value_unit": "mm"}),
                json!({"artifact_id": d.drawing}),
            ),
            body(
                p,
                "mfg.tolerance.length.minus",
                json!({"value_num": "0.1", "value_unit": "mm"}),
                json!({"artifact_id": d.drawing}),
            ),
            body(
                p,
                "qa.measured.length",
                json!({"value_num": format!("{:.2}", nominal as f64 + r.random_range(-0.3..0.3)), "value_unit": "mm"}),
                json!({"artifact_id": d.report, "scope": rep_scope}),
            ),
            body(p, "mfg.material", json!({"value_text": "C45"}), json!({})),
        ];
        // Certificate dates (date-order signals) and a later drawing version (revision drift).
        let day = |r: &mut StdRng| format!("2026-0{}-{:02}", r.random_range(1..4), r.random_range(1..29));
        for pred in ["qa.certificate_date", "qa.ship_date", "qa.inspection_date"] {
            let v = day(&mut r);
            templates.push(body(
                p,
                pred,
                json!({"value_text": v}),
                json!({"artifact_id": d.report, "scope": rep_scope}),
            ));
        }
        templates.push(body(
            p,
            "mfg.drawing_number",
            json!({"value_text": format!("DN-{pi}")}),
            json!({"artifact_id": d.drawing}),
        ));
        if r.random_bool(0.6) {
            let d2 = doc(&s, "drawing", &format!("Drawing {p} v2.pdf")).await;
            let len2 = format!("{}", nominal + r.random_range(0..2));
            templates.push(body(
                p,
                "mfg.drawing_number",
                json!({"value_text": format!("DN-{pi}")}),
                json!({"artifact_id": d2}),
            ));
            templates.push(body(
                p,
                "mfg.revision",
                json!({"value_text": "C"}),
                json!({"artifact_id": d2}),
            ));
            templates.push(body(
                p,
                "mfg.dimension.length",
                json!({"value_num": len2, "value_unit": "mm"}),
                json!({"artifact_id": d2, "revision": "C"}),
            ));
        }
        if r.random_bool(0.5) {
            templates.push(body(p, "qa.measured.length", json!({"value_num": format!("{nominal}.1"), "value_unit": "mm"}),
                json!({"artifact_id": d.report, "scope": {"purchase_order": "PO-9", "po_line": line, "lot": "LX", "supplier": sup}})));
        }
        for t in templates {
            if r.random_bool(0.85) {
                let id = claim(&s, t.clone()).await;
                let mut full = t;
                full["id"] = json!(id);
                claims.push((id, full));
            }
        }
    }
    if claims.is_empty() {
        return;
    }
    let mut decs: Vec<String> = Vec::new();
    for _ in 0..r.random_range(2..9) {
        let part = PARTS[r.random_range(0..2)];
        let mine: Vec<&(String, Value)> = claims.iter().filter(|c| c.1["subject_key"] == part).collect();
        let mut ev = Vec::new();
        for _ in 0..r.random_range(0..4) {
            if let Some(c) = mine.get(r.random_range(0..mine.len().max(1)))
                && !ev.iter().any(|(x, _): &(String, Value)| x == &c.0)
            {
                ev.push((c.0.clone(), materiality(&mut r)));
            }
        }
        let mut up = Vec::new();
        for _ in 0..r.random_range(0..3) {
            if !decs.is_empty() {
                let d = decs[r.random_range(0..decs.len())].clone();
                if !up.contains(&d) {
                    up.push(d);
                }
            }
        }
        if ev.is_empty() && up.is_empty() {
            continue;
        }
        if let Some(d) = decide(&s, part, ev, up).await {
            decs.push(d);
        }
    }
    if r.random_bool(0.6) {
        let (id, c) = claims[r.random_range(0..claims.len())].clone();
        let corr = variation(&mut r, &c);
        let (id2, corr2) = (id.clone(), corr.clone());
        let s2 = &s;
        check(
            &s,
            json!({"type": "supersede", "claim_id": id, "correction": corr}),
            || async move { supersede(s2, &id2, corr2).await },
        )
        .await;
    } else if r.random_bool(0.5) {
        let n = r.random_range(1..3).min(claims.len());
        let mut ids: Vec<String> = Vec::new();
        while ids.len() < n {
            let id = claims[r.random_range(0..claims.len())].0.clone();
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids.sort();
        let ids2 = ids.clone();
        let s2 = &s;
        check(&s, json!({"type": "withdraw", "claim_ids": ids}), || async move {
            reject(s2, &ids2).await
        })
        .await;
    } else {
        // Every current claim of one document.
        let (_, c) = claims.iter().find(|c| c.1["artifact_id"].is_string()).cloned().unwrap();
        let art = c["artifact_id"].as_str().unwrap().to_owned();
        let mut ids: Vec<String> = claims
            .iter()
            .filter(|x| x.1["artifact_id"] == json!(art))
            .map(|x| x.0.clone())
            .collect();
        ids.sort();
        let s2 = &s;
        check(&s, json!({"type": "withdraw", "artifact_id": art}), || async move {
            reject(s2, &ids).await
        })
        .await;
    }
}

#[tokio::test]
async fn simulation_equals_real_outcome_on_generated_tenants() {
    let cases: u64 = std::env::var("KIZUNA_WHATIF_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24);
    let base: u64 = std::env::var("KIZUNA_WHATIF_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x5151);
    for i in 0..cases {
        eprintln!("whatif case seed {}", base + i);
        generated_case(base + i).await;
    }
}

// ---- Performance: 100k-edge dependency graph -------------------------------------------------------------------

#[tokio::test]
async fn hundred_thousand_edges_are_simulated_within_bounds() {
    let s = tenant(ctx().await).await;
    let mut hubs = Vec::new();
    for i in 0..16 {
        hubs.push(
            claim(
                &s,
                body(
                    "HUB-1",
                    &format!("mfg.dimension.d{i}"),
                    json!({"value_num": "10", "value_unit": "mm"}),
                    json!({}),
                ),
            )
            .await,
        );
    }
    let hub_ids: Vec<Uuid> = hubs.iter().map(|h| Uuid::parse_str(h).unwrap()).collect();
    // 25 000 decisions; each has one claim edge (to hub i % 16) and 3 edges to earlier decisions: 100k edges.
    let n: i64 = 25_000;
    let mut tx = db::tenant_tx(&s.c.owner, s.t).await.unwrap();
    // Ids unique per run: a fresh UUIDv7 prefix with the sequence number as the node part.
    let prefix = Uuid::now_v7().to_string()[..24].to_owned();
    let dec = format!("('{prefix}' || lpad(to_hex(g), 12, '0'))::uuid");
    sqlx::query(&format!(
        "INSERT INTO decisions (id, tenant_id, kind, title, subject_type, subject_key, outcome, rationale, decided_by, derivation, derivation_sha256) \
         SELECT {dec}, $1, 'bulk_release', 'Bulk ' || g, 'part', 'HUB-1', 'approved', 'bulk', $2, '{{}}', sha256(g::text::bytea) \
         FROM generate_series(1, $3) g"
    ))
    .bind(s.t)
    .bind(s.uid)
    .bind(n)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(&format!(
        "INSERT INTO decision_dependencies (id, tenant_id, decision_id, depends_on_claim, role, subject_key, subject_type, predicate, materiality, snapshot) \
         SELECT gen_random_uuid(), $1, {dec}, ($2::uuid[])[1 + g % 16], 'basis', 'hub-1', 'part', 'mfg.dimension.d' || (g % 16), \
                '{{\"type\": \"any_change\"}}', '{{\"num\": \"10\", \"unit\": \"mm\"}}' FROM generate_series(1, $3) g"
    ))
    .bind(s.t)
    .bind(&hub_ids)
    .bind(n)
    .execute(&mut *tx)
    .await
    .unwrap();
    let up = format!("('{prefix}' || lpad(to_hex(1 + ((g * 7919 + k * 104729) % (g - 1))), 12, '0'))::uuid");
    sqlx::query(&format!(
        "INSERT INTO decision_dependencies (id, tenant_id, decision_id, depends_on_decision, role, subject_key, materiality, snapshot) \
         SELECT gen_random_uuid(), $1, {dec}, {up}, 'supporting', 'hub-1', '{{\"type\": \"any_change\"}}', '{{}}' \
         FROM generate_series(2, $2) g, generate_series(1, 3) k"
    ))
    .bind(s.t)
    .bind(n)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let edges: i64 = {
        let mut tx = db::tenant_tx(&s.c.owner, s.t).await.unwrap();
        sqlx::query_scalar("SELECT count(*) FROM decision_dependencies")
            .fetch_one(&mut *tx)
            .await
            .unwrap()
    };
    assert!(edges >= 99_990, "{edges} edges");

    let rows0 = counts(&s).await;
    let corr = json!({"subject_type": "part", "subject_key": "HUB-1", "predicate": "mfg.dimension.d0", "value_num": "11",
        "value_unit": "mm", "reason": "re-measured"});
    let t0 = Instant::now();
    let r =
        s.c.post(
            "/api/v1/simulations",
            Some(&s.tok),
            json!({"change": {"type": "supersede", "claim_id": hubs[0], "correction": corr}, "sample": 100}),
        )
        .await;
    let ms = t0.elapsed().as_millis();
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    eprintln!(
        "whatif 100k edges: {ms} ms, affected {}, material {}",
        r.body["decisions"]["affected_total"], r.body["decisions"]["material_total"]
    );
    let d = &r.body["decisions"];
    assert_eq!(d["bounded"], false);
    assert!(d["affected_total"].as_u64().unwrap() > 1_000);
    assert_eq!(
        d["sample"].as_array().unwrap().len(),
        100,
        "large impacts: counts plus a bounded sample"
    );
    assert_eq!(d["truncated"], true);
    assert_eq!(d["review_batch"], d["affected_total"].as_u64().unwrap() > 10_000);
    assert!(r.body["graph"]["nodes"].as_array().unwrap().len() <= 100 * 8);
    assert_eq!(counts(&s).await, rows0);
    // Debug build on a shared host (the 60 s request timeout is the hard limit); measured ~4 s here.
    assert!(ms < 30_000, "simulation took {ms} ms");
}

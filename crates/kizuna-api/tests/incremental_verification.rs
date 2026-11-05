#![allow(clippy::unwrap_used)]
//! Incremental verification (PERF_VERIFY_2026-11-05). Property, on the real database: for random change sequences
//! (claim inserts in several spellings and source kinds, supersessions, rejections / re-acceptance, human conflict
//! reviews, alias decisions, calendar-day changes), the state an `auto` (incremental) run leaves is exactly the
//! state a full run leaves from the same starting point, and a full run afterwards changes nothing.

mod common;

use common::*;
use kizuna_api::db;
use kizuna_api::verification::{RunMode, RunResp, detect};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

const KEYS: &[&str] = &[
    "BRK-1",
    "brk-1",
    "ＢＲＫ-1",
    "SHF-2",
    "shf-2 ",
    "HSG-3",
    "HSG3",
    "PIN-4",
];
const PREDS: &[(&str, &[&str], Option<&str>)] = &[
    ("mfg.revision", &["B", "C"], None),
    ("mfg.dimension.length", &["120", "125"], Some("mm")),
    ("mfg.tolerance.length.plus", &["0.2"], Some("mm")),
    ("mfg.tolerance.length.minus", &["0.2"], Some("mm")),
    ("mfg.approval.status", &["approved"], None),
    ("mfg.certificate.expiry", &["2026-11-06", "2031-01-01"], None),
    ("mfg.material", &["S355JR", "SUS304"], None),
    ("po.revision_ordered", &["B", "C"], None),
    ("qa.measured.length", &["120.05", "120.9"], Some("mm")),
    ("qa.result", &["pass", "fail"], None),
];

type Snap = Vec<(String, Uuid, Option<Uuid>, String, String, Value)>;

async fn snapshot(tx: &mut db::Tx) -> Snap {
    sqlx::query_as("SELECT kind, claim_a, claim_b, status, subject_key, explanation FROM conflicts ORDER BY 1, 2, 3")
        .fetch_all(&mut **tx)
        .await
        .unwrap()
}

async fn run(c: &Ctx, t: Uuid, now: OffsetDateTime, mode: RunMode, commit: bool) -> (RunResp, Snap) {
    let mut tx = db::tenant_tx(&c.st.pool, t).await.unwrap();
    let r = detect(&mut tx, t, now, mode).await.unwrap();
    let s = snapshot(&mut tx).await;
    if commit {
        tx.commit().await.unwrap();
    } else {
        tx.rollback().await.unwrap();
    }
    (r, s)
}

async fn insert_claim(tx: &mut db::Tx, t: Uuid, rng: &mut StdRng, arts: &[Uuid]) -> Uuid {
    let id = Uuid::now_v7();
    let (pred, vals, unit) = PREDS[rng.random_range(0..PREDS.len())];
    let v = vals[rng.random_range(0..vals.len())];
    let numeric = unit.is_some();
    let art = if arts.is_empty() || rng.random_bool(0.3) {
        None
    } else {
        Some(arts[rng.random_range(0..arts.len())])
    };
    let scope = if pred.starts_with("qa.") {
        json!({"lot": format!("L{}", rng.random_range(0..2))})
    } else {
        json!({})
    };
    sqlx::query(
        "INSERT INTO claims (id, tenant_id, subject_type, subject_key, predicate, value_text, value_num, value_unit, revision, scope, artifact_id, extraction_method, confidence_bps) \
         VALUES ($1,$2,'part',$3,$4,$5,$6::numeric,$7,$8,$9,$10,$11,9000)",
    )
    .bind(id)
    .bind(t)
    .bind(KEYS[rng.random_range(0..KEYS.len())])
    .bind(pred)
    .bind((!numeric).then_some(v))
    .bind(numeric.then_some(v))
    .bind(unit)
    .bind(["B", "C"][rng.random_range(0..2)])
    .bind(scope)
    .bind(art)
    .bind(if art.is_none() { "manual" } else { "rules:v1" })
    .execute(&mut **tx)
    .await
    .unwrap();
    id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incremental_matches_full() {
    let c = ctx().await;
    let mut incremental_runs = 0;
    for seed in 0..4u64 {
        let mut rng = StdRng::seed_from_u64(seed);
        let (t, _) = c.tenant().await;
        let (user, _) = c.user(t, "reviewer").await;
        let mut now = OffsetDateTime::now_utc();
        let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
        let mut arts = vec![];
        for kind in ["spec_document", "drawing", "purchase_order", "inspection_report"] {
            let a = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO artifacts (id, tenant_id, sha256, size_bytes, media_type, kind, status, storage_key, display_name, created_by) \
                 VALUES ($1,$2,$3,1,'application/pdf',$4,'accepted','k','d.pdf',$5)",
            )
            .bind(a)
            .bind(t)
            .bind(kizuna_api::crypto::random_bytes::<32>().to_vec())
            .bind(kind)
            .bind(user)
            .execute(&mut *tx)
            .await
            .unwrap();
            arts.push(a);
        }
        let mut claims = vec![];
        for _ in 0..12 {
            claims.push(insert_claim(&mut tx, t, &mut rng, &arts).await);
        }
        tx.commit().await.unwrap();
        run(&c, t, now, RunMode::Auto, true).await;
        for step in 0..30 {
            let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
            for _ in 0..rng.random_range(1..4) {
                match rng.random_range(0..100) {
                    0..45 => claims.push(insert_claim(&mut tx, t, &mut rng, &arts).await),
                    45..60 => {
                        let old = claims[rng.random_range(0..claims.len())];
                        let new = insert_claim(&mut tx, t, &mut rng, &arts).await;
                        claims.push(new);
                        sqlx::query("UPDATE claims SET superseded_at = now(), superseded_by = $2 WHERE id = $1 AND superseded_at IS NULL")
                            .bind(old)
                            .bind(new)
                            .execute(&mut *tx)
                            .await
                            .unwrap();
                    }
                    60..80 => {
                        let st = ["rejected", "accepted", "unreviewed"][rng.random_range(0..3)];
                        sqlx::query("UPDATE claims SET review_status = $2 WHERE id = $1")
                            .bind(claims[rng.random_range(0..claims.len())])
                            .bind(st)
                            .execute(&mut *tx)
                            .await
                            .unwrap();
                    }
                    80..92 => {
                        // A reviewer handles an open finding.
                        let st = ["needs_evidence", "resolved_approved", "resolved_rejected"][rng.random_range(0..3)];
                        sqlx::query(
                            "UPDATE conflicts SET status = $1 WHERE id = (SELECT id FROM conflicts WHERE status = 'open' ORDER BY id LIMIT 1)",
                        )
                        .bind(st)
                        .execute(&mut *tx)
                        .await
                        .unwrap();
                    }
                    92..96 => {
                        // Entity decision: two spellings are the same part.
                        let d = Uuid::now_v7();
                        sqlx::query("INSERT INTO entity_decisions (id, tenant_id, subject_type, left_key, right_key, outcome, decided_by) VALUES ($1,$2,'part','HSG3','HSG-3','same',$3)")
                            .bind(d).bind(t).bind(user).execute(&mut *tx).await.unwrap();
                        sqlx::query("INSERT INTO entity_aliases (tenant_id, subject_type, alias_key, canonical_key, decision_id) VALUES ($1,'part','HSG3','HSG-3',$2) ON CONFLICT DO NOTHING")
                            .bind(t).bind(d).execute(&mut *tx).await.unwrap();
                    }
                    _ => now += time::Duration::days(1),
                }
            }
            tx.commit().await.unwrap();
            // Reference: a full run from the same state (rolled back).
            let (_, want) = run(&c, t, now, RunMode::Full, false).await;
            let (r, got) = run(&c, t, now, RunMode::Auto, true).await;
            if r.mode == "incremental" {
                incremental_runs += 1;
            }
            assert_eq!(
                got, want,
                "seed {seed} step {step}: {} run differs from a full run",
                r.mode
            );
            let (r2, after) = run(&c, t, now, RunMode::Full, false).await;
            assert_eq!((r2.findings_new, r2.marked_obsolete), (0, 0), "seed {seed} step {step}");
            assert_eq!(after, got);
        }
    }
    assert!(
        incremental_runs > 60,
        "only {incremental_runs} incremental runs exercised"
    );
}

/// The endpoint takes `?mode=full`; a run with no changes evaluates nothing; the watermark advances.
#[tokio::test]
async fn endpoint_modes_and_watermark() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "reviewer").await;
    let tok = c.token(&slug, &email).await;
    let r1 = c.post("/api/v1/verification/run", Some(&tok), json!({})).await;
    assert_eq!(r1.body["mode"], "full", "first run has no state: {:?}", r1.body);
    let r2 = c.post("/api/v1/verification/run", Some(&tok), json!({})).await;
    assert_eq!(
        (r2.body["mode"].as_str(), r2.body["subjects_evaluated"].as_u64()),
        (Some("incremental"), Some(0))
    );
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    let mut rng = StdRng::seed_from_u64(9);
    insert_claim(&mut tx, t, &mut rng, &[]).await;
    tx.commit().await.unwrap();
    let r3 = c.post("/api/v1/verification/run", Some(&tok), json!({})).await;
    assert_eq!(
        (r3.body["mode"].as_str(), r3.body["subjects_evaluated"].as_u64()),
        (Some("incremental"), Some(1))
    );
    assert!(r3.body["watermark"].as_i64().unwrap() > r2.body["watermark"].as_i64().unwrap());
    let r4 = c
        .post("/api/v1/verification/run?mode=full", Some(&tok), json!({}))
        .await;
    assert_eq!(r4.body["mode"], "full");
}

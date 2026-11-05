#![allow(clippy::unwrap_used)]
//! Upload staging (PERF_VERIFY_2026-11-05): an object is staged, its row committed, then promoted. A failure before
//! or at the commit never leaves a stored object without a row (orphan), and a committed row never lacks its object.

mod common;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::http::StatusCode;
use common::*;
use kizuna_api::db;
use kizuna_api::storage::{FsStore, ObjectKey, sweep_staging_job};
use uuid::Uuid;

/// Stored objects (not staging copies) under the storage root: (tenant, sha256 hex).
fn stored_objects(root: &Path) -> HashSet<(String, String)> {
    let mut out = HashSet::new();
    let Ok(tenants) = std::fs::read_dir(root) else {
        return out;
    };
    for t in tenants.flatten() {
        let tn = t.file_name().to_string_lossy().into_owned();
        if tn.starts_with('.') {
            continue;
        }
        for hh in std::fs::read_dir(t.path()).unwrap().flatten() {
            for f in std::fs::read_dir(hh.path()).unwrap().flatten() {
                out.insert((tn.clone(), f.file_name().to_string_lossy().into_owned()));
            }
        }
    }
    out
}

/// Every artifact row has an intact object and every stored object has a row; nothing left in staging.
async fn assert_consistent(c: &Ctx, tenant: Uuid) -> usize {
    let mut tx = db::tenant_tx(&c.owner, tenant).await.unwrap();
    let rows: Vec<Vec<u8>> = sqlx::query_scalar("SELECT sha256 FROM artifacts")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let store = FsStore::new(&c.st.cfg.storage_dir);
    let mut missing = 0;
    for sha in &rows {
        let key = ObjectKey {
            tenant,
            sha256: sha.as_slice().try_into().unwrap(),
        };
        let ok = std::fs::read(store.path(&key)).is_ok_and(|b| kizuna_api::crypto::sha256(&b) == key.sha256);
        missing += usize::from(!ok);
    }
    let with_row: HashSet<(String, String)> = rows.iter().map(|s| (tenant.to_string(), hex::encode(s))).collect();
    let orphans: Vec<_> = stored_objects(&c.st.cfg.storage_dir)
        .into_iter()
        .filter(|o| o.0 == tenant.to_string() && !with_row.contains(o))
        .collect();
    assert_eq!(missing, 0, "artifact rows without their object");
    assert!(orphans.is_empty(), "orphan objects: {orphans:?}");
    assert!(
        store.staged().unwrap().iter().all(|s| s.0.tenant != tenant),
        "staging copies left"
    );
    rows.len()
}

fn doc(i: usize) -> Vec<u8> {
    format!("Lot,Part Number,Result\nL{i},BRK-{i},pass\n").into_bytes()
}

/// A failure at COMMIT (outcome unknown to the server) leaves a staging copy, never an object; the sweep deletes it
/// once old enough. A failure before the commit discards the staging copy at once. A committed upload whose
/// promotion never happened is promoted by the sweep.
#[tokio::test]
async fn failed_commits_leave_no_orphans_and_sweep_promotes_committed() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "analyst").await;
    let tok = c.token(&slug, &email).await;
    let tag = Uuid::new_v4().simple().to_string();
    // Deferred constraint trigger: fails this tenant's upload named fail-<tag>.csv at COMMIT time.
    for sql in [
        format!(
            "CREATE FUNCTION kz_test_fail_{tag}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN \
             IF NEW.tenant_id = '{t}' AND NEW.display_name = 'fail-{tag}.csv' THEN RAISE EXCEPTION 'injected commit failure'; END IF; \
             RETURN NULL; END $$"
        ),
        format!(
            "CREATE CONSTRAINT TRIGGER kz_test_fail_{tag} AFTER INSERT ON artifacts DEFERRABLE INITIALLY DEFERRED \
             FOR EACH ROW EXECUTE FUNCTION kz_test_fail_{tag}()"
        ),
    ] {
        sqlx::query(&sql).execute(&c.owner).await.unwrap();
    }
    let ok = c.upload(&tok, "ok.csv", &doc(1), Some("inspection_report")).await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.body);
    let bad = c
        .upload(&tok, &format!("fail-{tag}.csv"), &doc(2), Some("inspection_report"))
        .await;
    assert!(bad.status.is_server_error(), "{:?}", bad.body);
    sqlx::query(&format!("DROP TRIGGER kz_test_fail_{tag} ON artifacts"))
        .execute(&c.owner)
        .await
        .unwrap();
    sqlx::query(&format!("DROP FUNCTION kz_test_fail_{tag}()"))
        .execute(&c.owner)
        .await
        .unwrap();
    let store = FsStore::new(&c.st.cfg.storage_dir);
    let staged: Vec<_> = store
        .staged()
        .unwrap()
        .into_iter()
        .filter(|s| s.0.tenant == t)
        .collect();
    assert_eq!(
        staged.len(),
        1,
        "commit outcome unknown: the staging copy is kept for the sweep"
    );
    // Committed, never promoted (as after a crash between COMMIT and rename).
    let s3 =
        c.st.store
            .stage(
                &ObjectKey {
                    tenant: t,
                    sha256: kizuna_api::crypto::sha256(&doc(3)),
                },
                &doc(3),
            )
            .unwrap();
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    sqlx::query(
        "INSERT INTO artifacts (id, tenant_id, sha256, size_bytes, media_type, kind, status, storage_key, display_name, created_by) \
         VALUES ($1,$2,$3,10,'text/csv','inspection_report','accepted','k','c.csv',$2)",
    )
    .bind(Uuid::now_v7())
    .bind(t)
    .bind(s3.key.sha256.to_vec())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    // Young orphan: kept (it could be an upload in flight).
    let n = sweep_staging_job(&c.st.pool, Arc::clone(&c.st.store), Duration::from_secs(3600))
        .await
        .unwrap();
    assert!(n.promoted >= 1);
    assert_eq!(store.staged().unwrap().iter().filter(|s| s.0.tenant == t).count(), 1);
    let n = sweep_staging_job(&c.st.pool, Arc::clone(&c.st.store), Duration::ZERO)
        .await
        .unwrap();
    assert!(n.deleted >= 1);
    assert_eq!(assert_consistent(&c, t).await, 2);
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    let audited: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT details FROM audit_events WHERE action = 'storage.staging_swept' ORDER BY seq")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(audited.len(), 2, "{audited:?}");
    assert_eq!(
        (audited[0]["promoted"].as_u64(), audited[1]["deleted"].as_u64()),
        (Some(1), Some(1))
    );
    assert!(
        audited.iter().all(|a| a.as_object().unwrap().len() == 3),
        "counts only: {audited:?}"
    );
}

/// Chaos: database connections are terminated while uploads run. Needs a superuser URL of a PRIVATE test
/// PostgreSQL (`KIZUNA_CHAOS_PG_SUPERUSER_URL`); run with `--ignored`. Never point it at a shared server.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "chaos: needs KIZUNA_CHAOS_PG_SUPERUSER_URL of a private PostgreSQL"]
async fn chaos_connection_kills_during_uploads_leave_no_orphans() {
    let su = db::connect(&std::env::var("KIZUNA_CHAOS_PG_SUPERUSER_URL").unwrap(), 1)
        .await
        .unwrap();
    let c = ctx_with(|cfg| cfg.rate.uploads_per_user_per_min = 10_000).await;
    let (t, slug) = c.tenant().await;
    let mut toks = vec![];
    for _ in 0..8 {
        let (_, email) = c.user(t, "analyst").await;
        toks.push(c.token(&slug, &email).await);
    }
    let c = Arc::new(c);
    let stop = Arc::new(AtomicBool::new(false));
    let killer = {
        let stop = Arc::clone(&stop);
        tokio::spawn(async move {
            let mut killed = 0i64;
            while !stop.load(Ordering::Relaxed) {
                let n: i64 = sqlx::query_scalar(
                    "SELECT count(*) FILTER (WHERE pg_terminate_backend(pid)) FROM pg_stat_activity \
                     WHERE usename = 'kizuna_app' AND datname = current_database() AND pid <> pg_backend_pid()",
                )
                .fetch_one(&su)
                .await
                .unwrap();
                killed += n;
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            killed
        })
    };
    let mut workers = vec![];
    for (w, tok) in toks.into_iter().enumerate() {
        let c = Arc::clone(&c);
        workers.push(tokio::spawn(async move {
            let (mut ok, mut failed) = (0, 0);
            for i in 0..40 {
                let r = c
                    .upload(&tok, "d.csv", &doc(w * 1000 + i), Some("inspection_report"))
                    .await;
                if r.status == StatusCode::CREATED || r.status == StatusCode::OK {
                    ok += 1;
                } else {
                    assert!(
                        r.status.is_server_error() || r.status == StatusCode::UNAUTHORIZED,
                        "{:?}",
                        r.body
                    );
                    failed += 1;
                }
            }
            (ok, failed)
        }));
    }
    let (mut ok, mut failed) = (0, 0);
    for w in workers {
        let (o, f) = w.await.unwrap();
        ok += o;
        failed += f;
    }
    stop.store(true, Ordering::Relaxed);
    let killed = killer.await.unwrap();
    // After the uploads ended every staging copy is resolvable: sweep with no age cut-off.
    let mut n = None;
    for _ in 0..20 {
        // The pool reconnects lazily; retry until the sweep's own queries get through.
        if let Ok(x) = sweep_staging_job(&c.st.pool, Arc::clone(&c.st.store), Duration::ZERO).await {
            n = Some(x);
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let n = n.unwrap();
    let rows = assert_consistent(&c, t).await;
    println!(
        "CHAOS uploads ok={ok} failed={failed} backends_killed={killed} rows={rows} sweep_promoted={} sweep_deleted={} orphans=0 missing=0",
        n.promoted, n.deleted
    );
    assert!(failed > 0 && killed > 0, "the chaos did not hit any upload");
    assert!(rows >= ok, "every acknowledged upload has its row");
}

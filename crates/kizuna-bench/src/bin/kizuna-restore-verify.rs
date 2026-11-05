//! Restore-drill verification: for every tenant in a (restored) database, recompute the audit hash chain with the
//! server's own verifier and re-hash every stored artifact object against its recorded SHA-256. Read-only.
//!
//! KIZUNA_OWNER_DATABASE_URL=... kizuna-restore-verify <storage-dir>
//! Prints a JSON summary; exit 0 = all intact, 2 = any chain broken or object missing/altered.

use kizuna_api::storage::{FsStore, ObjectKey};
use kizuna_api::{audit, crypto, db};
use serde_json::json;
use uuid::Uuid;

type R<T> = Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> R<()> {
    let storage = std::env::args()
        .nth(1)
        .ok_or("usage: kizuna-restore-verify <storage-dir>")?;
    let url = std::env::var("KIZUNA_OWNER_DATABASE_URL").map_err(|_| "KIZUNA_OWNER_DATABASE_URL not set")?;
    let pool = db::connect(&url, 2).await?;
    let store = FsStore::new(&storage);
    let tenants: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, slug FROM tenants ORDER BY slug")
        .fetch_all(&pool)
        .await?;
    let mut ok = !tenants.is_empty();
    let mut report = Vec::new();
    for (t, slug) in tenants {
        let mut tx = db::tenant_snapshot_tx(&pool, t).await?;
        let chain = audit::verify_tenant(&mut tx, t).await.map_err(|e| e.to_string())?;
        let arts: Vec<(Uuid, Vec<u8>, String)> =
            sqlx::query_as("SELECT id, sha256, status FROM artifacts WHERE tenant_id = $1 ORDER BY id")
                .bind(t)
                .fetch_all(&mut *tx)
                .await?;
        tx.commit().await?;
        let (mut intact, mut bad) = (0usize, Vec::new());
        for (id, sha, status) in &arts {
            let Ok(sha) = <[u8; 32]>::try_from(sha.as_slice()) else {
                bad.push(json!({ "artifact": id, "problem": "bad recorded hash" }));
                continue;
            };
            match std::fs::read(store.path(&ObjectKey { tenant: t, sha256: sha })) {
                Ok(b) if crypto::sha256(&b) == sha => intact += 1,
                Ok(_) => bad.push(json!({ "artifact": id, "status": status, "problem": "hash mismatch" })),
                Err(e) => bad
                    .push(json!({ "artifact": id, "status": status, "problem": format!("unreadable: {}", e.kind()) })),
            }
        }
        ok &= chain.intact && bad.is_empty();
        report.push(json!({ "tenant": slug, "audit_chain": chain, "artifacts": arts.len(), "artifacts_intact": intact, "artifact_problems": bad }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "ok": ok, "tenants": report }))?
    );
    if !ok {
        std::process::exit(2);
    }
    Ok(())
}

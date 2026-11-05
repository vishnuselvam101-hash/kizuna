//! Operator CLI (uses the owner role, KIZUNA_OWNER_DATABASE_URL). Every mutating action is audited.
//!
//! kizuna-admin migrate
//! kizuna-admin create-tenant <slug> <name>
//! kizuna-admin create-user <tenant-slug> <email> <role> <display-name>   (password from stdin)
//! kizuna-admin reset-mfa <tenant-slug> <email>      (emergency recovery; revokes sessions)
//! kizuna-admin unlock <tenant-slug> <email>
//! kizuna-admin verify-audit <tenant-slug>
//! kizuna-admin emergency-unlock <tenant-slug>   (re-enable admins, lift IP blocks/restrictions; audited)

use std::io::BufRead;

use kizuna_api::audit::{self, Record};
use kizuna_api::{auth, db, users};
use policy_engine::Role;
use serde_json::json;
use uuid::Uuid;

type R<T> = Result<T, Box<dyn std::error::Error>>;

fn op(
    action: &'static str,
    target_type: &'static str,
    id: impl ToString,
    details: serde_json::Value,
) -> Record<'static> {
    Record {
        actor_user_id: None,
        actor_kind: "operator",
        action,
        target_type,
        target_id: Some(id.to_string()),
        details,
    }
}

async fn tenant_id(pool: &sqlx::PgPool, slug: &str) -> R<Uuid> {
    let id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM tenants WHERE slug = $1")
        .bind(slug)
        .fetch_optional(pool)
        .await?;
    id.ok_or_else(|| "unknown tenant".into())
}

async fn user_id(tx: &mut db::Tx, email: &str) -> R<Uuid> {
    let id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(auth::normalize_email(email))
        .fetch_optional(&mut **tx)
        .await?;
    id.ok_or_else(|| "unknown user".into())
}

#[tokio::main]
async fn main() -> R<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let url = std::env::var("KIZUNA_OWNER_DATABASE_URL").map_err(|_| "KIZUNA_OWNER_DATABASE_URL not set")?;
    let pool = db::connect(&url, 2).await?;
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        ["migrate"] => {
            db::MIGRATOR.run(&pool).await?;
            println!("migrations applied");
        }
        ["create-tenant", slug, name] => {
            let id = Uuid::now_v7();
            let mut tx = pool.begin().await?;
            sqlx::query("INSERT INTO tenants (id, slug, name) VALUES ($1,$2,$3)")
                .bind(id)
                .bind(slug)
                .bind(name)
                .execute(&mut *tx)
                .await?;
            audit::append(&mut tx, id, op("tenant.created", "tenant", id, json!({ "slug": slug })))
                .await
                .map_err(|e| e.to_string())?;
            tx.commit().await?;
            println!("{id}");
        }
        ["create-user", slug, email, role, name] => {
            let role = Role::parse(role).map_err(|_| "invalid role")?;
            let mut pw = String::new();
            std::io::stdin().lock().read_line(&mut pw)?;
            let pw = pw.trim_end_matches(['\r', '\n']);
            let t = tenant_id(&pool, slug).await?;
            let mut tx = db::tenant_tx(&pool, t).await?;
            let id = users::insert_user(&mut tx, t, email, name, role, pw)
                .await
                .map_err(|e| e.to_string())?;
            audit::append(
                &mut tx,
                t,
                op("user.created", "user", id, json!({ "role": role.as_str() })),
            )
            .await
            .map_err(|e| e.to_string())?;
            tx.commit().await?;
            println!("{id}");
        }
        ["reset-mfa", slug, email] => {
            let t = tenant_id(&pool, slug).await?;
            let mut tx = db::tenant_tx(&pool, t).await?;
            let u = user_id(&mut tx, email).await?;
            sqlx::query("UPDATE users SET totp_enabled = false, totp_secret_enc = NULL, totp_pending_enc = NULL, totp_last_step = NULL WHERE id = $1")
                .bind(u).execute(&mut *tx).await?;
            let n = auth::revoke_user_sessions(&mut tx, t, u, None, "operator_mfa_reset").await?;
            audit::append(
                &mut tx,
                t,
                op("user.mfa_reset", "user", u, json!({ "sessions_revoked": n })),
            )
            .await
            .map_err(|e| e.to_string())?;
            tx.commit().await?;
            println!("mfa reset; {n} sessions revoked");
        }
        ["unlock", slug, email] => {
            let t = tenant_id(&pool, slug).await?;
            let mut tx = db::tenant_tx(&pool, t).await?;
            let u = user_id(&mut tx, email).await?;
            sqlx::query("UPDATE users SET failed_logins = 0, locked_until = NULL, status = 'active' WHERE id = $1")
                .bind(u)
                .execute(&mut *tx)
                .await?;
            audit::append(&mut tx, t, op("user.unlocked", "user", u, json!({})))
                .await
                .map_err(|e| e.to_string())?;
            tx.commit().await?;
            println!("unlocked");
        }
        ["emergency-unlock", slug] => {
            let t = tenant_id(&pool, slug).await?;
            let out = kizuna_api::incidents::emergency_unlock(&pool, t)
                .await
                .map_err(|e| e.to_string())?;
            println!("{out}");
        }
        ["verify-audit", slug] => {
            let t = tenant_id(&pool, slug).await?;
            let mut tx = db::tenant_snapshot_tx(&pool, t).await?;
            let s = audit::verify_tenant(&mut tx, t).await.map_err(|e| e.to_string())?;
            tx.commit().await?;
            println!("{}", serde_json::to_string_pretty(&s)?);
            if !s.intact {
                std::process::exit(2);
            }
        }
        _ => {
            eprintln!(
                "usage: kizuna-admin migrate | create-tenant <slug> <name> | create-user <slug> <email> <role> <name> \
                       | reset-mfa <slug> <email> | unlock <slug> <email> | verify-audit <slug> | emergency-unlock <slug>"
            );
            std::process::exit(64);
        }
    }
    Ok(())
}

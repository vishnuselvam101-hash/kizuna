//! Database access: pools, tenant-scoped transactions, migrations and runtime-role safety checks.

use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub type Tx = Transaction<'static, Postgres>;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

pub async fn connect(url: &str, max: u32) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(url)
        .await
}

/// Begins a transaction whose RLS tenant context is `tenant` (transaction-local).
pub async fn tenant_tx(pool: &PgPool, tenant: Uuid) -> Result<Tx, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, tenant).await?;
    Ok(tx)
}

pub async fn set_tenant(tx: &mut Tx, tenant: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('kizuna.tenant_id', $1, true)")
        .bind(tenant.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum RoleCheckError {
    #[error("database role must not be superuser")]
    Superuser,
    #[error("database role must not have BYPASSRLS")]
    BypassRls,
    #[error("database role must not own KIZUNA tables")]
    Owner,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Refuses to run the API with a role that could bypass row-level security.
pub async fn assert_runtime_role(pool: &PgPool) -> Result<(), RoleCheckError> {
    let (sup, bypass): (bool, bool) =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user")
            .fetch_one(pool)
            .await?;
    if sup {
        return Err(RoleCheckError::Superuser);
    }
    if bypass {
        return Err(RoleCheckError::BypassRls);
    }
    let owns: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_tables WHERE schemaname = 'public' AND tablename = 'users' \
         AND tableowner = current_user)",
    )
    .fetch_one(pool)
    .await?;
    if owns {
        return Err(RoleCheckError::Owner);
    }
    Ok(())
}

//! Database access: pools, tenant-scoped transactions, migrations and runtime-role safety checks.

use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub type Tx = Transaction<'static, Postgres>;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// Database role names. The migrations are written for `kizuna` (schema owner) and `kizuna_app` (runtime);
/// `migrator` maps those identifiers to the configured names at migrate time, so one PostgreSQL cluster can host
/// installations with separate roles (e.g. the pilot's `kizuna_pilot_owner` / `kizuna_pilot_app`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roles {
    pub owner: String,
    pub app: String,
}

impl Default for Roles {
    fn default() -> Self {
        Roles {
            owner: "kizuna".into(),
            app: "kizuna_app".into(),
        }
    }
}

impl Roles {
    /// `KIZUNA_DB_OWNER_ROLE` / `KIZUNA_DB_APP_ROLE` (defaults `kizuna` / `kizuna_app`).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let d = Roles::default();
        let r = Roles {
            owner: get("KIZUNA_DB_OWNER_ROLE").filter(|v| !v.is_empty()).unwrap_or(d.owner),
            app: get("KIZUNA_DB_APP_ROLE").filter(|v| !v.is_empty()).unwrap_or(d.app),
        };
        let ok = |n: &str| {
            n.len() <= 63
                && n.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
                && n.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        };
        if !ok(&r.owner) || !ok(&r.app) || r.owner == r.app {
            return Err("invalid KIZUNA_DB_OWNER_ROLE / KIZUNA_DB_APP_ROLE".into());
        }
        Ok(r)
    }

    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }
}

/// Replaces the role identifiers `kizuna` and `kizuna_app` (whole SQL identifiers outside string literals; not
/// `kizuna.tenant_id`-style qualified names) with the configured roles.
pub fn map_roles(sql: &str, roles: &Roles) -> String {
    let b = sql.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = String::with_capacity(sql.len());
    let (mut i, mut in_str) = (0, false);
    while i < b.len() {
        let c = b[i];
        if !in_str && c == b'-' && b.get(i + 1) == Some(&b'-') {
            // Line comment: copied verbatim (apostrophes in prose must not open a string literal).
            let end = sql[i..].find('\n').map_or(b.len(), |n| i + n);
            out.push_str(&sql[i..end]);
            i = end;
            continue;
        }
        if c == b'\'' {
            in_str = !in_str;
        }
        if !in_str && ident(c) && (i == 0 || !(ident(b[i - 1]) || b[i - 1] == b'.')) {
            let mut j = i;
            while j < b.len() && ident(b[j]) {
                j += 1;
            }
            let tok = &sql[i..j];
            let qualified = j < b.len() && b[j] == b'.';
            out.push_str(match tok {
                "kizuna_app" if !qualified => &roles.app,
                "kizuna" if !qualified => &roles.owner,
                _ => tok,
            });
            i = j;
            continue;
        }
        // Copy one whole UTF-8 character.
        let len = sql[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&sql[i..i + len]);
        i += len;
    }
    out
}

/// The embedded migrations with role names mapped. Checksums stay those of the files, so databases migrated
/// under any role mapping keep validating (sqlx `Migrator` fields are pinned by Cargo.lock to 0.8).
pub fn migrator(roles: &Roles) -> sqlx::migrate::Migrator {
    let migrations: Vec<sqlx::migrate::Migration> = MIGRATOR
        .migrations
        .iter()
        .map(|m| sqlx::migrate::Migration {
            sql: std::borrow::Cow::Owned(map_roles(&m.sql, roles)),
            ..m.clone()
        })
        .collect();
    sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(migrations),
        ignore_missing: MIGRATOR.ignore_missing,
        locking: MIGRATOR.locking,
        no_tx: MIGRATOR.no_tx,
    }
}

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

/// Read-only transaction in a supplier portal scope: one supplier of one tenant (ADR 0023). `kizuna.tenant_id` is
/// deliberately NOT set, so tenant policies match nothing, and the restrictive `portal_*` policies open only the
/// supplier's own approved requests, its uploads and its supplier record.
pub async fn portal_tx(pool: &PgPool, tenant: Uuid, supplier: Uuid) -> Result<Tx, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION READ ONLY").execute(&mut *tx).await?;
    sqlx::query(
        "SELECT set_config('kizuna.portal_tenant_id', $1, true), set_config('kizuna.portal_supplier_id', $2, true)",
    )
    .bind(tenant.to_string())
    .bind(supplier.to_string())
    .execute(&mut *tx)
    .await?;
    Ok(tx)
}

/// Read-only snapshot transaction (REPEATABLE READ) for consistent multi-statement reads such as audit verification.
pub async fn tenant_snapshot_tx(pool: &PgPool, tenant: Uuid) -> Result<Tx, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    set_tenant(&mut tx, tenant).await?;
    Ok(tx)
}

#[cfg(test)]
mod role_tests {
    use super::*;

    #[test]
    fn default_roles_leave_migrations_unchanged() {
        for m in MIGRATOR.migrations.iter() {
            assert_eq!(map_roles(&m.sql, &Roles::default()), m.sql, "migration {}", m.version);
        }
    }

    #[test]
    fn custom_roles_replace_identifiers_only() {
        let r = Roles {
            owner: "kizuna_pilot_owner".into(),
            app: "kizuna_pilot_app".into(),
        };
        let sql = "-- Owner: kizuna (don't). Runtime: kizuna_app\nGRANT SELECT ON users TO kizuna_app;\n\
                   CREATE POLICY p ON t FOR SELECT TO kizuna USING (current_setting('kizuna.tenant_id') = 'kizuna');\n\
                   SELECT kizuna_tenant FROM kizuna.x; -- 'é'";
        let out = map_roles(sql, &r);
        assert!(out.contains("TO kizuna_pilot_app;"));
        assert!(out.contains("FOR SELECT TO kizuna_pilot_owner USING"));
        assert!(
            out.contains("'kizuna.tenant_id') = 'kizuna'"),
            "string literals untouched: {out}"
        );
        assert!(
            out.contains("kizuna_tenant FROM kizuna.x"),
            "other identifiers untouched"
        );
        assert!(out.starts_with("-- Owner: kizuna (don't)"), "comments untouched");
        assert!(out.ends_with("'é'"));
        let all: String = migrator(&r).migrations.iter().map(|m| m.sql.to_string()).collect();
        assert!(
            !all.contains("TO kizuna_app") && !all.contains("TO kizuna "),
            "every grant/policy mapped"
        );
        for (a, b) in migrator(&r).migrations.iter().zip(MIGRATOR.migrations.iter()) {
            assert_eq!(a.checksum, b.checksum);
        }
    }

    #[test]
    fn role_names_validated() {
        let get = |o: &'static str, a: &'static str| {
            move |k: &str| match k {
                "KIZUNA_DB_OWNER_ROLE" => Some(o.to_string()),
                "KIZUNA_DB_APP_ROLE" => Some(a.to_string()),
                _ => None,
            }
        };
        assert_eq!(Roles::from_lookup(|_| None).unwrap(), Roles::default());
        assert!(Roles::from_lookup(get("kizuna_pilot_owner", "kizuna_pilot_app")).is_ok());
        assert!(Roles::from_lookup(get("x; DROP TABLE users", "a")).is_err());
        assert!(Roles::from_lookup(get("same", "same")).is_err());
        assert!(Roles::from_lookup(get("Upper", "a")).is_err());
    }
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

/// Default tenant timezone (the pilot customer is in Japan).
pub const DEFAULT_TIMEZONE: &str = "Asia/Tokyo";

/// The tenant's IANA timezone (latest `general` tenant setting; RLS scope of `tx`).
pub async fn tenant_timezone(tx: &mut Tx) -> Result<String, sqlx::Error> {
    let tz: Option<String> = sqlx::query_scalar(
        "SELECT settings->>'timezone' FROM tenant_settings WHERE kind = 'general' ORDER BY version DESC LIMIT 1",
    )
    .fetch_optional(&mut **tx)
    .await?
    .flatten();
    Ok(tz.unwrap_or_else(|| DEFAULT_TIMEZONE.into()))
}

/// "Today" in the tenant's calendar at `now` (from the injected clock); DST-aware via PostgreSQL's tz database.
pub async fn tenant_today(tx: &mut Tx, tenant: Uuid, now: time::OffsetDateTime) -> Result<time::Date, sqlx::Error> {
    set_tenant(tx, tenant).await?;
    let tz = tenant_timezone(tx).await?;
    sqlx::query_scalar("SELECT ($1::timestamptz AT TIME ZONE $2)::date")
        .bind(now)
        .bind(tz)
        .fetch_one(&mut **tx)
        .await
}

/// Start of a tenant-local calendar day as an instant.
pub async fn tenant_midnight(tx: &mut Tx, day: time::Date) -> Result<time::OffsetDateTime, sqlx::Error> {
    let tz = tenant_timezone(tx).await?;
    sqlx::query_scalar("SELECT ($1::date::timestamp AT TIME ZONE $2)")
        .bind(day)
        .bind(tz)
        .fetch_one(&mut **tx)
        .await
}

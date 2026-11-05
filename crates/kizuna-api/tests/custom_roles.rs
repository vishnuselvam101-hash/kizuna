#![allow(clippy::unwrap_used)]
//! Migrations under custom role names (pilot: separate roles from dev). Needs a database owned by a second role
//! pair: KIZUNA_TEST_ALT_OWNER_DATABASE_URL / KIZUNA_TEST_ALT_DATABASE_URL (scripts/ci-local.sh and CI create them).
//! Without them the test is skipped, unless KIZUNA_REQUIRE_ALT_ROLES=1 (CI, ci-local), where it fails.

use kizuna_api::db::{self, Roles};
use uuid::Uuid;

fn user_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap()
        .split([':', '@'])
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn migrations_grants_and_rls_work_under_custom_role_names() {
    let (Ok(owner_url), Ok(app_url)) = (
        std::env::var("KIZUNA_TEST_ALT_OWNER_DATABASE_URL"),
        std::env::var("KIZUNA_TEST_ALT_DATABASE_URL"),
    ) else {
        assert!(
            std::env::var("KIZUNA_REQUIRE_ALT_ROLES").as_deref() != Ok("1"),
            "KIZUNA_TEST_ALT_* not set but required"
        );
        eprintln!("SKIPPED: KIZUNA_TEST_ALT_* not set (scripts/ci-local.sh runs this test)");
        return;
    };
    let roles = Roles {
        owner: user_of(&owner_url),
        app: user_of(&app_url),
    };
    assert_ne!(roles, Roles::default(), "the alt database must use non-default roles");
    let owner = db::connect(&owner_url, 2).await.unwrap();
    db::migrator(&roles).run(&owner).await.unwrap();
    // Idempotent; the default-named migrator validates too (same checksums).
    db::migrator(&roles).run(&owner).await.unwrap();
    let app = db::connect(&app_url, 2).await.unwrap();
    db::assert_runtime_role(&app).await.unwrap();

    // Grants went to the custom runtime role, never to the dev role names.
    let q = |sql: &'static str| sqlx::query_scalar::<_, bool>(sql);
    let has = |role: &str, t: &str, p: &str| {
        let (role, t, p) = (role.to_string(), t.to_string(), p.to_string());
        let owner = owner.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = $1) AND has_table_privilege($1, $2, $3)",
            )
            .bind(role)
            .bind(t)
            .bind(p)
            .fetch_one(&owner)
            .await
            .unwrap()
        }
    };
    assert!(has(&roles.app, "users", "SELECT").await);
    assert!(has(&roles.app, "security_events", "INSERT").await);
    assert!(!has(&roles.app, "security_events", "DELETE").await);
    assert!(
        !has("kizuna_app", "users", "SELECT").await,
        "dev runtime role has no rights here"
    );
    assert!(
        !has("kizuna", "users", "SELECT").await,
        "dev owner role has no rights here"
    );
    let owner_name: String = sqlx::query_scalar("SELECT tableowner::text FROM pg_tables WHERE tablename = 'users'")
        .fetch_one(&owner)
        .await
        .unwrap();
    assert_eq!(owner_name, roles.owner);
    assert!(
        q("SELECT bool_and(relforcerowsecurity) FROM pg_class WHERE relname IN ('users','security_events','artifacts')")
            .fetch_one(&owner)
            .await
            .unwrap()
    );

    // RLS under the custom names: tenant isolation for the runtime role, platform_read policy for the owner.
    let (t1, t2) = (Uuid::now_v7(), Uuid::now_v7());
    for t in [t1, t2] {
        sqlx::query("INSERT INTO tenants (id, slug, name) VALUES ($1, $2, 'Alt')")
            .bind(t)
            .bind(format!("alt-{}", t.simple()))
            .execute(&owner)
            .await
            .unwrap();
    }
    let mut tx = db::tenant_tx(&app, t1).await.unwrap();
    sqlx::query(
        "INSERT INTO security_events (id, tenant_id, kind, severity) VALUES ($1, $2, 'auth.login_failed', 'low')",
    )
    .bind(Uuid::now_v7())
    .bind(t1)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let count = |t: Uuid| {
        let app = app.clone();
        async move {
            let mut tx = db::tenant_tx(&app, t).await.unwrap();
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM security_events WHERE tenant_id = $1")
                .bind(t1)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            n
        }
    };
    assert_eq!(count(t1).await, 1);
    assert_eq!(count(t2).await, 0, "other tenant sees nothing");
    let mut tx = db::tenant_tx(&app, t2).await.unwrap();
    let cross = sqlx::query("INSERT INTO security_events (id, tenant_id, kind, severity) VALUES ($1, $2, 'x', 'low')")
        .bind(Uuid::now_v7())
        .bind(t1)
        .execute(&mut *tx)
        .await;
    assert!(cross.is_err(), "cross-tenant write refused by RLS");
    drop(tx);
    let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM security_events WHERE tenant_id = $1")
        .bind(t1)
        .fetch_one(&owner)
        .await
        .unwrap();
    assert_eq!(seen, 1, "owner platform_read policy mapped to the custom owner role");
}

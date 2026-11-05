#![allow(clippy::unwrap_used)]
mod common;

use axum::http::{Method, StatusCode};
use common::*;
use kizuna_api::db;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn rbac_denies_by_default_and_logs() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, viewer) = c.user(t, "viewer").await;
    let (_, auditor) = c.user(t, "auditor").await;
    let v = c.token(&slug, &viewer).await;
    let a = c.token(&slug, &auditor).await;
    for path in [
        "/api/v1/users",
        "/api/v1/audit/events",
        "/api/v1/audit/verify",
        "/api/v1/security-events",
    ] {
        assert_eq!(
            c.get(path, &v).await.status,
            StatusCode::FORBIDDEN,
            "viewer {path}"
        );
        assert_eq!(
            c.get(path, &a).await.status,
            StatusCode::OK,
            "auditor {path}"
        );
    }
    let r = c
        .post(
            "/api/v1/users",
            Some(&a),
            json!({"email":"y@example.test","display_name":"Y","role":"viewer","password":PW}),
        )
        .await;
    assert_eq!(
        (r.status, r.body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("role_lacks_permission"))
    );
    assert!(
        c.security_kinds(t)
            .await
            .contains(&"authz.denied".to_string())
    );
}

#[tokio::test]
async fn admin_cannot_touch_other_tenant() {
    let c = ctx().await;
    let (_, _, _, tok_a) = c.admin().await;
    let (tb, _) = c.tenant().await;
    let (ub, _) = c.user(tb, "viewer").await;
    let r = c
        .call(
            Method::PATCH,
            &format!("/api/v1/users/{ub}"),
            Some(&tok_a),
            Some(json!({"status": "disabled"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(
        c.post(
            &format!("/api/v1/users/{ub}/revoke-sessions"),
            Some(&tok_a),
            json!({})
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        c.post(
            &format!("/api/v1/users/{ub}/reset-mfa"),
            Some(&tok_a),
            json!({})
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    let list = c.get("/api/v1/users", &tok_a).await;
    assert!(!list.body.to_string().contains(&ub.to_string()));
    let st: String = sqlx::query_scalar("SELECT status FROM users WHERE id = $1")
        .bind(ub)
        .fetch_one(&mut *db::tenant_tx(&c.owner, tb).await.unwrap())
        .await
        .unwrap();
    assert_eq!(st, "active");
}

#[tokio::test]
async fn rls_isolates_at_sql_level_for_runtime_role() {
    let c = ctx().await;
    let (ta, _) = c.tenant().await;
    let (tb, _) = c.tenant().await;
    c.user(ta, "viewer").await;
    let (ub, _) = c.user(tb, "viewer").await;
    let pool = &c.st.pool;

    // No tenant context: nothing visible.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    // Tenant A context cannot see B's user even by id.
    let mut tx = db::tenant_tx(pool, ta).await.unwrap();
    let seen: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE id = $1")
        .bind(ub)
        .fetch_optional(&mut *tx)
        .await
        .unwrap();
    assert!(seen.is_none());
    // Writing a row for B under A's context is rejected by WITH CHECK.
    let e = sqlx::query("INSERT INTO users (id, tenant_id, email, display_name, password_hash, role) VALUES ($1,$2,'z@x.y','Z','h','viewer')")
        .bind(Uuid::now_v7())
        .bind(tb)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("row-level security"), "{e}");
    tx.rollback().await.unwrap();

    // Runtime role cannot rewrite history or create tenants.
    let mut tx = db::tenant_tx(pool, ta).await.unwrap();
    let e = sqlx::query("DELETE FROM audit_events")
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("permission denied"), "{e}");
    tx.rollback().await.unwrap();
    let e = sqlx::query("INSERT INTO tenants (id, slug, name) VALUES ($1, 'evil-tenant', 'x')")
        .bind(Uuid::now_v7())
        .execute(pool)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("permission denied"), "{e}");
    let e = sqlx::query("UPDATE security_events SET severity = 'info'")
        .execute(pool)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("permission denied"), "{e}");
    // Security events of tenant B are invisible to A.
    let mut tx = db::tenant_tx(pool, ta).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM security_events WHERE tenant_id = $1")
        .bind(tb)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn runtime_role_check() {
    let c = ctx().await;
    db::assert_runtime_role(&c.st.pool).await.unwrap();
    assert!(
        db::assert_runtime_role(&c.owner).await.is_err(),
        "owner role must be refused"
    );
}

#[tokio::test]
async fn last_admin_guard_and_role_change_revokes_sessions() {
    let c = ctx().await;
    let (t, slug, admin_id, tok) = c.admin().await;
    let r = c
        .call(
            Method::PATCH,
            &format!("/api/v1/users/{admin_id}"),
            Some(&tok),
            Some(json!({"role": "viewer"})),
        )
        .await;
    assert_eq!(
        (r.status, r.body["error"].as_str()),
        (StatusCode::CONFLICT, Some("last_active_admin"))
    );
    let r = c
        .call(
            Method::PATCH,
            &format!("/api/v1/users/{admin_id}"),
            Some(&tok),
            Some(json!({"status": "disabled"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);

    let (uid, email) = c.user(t, "analyst").await;
    let utok = c.token(&slug, &email).await;
    let r = c
        .call(
            Method::PATCH,
            &format!("/api/v1/users/{uid}"),
            Some(&tok),
            Some(json!({"role": "reviewer"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(
        c.get("/api/v1/auth/me", &utok).await.status,
        StatusCode::UNAUTHORIZED,
        "stale privileges revoked"
    );
    let r = c
        .call(
            Method::PATCH,
            &format!("/api/v1/users/{uid}"),
            Some(&tok),
            Some(json!({"role": "superuser"})),
        )
        .await;
    assert_eq!(r.body["error"], "invalid_role");
    let r = c
        .call(
            Method::PATCH,
            &format!("/api/v1/users/{uid}"),
            Some(&tok),
            Some(json!({"status": "disabled"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        c.login(&slug, &email, PW, None).await.status,
        StatusCode::UNAUTHORIZED
    );
    let kinds = c.security_kinds(t).await;
    assert!(
        kinds.contains(&"admin.role_changed".to_string())
            && kinds.contains(&"admin.user_disabled".to_string())
    );
}

#[tokio::test]
async fn admin_revokes_sessions_and_resets_mfa() {
    let c = ctx().await;
    let (t, slug, admin_id, tok) = c.admin().await;
    let (uid, email) = c.user(t, "reviewer").await;
    let utok = c.token(&slug, &email).await;
    c.enroll_totp(&utok).await;
    let r = c
        .post(
            &format!("/api/v1/users/{uid}/revoke-sessions"),
            Some(&tok),
            json!({}),
        )
        .await;
    assert_eq!(r.body["revoked"], 1);
    assert_eq!(
        c.get("/api/v1/auth/me", &utok).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        c.post(
            &format!("/api/v1/users/{admin_id}/reset-mfa"),
            Some(&tok),
            json!({})
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        c.post(
            &format!("/api/v1/users/{uid}/reset-mfa"),
            Some(&tok),
            json!({})
        )
        .await
        .status,
        StatusCode::OK
    );
    assert_eq!(
        c.login(&slug, &email, PW, None).await.status,
        StatusCode::OK,
        "MFA no longer required after reset"
    );
}

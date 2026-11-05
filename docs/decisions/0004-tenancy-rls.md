# ADR 0004: Tenant isolation with PostgreSQL RLS (FORCE), separate owner and runtime roles

Status: accepted (2026-11-05)

## Decision
- Every tenant-scoped table has `tenant_id uuid NOT NULL`, `ENABLE` + `FORCE ROW LEVEL SECURITY` and a policy
  `tenant_id = kz_tenant_id()`, where `kz_tenant_id()` reads the transaction-local setting `kizuna.tenant_id`.
  Without a tenant context, no row is visible and no row can be inserted.
- The API sets the context with `set_config('kizuna.tenant_id', $1, true)` at the start of every transaction
  (`db::tenant_tx`). Pool connections therefore never carry a tenant between requests.
- Roles: `kizuna` owns schema and runs migrations (LOGIN, no superuser, no BYPASSRLS). `kizuna_app` is the runtime
  role (LOGIN, NOBYPASSRLS, not owner) with per-table least-privilege grants; append-only tables get SELECT+INSERT only.
- The server refuses to start if its role is superuser, has BYPASSRLS or owns the schema (`db::assert_runtime_role`).
- `tenants` is a directory table without RLS; runtime may SELECT (slug lookup at login) but not modify.
- Authorization in Rust (policy-engine) still checks tenant equality: RLS is the second wall, not the only one.

## Consequences
Bugs in Rust query filters cannot leak cross-tenant rows. Cross-tenant platform operations (tenant creation) run via the
owner role in `kizuna-admin`, never via HTTP.

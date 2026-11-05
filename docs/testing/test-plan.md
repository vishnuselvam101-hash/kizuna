# Test plan

| Level | Tooling | Scope | Where |
|---|---|---|---|
| Unit | `cargo test` | engines: canonical encoding, chain verification, RBAC table, TOTP vectors, token parsing, rate limiter, config | `crates/*/src` |
| Property | proptest | audit chain: any single-byte/field mutation breaks verification; canonical JSON order-independence; token parse never panics | `crates/audit-engine`, `crates/kizuna-api` |
| Integration (real PostgreSQL) | `cargo test -p kizuna-api --test '*'` against `kizuna_test` | login/MFA/refresh/reuse/logout, RBAC denials, tenant isolation (RLS) at SQL and API level, audit append + tamper detection, security events, rate limits, headers, body limits, runtime-role safety | `crates/kizuna-api/tests` |
| Python | pytest, ruff | extraction worker (M3) incl. malformed files, resource limits, prompt-injection text treated as data | `services/extraction/tests` |
| Frontend | vitest, tsc | API client, security-sensitive state (token storage) | `web/src` |
| E2E | Rust integration + synthetic data | M6 manufacturing workflows with injected faults | `crates/kizuna-api/tests/e2e_*` |
| Benchmarks | release builds, recorded JSON | M6 | `reports/` |

Rules: integration tests require `KIZUNA_TEST_DATABASE_URL` (runtime role) and `KIZUNA_TEST_OWNER_DATABASE_URL`
(owner role); they **fail** (never skip) when unset. Each test creates its own tenants with random slugs, so tests run
in parallel on one database. Every confirmed vulnerability gets a regression test named `regression_<id>_*`.

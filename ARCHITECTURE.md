# Architecture

Modular monolith (ADR 0001). Pure engine crates hold rules; `kizuna-api` does I/O.

```
web (React/TS) ──HTTPS/JSON──► kizuna-api (axum)
                                ├─ auth / sessions / MFA ──► policy-engine (RBAC+ABAC)
                                ├─ audit (hash chain) ─────► audit-engine
                                ├─ security_events
                                ├─ PostgreSQL (runtime role kizuna_app, RLS FORCE per tenant)
                                ├─ object store (filesystem, trait)
                                └─ extraction worker (python subprocess,       [M3]
                                   no DB creds, no network, rlimits)
```

## Directive §6 module map
| Directive module | Location | State |
|---|---|---|
| audit-engine | `crates/audit-engine` | M1 done |
| policy-engine | `crates/policy-engine` | M1 done |
| security-events | `kizuna-api/src/security_events.rs` | M1 recording; detection M5 |
| authentication & identity, tenant management | `kizuna-api/src/{auth,users}.rs`, `kizuna-admin` | M1 done |
| evidence-core (file types, scan, claim validation), claim-model, provenance, integrity | `crates/evidence-core`, `kizuna-api/src/{artifacts,claims,storage}.rs` | M2 done |
| entity-resolution, conflict-engine, review workflows | planned M3 | not started |
| temporal-engine, dependency-engine, decision-engine | planned M4 | not started |
| notification service, connectors (ERP/PLM/QMS/MES) | interfaces only, later | not started |

## Request path
1. Bearer token parsed (`kza.<tenant>.<secret>`), tenant transaction opened (`set_config('kizuna.tenant_id')`).
2. Session looked up by SHA-256(secret); revoked/expired/disabled → 401.
3. Handler calls `Principal::require(permission)` → policy-engine; deny → 403 + `authz.denied` security event.
4. Data access in the same tenant-scoped transaction; state changes append an audit event in that transaction.

## Data
Schema in `migrations/`; claim model design in `docs/data-model/claims.md`.
Roles: `kizuna` (owner/migrations), `kizuna_app` (runtime, least privilege). Append-only tables: `audit_events`,
`security_events` (grants + triggers).

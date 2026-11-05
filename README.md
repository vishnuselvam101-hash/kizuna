# KIZUNA Verify

Industrial evidence and decision-integrity platform (first product: supplier/part specification verification for
manufacturers). Governing document: [DIRECTIVE.md](DIRECTIVE.md). Current state: [BUILD_STATUS.md](BUILD_STATUS.md).

Not production-ready. Synthetic data only.

## Layout
| Path | What |
|---|---|
| `crates/audit-engine` | pure hash-chained audit encoding/verification |
| `crates/policy-engine` | deny-by-default RBAC + ABAC hooks |
| `crates/evidence-core`, `entity-resolution`, `conflict-engine`, `dependency-engine`, `security-detection` | pure engines (upload scan, claims, identity, conflicts, invalidation, detection) |
| `crates/domain-manufacturing` | manufacturing domain pack (record mapping, rules) |
| `crates/kizuna-api` | axum API server (`kizuna-server`) and operator CLI (`kizuna-admin`) |
| `crates/kizuna-bench` | benchmarks and restore verification |
| `migrations/` | PostgreSQL schema (RLS FORCE on tenant tables) |
| `services/extraction` | Python extraction worker (uv) |
| `web/` | TypeScript/React UI (pnpm) |
| `docs/` | decisions (ADRs), research, security, testing, data model |

## Quick start (local)
```sh
scripts/db-setup.sh                      # roles kizuna/kizuna_app, DBs kizuna_dev/kizuna_test, writes .env.local
set -a; source .env.local; set +a
cargo run -p kizuna-api --bin kizuna-admin -- migrate
cargo run -p kizuna-api --bin kizuna-admin -- create-tenant acme "Acme Precision"
echo 'a long admin passphrase' | cargo run -p kizuna-api --bin kizuna-admin -- create-user acme admin@acme.test tenant_admin "Admin"
cargo run -p kizuna-api --bin kizuna-server          # 127.0.0.1:8080
(cd web && pnpm install && pnpm dev)                 # 127.0.0.1:5173, proxies /api
scripts/check.sh                                      # full gate
scripts/e2e.sh --seed 7                               # synthetic end-to-end run with injected faults
scripts/fuzz.sh                                       # property-based fuzzing of all input parsers
```

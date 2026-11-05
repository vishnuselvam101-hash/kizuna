# Build status

Updated: 2026-11-05 (end of M1). Environment: Linux, 8 CPU, Rust 1.95.0, PostgreSQL 18.6 (local), Node 24.22.2,
pnpm 12.15.0, Python 3.14.4 + uv.

## Milestones
| M | State | Notes |
|---|---|---|
| M0 | done | ADRs 0001–0010, THREAT_MODEL, claim data model, test plan, research + unverified-claims registers, dependency register |
| M1 | done | auth (argon2id, TOTP, rotating refresh + reuse detection, revocation), tenants, RBAC/ABAC, RLS FORCE, audit chain, security events, rate limits, headers, CI, check.sh |

## Commands run and results (2026-11-05, this machine)
| Command | Result |
|---|---|
| `scripts/db-setup.sh` | roles `kizuna`, `kizuna_app`; DBs `kizuna_dev`, `kizuna_test` created |
| `scripts/check.sh` | pass: fmt, clippy -D warnings, 46 Rust tests (audit-engine 6, policy-engine 5, kizuna-api unit 11, integration m1_audit 6 / m1_auth 12 / m1_authz_tenancy 6), cargo audit (1 ignored, not compiled: RUSTSEC-2023-0071), pnpm audit 0, ruff + 1 pytest, tsc + 2 vitest, gitleaks 0 |
| smoke: `kizuna-admin migrate/create-tenant/create-user`, `kizuna-server` + curl login, `kizuna-admin verify-audit acme` | login 200, chain intact (3 events); server refuses to start with the owner role (`Error: Owner`); server log contained no password/token strings |

## Known defects / gaps
- WebAuthn designed (ADR 0006) but not implemented.
- No breached-password screening; static data key without rotation; in-process rate limiter.
- Session expiry is not tested with an injected clock.
- Python and web are scaffolds (no real extraction or UI screens yet).
- CI workflow written but never executed (no remote, by instruction).

## Next step
M2: artifact ingestion (`artifacts` table, filesystem object store, upload limits + magic-byte allowlist, SHA-256
integrity + verification), claims + provenance tables, evidence APIs, evidence viewer.

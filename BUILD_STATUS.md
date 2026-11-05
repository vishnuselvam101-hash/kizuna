# Build status

Updated: 2026-11-05 (end of M5). Environment: Linux, 8 CPU, Rust 1.95.0, PostgreSQL 18.6 (local), Node 24.22.2,
pnpm 12.15.0, Python 3.14.4 + uv.

## Milestones
| M | State | Notes |
|---|---|---|
| M0 | done | ADRs 0001–0010, THREAT_MODEL, claim data model, test plan, research + unverified-claims registers, dependency register |
| M1 | done | auth (argon2id, TOTP, rotating refresh + reuse detection, revocation), tenants, RBAC/ABAC, RLS FORCE, audit chain, security events, rate limits, headers, CI, check.sh; independent review: 9 findings, 8 fixed with regression tests |
| M2 | done | artifacts (content-typed, static scan, quarantine/release, content-addressed FS store, capture provenance, integrity verify on every read), claims v1 (append-only, supersession, transaction-time `known_at` and valid-time `valid_at` queries), evidence explorer API, React UI (login+MFA, documents, evidence explorer) |
| M3 | done | sandboxed extraction worker (CSV, XLSX, PDF text, tesseract OCR; bubblewrap + prlimit), `mfg-map@1` record→claim mapping, entity resolution (safe normalisation + human-confirmed aliases), 9 conflict kinds (`mfg-rules@1`), findings with sources/limitations, review workflow (approve/reject/request evidence, four-eyes on manual claims), revision comparison, UI for findings/review/revisions |
| M4 | done | decisions with derivation snapshots + SHA-256, typed dependencies (claim/decision, applicability, materiality), selective invalidation hook on supersession/rejection/new evidence incl. earlier claim versions, propagation through decision edges, invalidation events, traces (known-at/changed-since/reproducibility), reconfirm/revoke, verification report (JSON/Markdown), decisions UI |
| M5 | done | detection rules (10), incidents, pre-authorised vs approval-gated containment (sessions, account, address, artifact quarantine, uploads, extraction), guards (last admin, admin address), revert/resolve, emergency unlock CLI, enforcement at login/upload/extraction, signed Ed25519 audit checkpoints (closes M1 #6 when exported), artifact self-repair on re-upload, security dashboard UI |

## Commands run and results (2026-11-05, this machine)
| Command | Result |
|---|---|
| `scripts/db-setup.sh` | roles `kizuna`, `kizuna_app`; DBs `kizuna_dev`, `kizuna_test` created |
| `scripts/check.sh` | pass: fmt, clippy -D warnings, 46 Rust tests (audit-engine 6, policy-engine 5, kizuna-api unit 11, integration m1_audit 6 / m1_auth 12 / m1_authz_tenancy 6), cargo audit (1 ignored, not compiled: RUSTSEC-2023-0071), pnpm audit 0, ruff + 1 pytest, tsc + 2 vitest, gitleaks 0 |
| smoke: `kizuna-admin migrate/create-tenant/create-user`, `kizuna-server` + curl login, `kizuna-admin verify-audit acme` | login 200, chain intact (3 events); server refuses to start with the owner role (`Error: Owner`); server log contained no password/token strings |

## Run at end of M2
`scripts/check.sh` → pass: fmt, clippy -D warnings, **72 Rust tests** (audit-engine 6, evidence-core 9, policy-engine 5,
kizuna-api unit 15, integration m1_audit 6 / m1_auth 12 / m1_authz_tenancy 6 / m1_review_regressions 6 /
m2_evidence 7), cargo audit (1 documented ignore), pnpm audit 0, ruff + pytest 1, tsc + vitest 4, gitleaks 0.

## Run at end of M5
`scripts/check.sh` → pass: Rust all green incl. `m5_defense` 8 tests (token theft, spraying block + revert, admin
address guard, last-admin refusal + emergency unlock, integrity quarantine + recovery, upload/authz containment,
checkpoint detects full re-chain by DB owner, RBAC/isolation), Python 11, web 7, audits 0 open, gitleaks 0.

## Run at end of M4
`scripts/check.sh` → pass. Rust **103 tests** (adds dependency-engine 5 incl. proptest flagged-set = ground truth on
random DAGs, `m4_decisions` 6 integration tests), Python 11, web 6, audits 0 open, gitleaks 0.

## M4 self-review notes
Hooks run inside the evidence-changing transaction (no lost flags); propagation loads all decision→decision edges of
a tenant; markdown report escapes `|`, `<` and newlines in user text; decisions/dependencies/invalidation events are
immutable at DB level (trigger + grants).

## Run at end of M3
`scripts/check.sh` → pass. Rust **92 tests** (adds entity-resolution, conflict-engine, domain-manufacturing unit tests
and `m3_verification` 6 integration tests that run the real bubblewrap-sandboxed worker on generated PDF/CSV
documents), Python **11** (CSV/XLSX/PDF/OCR extraction, DTD refusal, malformed PDFs, prompt-injection-as-data,
sandbox hardening), web **5**, audits 0 open, gitleaks 0.
Accuracy of extraction/ER/conflict rules has **not** been measured on real or labelled data yet (M6 synthetic benchmark).

## M3 self-review notes
Worker output is treated as untrusted (fixed vocabulary mapping + claim validation); extraction is synchronous within
the request (45 s cap, 2 concurrent workers); detection loads all current part claims of a tenant into memory;
static PDF markers can be obfuscated (sandbox is the second line); scanned PDFs are not OCRed.

## Security review M1 (independent read-only reviewer, commit 448f97c)
| # | Sev | Finding | Status |
|---|---|---|---|
| 1 | High | Leftmost X-Forwarded-For trusted → rate-limit bypass, forged IPs in logs | fixed: `KIZUNA_TRUSTED_PROXY_HOPS`, right-most trusted hop; test `regression_m1r1_*` + unit test |
| 2 | High | Login held row lock + pooled connection during argon2; unbounded argon2 concurrency | fixed: hash outside any lock, re-check under lock after; semaphore (= CPU count, 5 s wait → 429). No load test yet |
| 3 | Med | TOTP enrolment by a password-only session yielded an MFA-verified session | fixed: setup requires password; enrolment never upgrades the session; test `regression_m1r3_*`, `totp_mfa_flow_and_replay` |
| 4 | Med | `change_password` unlimited password oracle | fixed: re-auth failures count to lockout, lock revokes sessions; test `regression_m1r4_*` |
| 5 | Med | Audit verify race → false `audit_chain_broken`; whole chain in memory | fixed: REPEATABLE READ snapshot, batched verification (1000/batch); test `regression_m1r5_*` |
| 6 | Med | Unkeyed chain + head in same DB: DB owner can rewrite and re-chain | fixed in M5: signed checkpoints; test `signed_checkpoints_detect_full_rechain_by_db_owner` (requires exporting checkpoints and keeping the key out of the DB) |
| 7 | Low | Tenant enumeration via 429 vs 401 | fixed: account limiter keyed by slug+email before lookup; test `regression_m1r7_*` |
| 8 | Low | No admin unlock path | fixed: PATCH `status: active` clears lockout; test `regression_m1r8_*` |
| 9 | Low | Limiter O(n) prune under key flood | fixed: key cap 100k, prune throttled, fail closed for new keys; unit test |

## M2 self-review notes
Upload memory bound = upload limit × concurrent uploads (per-user rate limit 30/min); objects written before DB commit
can be orphaned on rollback (content-addressed, harmless, no GC yet); static scan is not antivirus (ADR 0011).

## Auth hardening lane (branch `lane-auth`, 2026-11-05)
WebAuthn (webauthn-rs 0.6.2) with recovery codes / peer reset / operator CLI, offline common-password screening,
versioned data keys + `kizuna-admin rotate-data-key`, injected clock; migration `0006_auth_hardening.sql`.
`scripts/check.sh`: fmt, clippy -D warnings, **119 Rust tests** (new: `auth_hardening` 9 integration, 8 unit),
cargo audit 0 (425 crates), Python/web pass; gitleaks dir scan of changed paths 0 (`gitleaks git` scans all refs and
reports findings from other lanes' commits). Independent review: 2 findings fixed (enrolment/login downgrade race guard,
unbounded challenge rows) with tests; the race guard in `complete_login` is SQL-level and not reproducible over HTTP.
CLI smoke: `rotate-data-key` exits 2 while any secret is unreadable/stale, `unknown tenant` on a bad slug.

## Known defects / gaps
- WebAuthn verified only with a software authenticator (no real-hardware or browser test yet); attestation not enforced.
- Password screening uses a bundled 46k common-password list, not a full breach corpus. In-process rate limiter.
- Data keys come from env (no KMS/HSM); rotation is an operator-run CLI job.
- Python extraction is still a scaffold (M3).
- No orphan-object garbage collection; no bulk integrity sweep job yet (M5).
- CI workflow written but never executed (no remote, by instruction).

## Next step
M6: integrate parallel lanes (m6-prep, lane-python, lane-web, lane-auth), E2E, benchmarks, backup/restore,
fuzzing, final security review.

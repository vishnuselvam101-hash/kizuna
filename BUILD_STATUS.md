# Build status

Updated: 2026-11-05 (end of M6, integrated `main`). Environment: Linux, 8 CPU, Rust 1.95.0, PostgreSQL 18.6 (local),
Node 24.22.2 / pnpm 12.15.0, Python 3.14 + uv. No remote (by instruction); CI workflow written but never executed.

**Not production-ready.** Synthetic data only; pilot-readiness needs the founder decisions below.

## Milestones
| M | State | Summary |
|---|---|---|
| M0 | done | ADRs 0001–0015, threat model, claim data model, test plan, research + unverified-claims registers, dependency register |
| M1 | done | argon2id, TOTP, rotating refresh + reuse detection, revocation, tenants, deny-by-default RBAC/ABAC, RLS FORCE with separate owner/runtime roles, hash-chained audit, security events, rate limits, headers, CI, `scripts/check.sh` |
| M2 | done | content-typed uploads, static scan + quarantine, content-addressed store, capture provenance, integrity on every read, append-only bitemporal claims, evidence explorer |
| M3 | done | sandboxed Python extraction (CSV, XLSX, PDF text/tables, scanned-PDF OCR, images, DXF title blocks; bubblewrap + prlimit), `mfg-map@1`, blocked entity resolution with human-confirmed aliases, 9 conflict kinds (`mfg-rules@1`), review workflow with four-eyes |
| M4 | done | decisions with derivation hashes, typed dependencies with materiality, selective invalidation incl. earlier claim versions and decision chains, traces, reconfirm/revoke, signed verification reports |
| M5 | done | 10 detection rules, incidents, pre-authorised vs approval-gated containment, guards, revert/resolve, emergency unlock, Ed25519 audit checkpoints, security monitoring screen |
| M6 | done (pilot-ready demo, not production) | synthetic E2E, benchmarks, encrypted backup + restore drill, deployment + demo docs, WebAuthn/password screening/key rotation/injected clock (lane-auth), full UI (lane-web), API gaps, fuzzing, scans, two independent security reviews fixed |

## Commands run and results (2026-11-05, this host, final `main`)
| Command | Result |
|---|---|
| `scripts/check.sh` | **pass**: fmt; clippy `-D warnings`; **156 Rust tests** (unit + property + integration against real PostgreSQL incl. the real bubblewrap-sandboxed worker); cargo audit 0 vulnerabilities (1 documented ignore of an uncompiled crate); pnpm audit 0; pip-audit 0; ruff + **48 pytest**; tsc + **29 vitest** (incl. axe a11y); gitleaks 0 (35 exact fingerprints allowlisted: invalid tokens of dropped E2E DBs, see `.gitleaksignore`) |
| `scripts/fuzz.sh 20000` | pass in 142 s: every proptest property (scanner, ZIP reader, PDF name decoding, tokens, canonical JSON, decimals, claim validation, record mapping, quantity/tolerance/date parsers, ER blocking ≡ all-pairs, invalidation ≡ ground truth) at 20 000 cases + Python worker tests. cargo-fuzz not installed |
| `scripts/e2e.sh --seed 7` / `--seed 11 --parts 48` | pass: rule-finding precision 1.00, fault recall 1.00 (25/25 and seed-11 set), malicious uploads 3/3 refused/quarantined, unauthorised requests 10/10 denied (`reports/e2e/`) |
| `scripts/bench.sh` | `reports/bench/bench-20261105T1432Z.md` — MEASURED on a shared, loaded host; ER candidates 5 000 keys P50 41 ms (was ~3.6 s for 2 000 keys before blocking) |
| `scripts/backup.sh` + `scripts/restore-drill.sh --tamper-test` | pass: restore 1.1 s, verify 0.07 s, 54/54 artifacts intact, tampered object detected (`reports/drill/drill-20261105T145442Z.json`) |
| semgrep (p/rust, p/python, p/typescript, p/secrets) | 5 findings, all triaged as non-issues (CLI `env::args`, temp dirs in tests/bench) |
| trivy fs (vuln, secret, misconfig) | 0 findings on Cargo.lock, uv.lock, pnpm-lock.yaml |
| clippy pedantic on security crates | lossy casts fixed; remaining are `must_use`/doc-style lints (not enforced) |
| cargo-deny | not installed; licences reviewed manually in `docs/research/dependencies.md` |

## Security reviews
- M1 independent review: 9 findings, all fixed (details below).
- M2–M4 independent review (`docs/security/REVIEW_M2_M4.md`): 22 findings; 20 fixed with regression tests, I3 documented,
  M6 partly fixed (no seccomp yet).
- Final independent review of M5 + lanes (2026-11-05): 7 findings, all fixed with regression tests
  (`tests/final_review_regressions.rs`): H1 detection flood (per-user denial-event limit, non-blocking detection lock,
  newest-first window), M1 incident lifecycle (false positives stay resolved, expired blocks re-applied), M2 tenant-wide
  extraction suspension now needs approval + per-artifact retry limit, M3 no admin decides containment about themselves,
  M4 emergency unlock only re-enables containment-disabled admins, L1 junk checkpoints → 400 without alerts, L2 server
  refuses to run as root (`KIZUNA_ALLOW_ROOT=1` for dev only).
- ASVS 5.0 control-to-test mapping re-run: `docs/security/asvs-m1-mapping.md` (all mapped tests pass).

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

## Known defects / gaps
- No seccomp filter in the extraction sandbox; process limit only when the server runs as a non-root user.
- Static upload scan is not antivirus; `/ObjStm` PDFs are quarantined conservatively (expect false positives).
- Extraction accuracy, ER and conflict rules measured only on synthetic data (E2E); no real supplier documents yet.
- Detection thresholds are static; address blocks are per tenant and login-only; no cross-tenant (platform) correlation.
- Notifications are in-app only. Import settings are stored/versioned but not yet consumed by the extraction worker.
  Web API gaps 2–8 have endpoints; the UI still uses its earlier client-side workarounds for them.
- Benchmarks ran on a busy shared host (upper bounds). CI workflow never executed (no remote).
- Rate limiter is per process (single instance only). Dependency major upgrades pending (sqlx 0.9, rand 0.10, ...).

## Founder decisions needed
1. Make WebAuthn mandatory for tenant administrators (today: optional; once enrolled it is required for that admin).
2. KIZUNA trademark/patent/FTO checks before any public demo (`docs/research/unverified-claims.md`).
3. Pilot hosting, TLS domain, off-host backup/checkpoint storage, and whether e-mail/webhook alerts are allowed.
4. Any external AI provider for extraction (none used; would need customer authorisation + data-processing policy).

## Next step
`scripts/check.sh && scripts/e2e.sh --seed 7`, then on pilot hardware: run as the dedicated `kizuna` user per
`docs/deploy/DEPLOYMENT.md`, add a seccomp profile for the worker, and re-run `scripts/bench.sh` on an idle host.

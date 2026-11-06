# Build status

Updated: 2026-11-05 (M7 on `main`). Environment: Linux, 8 CPU, Rust 1.95.0, PostgreSQL 18.6 (local),
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
| M7 | done | opt-in AI-assisted label mapping with free models (ADR 0018), saved import settings consumed by the worker, per-tenant + adaptive detection thresholds (ADR 0015), web API gaps 2–8 switched to real endpoints, WebAuthn-optional decision recorded (ADR 0006) |
| M7 | done | opt-in AI-assisted label mapping with free models (ADR 0018), saved import settings consumed by the worker, per-tenant + adaptive detection thresholds (ADR 0015), web API gaps 2–8 switched to real endpoints, WebAuthn-optional decision recorded (ADR 0006) |
| I2/I3 | done (synthetic eval only) | certificate integrity signals (9 kinds) + revision drift radar, ADR 0019, migration 0016, `/integrity/*` API, "Integrity signals" and "Revision drift" views; eval docs/testing/INTEGRITY_SIGNALS_EVAL.md |
| M6 | done (pilot-ready demo, not production) | synthetic E2E, benchmarks, encrypted backup + restore drill, deployment + demo docs, WebAuthn/password screening/key rotation/injected clock (lane-auth), full UI (lane-web), API gaps, fuzzing, scans, two independent security reviews fixed |

## Integration results (2026-11-05 late, main 5009a60)
Further merged: feat-evidence-pack (mig 0022, ADR 0023), feat-whatif (0023), qa (ADR 0024: mfg-rules@2, mfg-map@4,
ai-map@3), er-tuning (0024), ja-v2 (0025, mfg-map@5), perf-verify (0026), feat-supplier-portal (0027, ADR 0025),
feat-8d (0028, ADR 0026); integration: 8D requests published to the supplier portal and portal 8D responses linked as
supplier evidence (0029, portal deny-by-default for later tables), CAPA overdue in tenant time zone + alerts.
Portal security review (`docs/security/REVIEW_PORTAL_2026-11-05.md`): High 0, Medium 0, Low 3 — all fixed with tests.

| Command | Result |
|---|---|
| `scripts/ci-local.sh` | **green in 228 s** on 5009a60: **530 Rust tests**, 123 pytest, 101 vitest, audits/deny/gitleaks clean |
| `scripts/e2e.sh --seed 7` / `--seed 11 --parts 48` | pass (precision 1.00, fault recall 1.00) |
| restore drill `--tamper-test` | pass: restore 1.22 s, verify 0.09 s, tamper detected (`reports/drill/drill-20261105T220156Z.json`) |
| `scripts/e2e-browser.sh` (Playwright, from qa) | last full run before selector updates: 11 passed / 18 failed (selectors written for the qa UI); update in progress on a separate branch |

Notes: the what-if 100k-edge test can exceed the 60 s request timeout on a heavily loaded host (passes alone in 7 s);
release builds once reused stale fingerprints after merges — `restore-drill.sh` can take a prebuilt
`KIZUNA_RESTORE_VERIFY_BIN`.

## Integration results (2026-11-05, main c215db0 + this commit: all lanes merged)
Merged: ops-pilot, bughunt, ui-polish, docs-manual, redteam, algo-stats, algo-tolerance, innov-match, innov-signals,
web-gaps, ja-extraction, chaos, algo-er2, algo-invalidation. Renumbered: migrations 0018 match, 0019 signals,
0020 ja_evidence (0021 review_batches kept); ADRs 0018 AI, 0019 quality-stats, 0020 tolerance, 0021 three-way match,
0022 integrity signals.

| Command | Result |
|---|---|
| `scripts/check.sh` | **pass**: **454 Rust tests**, cargo audit/deny ok, pnpm audit 0, pip-audit 0, ruff + **117 pytest**, tsc + **76 vitest** (axe), shellcheck, ops/supply-chain tests, gitleaks 0 |
| `scripts/ci-local.sh` | **green in 177 s** on c215db0 (fresh throwaway DB) |
| `scripts/e2e.sh --seed 7` / `--seed 11 --parts 48` | pass: precision 1.00, fault recall 1.00, malicious uploads 3/3, unauthorised 10/10 (`reports/e2e/`) |
| backup + `scripts/restore-drill.sh --tamper-test` (E2E seed-7 data) | pass: restore 3.28 s, verify 0.09 s, tampered object detected (`reports/drill/drill-20261105T195243Z.json`) |
| JA eval (`services/extraction/tests/ja_eval.py`) | no regression after merging mfg-map@2+@3: worker all 0.938/0.824; rust train 1.000/0.959, all 0.960/0.679 |
| Browser E2E | none in the repo |

Open: ISO 286/ISO 965 tables in `crates/tolerance` typed from memory — must be verified against the standards
(ADR 0020). Shared `kizuna_test` DB is used by several lanes at once (a lane's migration can break others' runs);
ci-local uses a throwaway DB and is the reference gate.

## Integration results (2026-11-05, main after ops-pilot, bughunt, ui-polish merges + two security reviews)
| Command | Result |
|---|---|
| `scripts/check.sh` / `scripts/ci-local.sh` | **pass** (ci-local green in 89 s on 7ba20fe): **226 Rust tests** (1 ignored live-AI test), cargo audit 0, **cargo deny** ok, pnpm audit 0, pip-audit 0 (pinned 2.10.1), ruff + **63 pytest**, tsc + **50 vitest** (axe), shellcheck, ops script tests incl. supply-chain pins, gitleaks 0 |
| Security reviews | `REVIEW_M5_M7.md` (2 M, 8 L, 4 I) and `REVIEW_DEEP_2026-11-05.md` (4 M, 9 L, 7 I): all fixed with regression tests (KZ-I3 accepted) |
| Bughunt | 6 fixes merged + 7 more fixed on main (`bughunt2_regressions.rs`, worker tests) |

Since: extraction uses a short lease row instead of a held connection (8 concurrent extractions pass on a pool of 2);
tenant timezone (IANA, admin screen, audited) drives expiry "today" and report dates; checkpoints are chained (KZCP2);
JA/EN manual merged and updated (idle timeout, re-auth, time zone, re-extract).

## Innovation I6: regulation-list versions as dependency nodes (2026-11-06, branch `inn-reglist`)
ADR 0028, migration 0031, engine `mfg-reglist@1`. Tenants import
substance-list versions, compliance declarations (`reg.declaration` claims linked to the list version answered) and
FMD-style compositions (`reg.composition`); curated material profiles (MFA). A new version assesses every current
declaration (`may_contain` / `cannot_rule_out` / `not_affected`, all stored) and flags dependent decisions via
`dependency_engine::propagate_seeds` (cause `regulation_list_changed`); the I4 planner re-requests the stale
declaration. No shipped list content; tests use invented substances. API: `/reg-lists`, `/reg-lists/{id}/versions`,
`/reg-lists/{declarations,compositions}`, `/material-profiles`. No web screen yet (upload kinds + error texts only).

## Innovation I1 + I4: three-way match and minimal re-request (2026-11-05, branch `innov-match`)
| Command | Result |
|---|---|
| `scripts/check.sh` (own DB `kizuna_test_match`) | **pass**: fmt, clippy `-D warnings`, 241 Rust tests incl. `tests/innov_match.rs` (3, real PostgreSQL + sandboxed extraction), audits 0, 62 pytest, 57 vitest (+7 match/re-request UI incl. axe EN/JA) |
| `scripts/e2e.sh --seed 7` / `--seed 11 --parts 48` | pass. Match case: 30 PO lines, 22 injected mismatches (11 kinds × 2): precision 1.00, recall 1.00 (tp 22, fp 0, fn 0) on both seeds. Re-request: 4 evidence changes → 3 documents requested (all expected) vs 6 under "resend everything"; the approval change asks the supplier for nothing. SYNTHETIC: generator and rules were built together, so this shows the pipeline works on known layouts, not accuracy on real documents. `dirty: true` in provenance = the untracked research note only |

## Innovation I10: evidence debt ledger (2026-11-06, branch `inn-debt`)
Read-only `mfg-debt@1`: per accepted decision, the evidence it rests on that is missing (per release requirement),
unconfirmed, low-confidence, rejected, superseded (claim `superseded_at`), pending re-request (claim
`request_evidence` review until approve/reject; supplier re-requests until cancelled), expiring/expired, plus open
integrity signals, drift, match findings, conflicts and 8D cases. Components stay separate (no single score); every
value is computed "as known on" a transaction time (property test: later history never changes a past date).
API: `GET /evidence-debt` (group part|supplier|decision, trends), `GET /evidence-debt/decisions/{id}` (components +
debts with age), `GET /evidence-debt/items` (one row per piece of debt: age, decisions it affects, parts, suppliers).
UI: "Evidence debt" view under Decisions. No migration. Nothing changes a decision: people decide.

| Command | Result |
|---|---|
| `cargo test -p domain-manufacturing --lib debt` | 16 passed (incl. 3 proptests: as-of invariance of components and debts, debts match components) |
| `cargo test -p kizuna-api --test evidence_debt` (own DB `kizuna_test_debt`) | 4 passed: weakening evidence, past date unchanged, items + blocked decisions, part/supplier grouping, expired cert, supplier re-request open→cancelled, tenant isolation, 400/401/404 |
| `cargo test --workspace --no-fail-fast` | 563 passed, 2 ignored; 2 failed then fixed/explained: i18n catalog (4 new error codes, fixed: 3/3), what-if 100k edges 408 under load (passes alone, known flake) |
| clippy `-D warnings`, fmt, `pnpm lint` + vitest | clean; vitest 114 passed (+3 debt view incl. axe EN/JA) |

Gaps: obsolete findings carry no time of that change and are left out of every date; approved supplier re-requests
stay pending until the decision is reconfirmed or revoked (no fulfilment status exists); release requirements are a
fixed list for `*_release`/`*_acceptance`/`first_article` kinds only; pilot rank-correlation measure not done.

## M7 results (2026-11-05, this host)
| Command | Result |
|---|---|
| `scripts/check.sh` | **pass**: fmt, clippy `-D warnings`, **176 Rust tests** (+20; 1 ignored live test), cargo/pnpm/pip audit 0, ruff + **59 pytest** (+11), tsc + **35 vitest** (+6), gitleaks 0. The 8 GB guard removed `target/debug` (8.2 GB); disk 43 GB free |
| `cargo test -p kizuna-api --test m7_ai_settings` | 7/7 against real PostgreSQL + local mock OpenAI server: off by default, no request for tenants without opt-in, notice + admin-MFA opt-in, redaction (no e-mail/part number sent), `ai_suggested` + not decision evidence until four-eyes review, DB trigger, injection corpus (7 labels × 7 hijacked answers → fallback, only `unreviewed` claims), 500/timeout/budget fallback, import settings drive worker + long-table mapping, per-tenant/adaptive thresholds |
| M7 follow-up | admin screens for AI opt-in (verbatim notice, acknowledgement, MFA step-up, provider/budget/usage, off switch) and detection thresholds (bounds, defaults, active values, adaptive toggle), EN+JA, axe-tested; `POST /artifacts/{id}/reextract` (reason required; new run supersedes the previous run; changed claims superseded, unchanged kept, missing ones → `needs_evidence` + withdrawn; decisions via selective invalidation; audited); AI guards `ai-map@2` (governance denylist, multilingual injection screen → security event `ingest.prompt_injection_suspected`, plausibility floor) |
| live free model (ignored test, `gemma4:cloud` via local Ollama `127.0.0.1:11434/v1`) | pass, 1.1 s, 374+174 tokens: Werkstoff→material, Breite (mm)→width (mm), Härte→hardness; an injected label was mapped to `approval status` → stays `ai_suggested` (residual risk, ADR 0018). OmniRoute :20128 was down |

## M6 commands run and results (2026-11-05, this host)
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
- ASVS 5.0 control-to-test mapping re-run: `docs/security/asvs-mapping.md` (all mapped tests pass).

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
- Detection thresholds: per tenant (bounded) with optional adaptive mode, still untuned on real traffic; address
  blocks are per tenant and login-only; no cross-tenant (platform) correlation.
- AI-assisted extraction: free providers may log/train on what is sent (stated in the opt-in notice); redaction,
  injection screen and plausibility lexicon are heuristic (false positives only lose a suggestion).
- Notifications are in-app only.
- Benchmarks ran on a busy shared host (upper bounds). CI workflow never executed (no remote).
- Rate limiter is per process (single instance only). Dependency major upgrades pending (sqlx 0.9, rand 0.10, ...).

## Founder decisions (2026-11-05)
1. WebAuthn stays optional for admins (recorded in ADR 0006).
2. No trademark/patent work.
3. Pilot hosted on this server (deployment, seccomp, AV, backups, alerts: `ops-pilot` lane).
4. External AI = yes, free models only (ADR 0018; off by default, per-tenant opt-in).

## Next step
A re-extract button in Documents; merge the `ops-pilot` lane; then `scripts/check.sh && scripts/e2e.sh --seed 7`, and on the pilot host: run as the dedicated `kizuna` user per
`docs/deploy/DEPLOYMENT.md`, add a seccomp profile for the worker, and re-run `scripts/bench.sh` on an idle host.

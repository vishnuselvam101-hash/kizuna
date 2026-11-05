# Security review: M5, lane-auth, M7, API gaps, web session (main de315db)

Date: 2026-11-05. Read-only, independent. Snapshot `/root/kizuna-audit` (detached `de315db`). No code changed.
Checklists: security-and-hardening (authz, sessions/MFA, secrets, DoS, external integrations) and code-review-and-quality
(correctness first; each finding confirmed by reading the code path; speculative items dropped).
Out of scope (already reviewed): M1 review, `REVIEW_M2_M4.md`, the 2026-11-05 "final review of M5 + lanes" H1/M1-M4/L1/L2
(`tests/final_review_regressions.rs`). None of those are reported again.

Severity: **High** = fix before release. **Medium** = required. **Low** = should fix. **Info** = for information.

Counts: High 0 · Medium 2 · Low 8 · Info 4

---

## Cross-cutting checks (passed)

- **RLS:** every new table has `ENABLE` + `FORCE ROW LEVEL SECURITY` and a `tenant_isolation` policy with `WITH CHECK`:
  `incidents`, `response_actions`, `ip_blocks`, `tenant_controls`, `audit_checkpoints` (0005), `webauthn_credentials`,
  `webauthn_challenges`, `mfa_recovery_codes` (0006), `import_settings` (0008), `tenant_settings`, `ai_calls` (0010).
  Append-only triggers on `audit_checkpoints`, `import_settings`, `tenant_settings`, `ai_calls`. Grants are minimal
  (column-level UPDATE on `incidents`/`response_actions`). Roles are NOSUPERUSER/NOBYPASSRLS (DEPLOYMENT.md:36-37).
- **Authz, deny by default:** all 67 routes in `app.rs:72-140` were enumerated. Every handler except the
  unauthenticated login/refresh/WebAuthn-login/recover endpoints starts with `p.require(...)`/`require_on(...)`
  before any read. New routes: `/security/*` (SecurityEventRead / IncidentManage), `/audit/checkpoints*`
  (AuditVerify), `/integrations/*` + `/security/detection-settings` (GET UserRead / SecurityEventRead,
  PUT UserManage = admin + MFA), `/dashboard/summary` + `/subjects` (ClaimRead), `/reports` (AuditRead),
  `/users/{id}/sessions` (UserRead), WebAuthn register (session + password re-auth). The web `GRANTS` table
  (`web/src/session.ts`) is UI-only and matches `policy-engine::role_grants`.
- **Self-decisions:** `NoSelfReview` covers `IncidentManage` (`verification.rs:34`); `not_about_self` applied to
  decide/resolve; `reset_mfa` refuses self.
- **WebAuthn / recovery:** ceremony state sealed with AAD bound to tenant/user/row, single use (`DELETE … RETURNING`),
  5-min TTL, per-user/purpose bounded; enrolment needs password re-auth and revokes sessions; downgrade race closed
  in `complete_login` (`NOT webauthn_enabled OR $4`); recovery codes 128-bit, SHA-256, single use, recovery session
  is not MFA-verified. Key rotation (`keyring.rs`) covers all four sealed columns with the same AAD functions.
- **Password screening:** applied on create and change (`users.rs:43`, `auth.rs:883`).
- **AI:** off unless operator config **and** tenant opt-in under the current notice version; base URL https or
  loopback-http only, no redirects, no env proxies, response capped (256 KiB / 64 KiB), schema-checked, unknown
  ids/fields refused, confidence capped at 0.70, budget reserved under a per-tenant lock before the call, API key in
  `Secret` (redacted Debug). Import settings are whitelisted (`extraction.rs:206`) and the worker only lowers limits
  (`worker.py:178-179`).
- **Secrets in logs:** no token, password, TOTP secret, recovery code or AI key reaches `tracing` or audit details.
- **Web session:** tokens in memory only; refresh is effectively single-flight (tokens nulled before the await,
  `api.ts:53-58`), so parallel 401s do not replay a rotated refresh token (no false `token_theft`). No
  `dangerouslySetInnerHTML`/`innerHTML`/storage use.

---

## Medium

### M1. An AI suggestion becomes usable evidence through "request evidence", without anyone accepting it
- **Where:** `crates/kizuna-api/src/verification.rs:856-866` (`request_evidence` → `needs_evidence` for any current
  status, including `ai_suggested`), `:881-885` (the new-evidence hook runs only for `ai_suggested → accepted`),
  `:441` (rules exclude only `rejected`/`ai_suggested`), `decisions.rs:520-526` (decisions refuse only
  `rejected`/`ai_suggested`). The DB trigger (`0010_ai_thresholds.sql`) only blocks transitions *into* `ai_suggested`.
- **Scenario:** extraction stores an injected/hallucinated AI mapping as `ai_suggested` (ADR 0016 residual risk).
  A reviewer, unsure, chooses "request evidence" — the opposite of approval. The claim becomes `needs_evidence`.
  From then on it feeds conflict detection and `compare`, and any Reviewer can cite it in `POST /decisions`
  (no `claim_ai_suggested_unreviewed` check). Dependent decisions were never notified (`on_claim_change` skipped),
  and if it is later accepted the hook does not run either (current is no longer `ai_suggested`). The M7 invariant
  "AI output is not decision evidence until a person accepts it" is broken. Not covered by `m7_ai_settings.rs`.
- **Fix:** make AI provenance sticky: either keep `ai_suggested` on `request_evidence` (only `approve`/`reject` leave
  it), or add an `ai_origin`/`extraction_method LIKE 'ai:%'` check so rules and decisions require
  `review_status = 'accepted'` for AI-origin claims; run `on_claim_change(new_evidence)` on the first transition to
  `accepted` for AI-origin claims regardless of the previous status. Optionally extend the trigger to forbid
  `ai_suggested → needs_evidence`.
- **Regression test:** `m7_ai_request_evidence_does_not_launder_suggestion`: AI claim → `request_evidence` →
  assert status still `ai_suggested` (or AI-origin), `POST /decisions` citing it → 409
  `claim_ai_suggested_unreviewed`, `/verification/run` produces no finding from it; then approve by a second
  reviewer → decision allowed and dependents notified.

### M2. Any Auditor can stall all audited writes of a tenant (checkpoint lock held during full-chain verification; no rate limits)
- **Where:** `crates/kizuna-api/src/audit.rs:280-290` (`create_checkpoint`: `SELECT … FROM audit_heads … FOR UPDATE`,
  then `verify_tenant` over the whole chain in 1000-row batches, inside the same transaction); `audit.rs:49-60`
  (every `audit::append` needs that same row lock); `audit.rs:353-385` (`verify_checkpoints`: one query per stored
  checkpoint + full chain); `insights.rs:88-131` (`POST /reports`: unbounded `findings`/`decisions` arrays + full
  chain). The only per-user limiter on these paths is none (`grep limiter.check`: login, refresh, upload, artifact
  verify, authz-event).
- **Scenario:** an Auditor (read-only role, AuditVerify, no MFA needed) loops `POST /audit/checkpoints`. Each call
  holds the tenant's audit-head lock for the full O(n) verification (seconds on a large chain; up to the 60 s request
  timeout), so every review, decision, upload, settings change and incident action of the tenant queues behind it
  and pool connections (max_connections, 10 s acquire timeout) are exhausted. Each call also inserts a checkpoint
  row, so `verify_checkpoints` gets slower without bound. `/reports scope=all` and `/audit/verify` add unbounded CPU
  and memory per request.
- **Fix:** do not hold `audit_heads FOR UPDATE` during verification — read the head in a REPEATABLE READ snapshot
  (as `/audit/verify` does) and sign that snapshot's head; or verify incrementally from the last checkpoint. Add
  per-user and per-tenant limits (e.g. 6/min) on `/audit/verify`, `/audit/checkpoints*`, `/reports`; cap or page
  report arrays; restrict checkpoint creation to `tenant_admin` or a scheduled job.
- **Regression test:** `m5_checkpoint_does_not_block_appends`: start a checkpoint on a 50k-event chain and assert a
  concurrent claim review commits within 1 s; `m5_audit_endpoints_rate_limited`: 10 rapid `POST /audit/checkpoints`
  by an Auditor → 429 after the limit.

---

## Low

### L1. A tenant admin can switch off "authorization probing" detection within the allowed bounds
- **Where:** `crates/security-detection/src/lib.rs:155` (`authz_denials` upper bound 500 per 10 min);
  `crates/kizuna-api/src/auth.rs:92` (at most 30 `authz.denied` events recorded per user per minute = 300 per 10 min).
- **Scenario:** a stolen admin+MFA session sets `authz_denials` to 301-500 via `PUT /security/detection-settings`;
  R4 can then never fire for any single user. The bounds comment says upper bounds exist to stop exactly this.
  Adaptive mode can reach the same state (`authz_denials 100 × factor 5`).
- **Fix:** derive the R4 upper bound (and adaptive cap) from the recording limit (≤ 300, better ≤ 150), or apply the
  cap in `effective_thresholds`; assert `bound < 10 × authz-event limit` in a unit test.
- **Regression test:** `detection_bounds_stay_below_recording_caps` (unit) + API test: `authz_denials: 301` → 400.

### L2. Adaptive baseline learns from events that already fired an incident
- **Where:** `crates/kizuna-api/src/incidents.rs:55-63` (baseline query takes all events 1 h–N days old);
  `security-detection/src/lib.rs:224-253`.
- **Scenario:** in adaptive mode an insider triggers R4/R7 once (sessions revoked / uploads restricted for 1 h, then
  logs in again). Those events stay in the baseline, so for the next `baseline_days` the per-user threshold is raised
  up to `factor ×` and the same insider can repeat the activity just below it, undetected.
- **Fix:** exclude events referenced by any incident (`event_ids`) that was not closed as `false_positive`, or
  compute the baseline from a robust statistic (p95 of per-user windows) instead of the single peak.
- **Regression test:** `adaptive_baseline_ignores_incident_events`: seed 25 denials in one window that opened an
  incident 2 days ago → effective `authz_denials` stays at the configured value.

### L3. Weakening security settings is audited but never alerts
- **Where:** `tenant_settings.rs:32-65` (`save`: audit only), `insights.rs:201-256` (`put_settings`: audit only, and
  the audit detail stores only the version, not what changed).
- **Scenario:** detection thresholds raised to the maximum, adaptive mode on, AI egress enabled or the long-table
  mapping changed (which changes how every future CSV becomes claims) — none produce a security event, so R5
  ("privileged operation burst"), the security dashboard and the incident list never show it.
- **Fix:** record a security event (`admin.settings_changed`, severity medium/high when thresholds rise or AI is
  enabled) and include it in R5's kinds; put the old→new diff in the import-settings audit detail.
- **Regression test:** `settings_changes_emit_security_events`: PUT detection/AI/import → event of that kind in
  `/security-events`.

### L4. The audit signing key cannot be rotated without raising a false "audit chain broken" alarm
- **Where:** `audit.rs:327-334` (`check_one` requires `c.public_key == current key`), `audit.rs:353-385`
  (`verify_checkpoints` re-labels every stored checkpoint with the *current* public key and records
  `AuditChainBroken` on any failure), `config.rs:179-182` (single key, no previous keys); `key_id` is stored but unused.
- **Scenario:** after the operator replaces `KIZUNA_AUDIT_SIGNING_KEY` (e.g. suspected leak), every old checkpoint
  fails, `integrity.audit_chain_broken` fires the critical `audit_chain_broken` incident and the real integrity
  signal is lost in noise — or the operator never rotates.
- **Fix:** `KIZUNA_AUDIT_SIGNING_KEYS_PREVIOUS` (like `DataKeys`), look up the verification key by stored `key_id`,
  report "valid under retired key" separately from tampering.
- **Regression test:** `checkpoints_survive_signing_key_rotation`: sign, rotate (old key kept as previous), verify →
  intact, no security event; drop old key → reported as `unknown_key`, not tampering.

### L5. Containment bookkeeping: revert lifts other actions' blocks; actions of closed incidents stay approvable
- **Where:** `incidents.rs:461-466` (`DELETE FROM ip_blocks WHERE ip = $1`, ignores `action_id`), `:476-482`
  (same for `restrict_uploads`), `:683-724` (`decide_action` does not check the incident status), `:761-790`
  (`resolve` leaves `proposed` actions pending).
- **Scenario:** two spraying incidents for one address; reverting the older one deletes the newer block. After an
  incident is closed as `false_positive`, its pending `lock_account` can still be approved later, locking a user
  for a closed case.
- **Fix:** revert only `WHERE ip = $1 AND action_id = $2` (same idea for uploads: remember the action id); on
  resolve mark pending actions `rejected` ("incident closed"); refuse approve on closed incidents.
- **Regression test:** `revert_only_lifts_own_block`, `closed_incident_actions_not_approvable`.

### L6. A slow AI provider holds one of the two global extraction slots
- **Where:** `verification.rs:217` (`worker_permits`, 2 per process, all tenants) and `:269` (`ai::suggest`, up to
  `KIZUNA_AI_TIMEOUT_SECS` ≤ 30 s, called while the permit and the per-artifact lock connection are held).
- **Scenario:** one opted-in tenant with a throttled free provider keeps both slots busy for up to 30 s per
  extraction; other tenants' extractions queue (unbounded `acquire()`) until the 60 s request timeout.
- **Fix:** drop the permit (`drop(_permit)`) before `ai::suggest`; use a separate small semaphore with
  `try_acquire`/timeout for AI calls.
- **Regression test:** with the mock provider sleeping 10 s, a second tenant's extraction completes in < 5 s.

### L7. The SPA is served without CSP or frame-ancestors
- **Where:** `docs/deploy/DEPLOYMENT.md:141-148` (Caddy `header` block: HSTS, nosniff, referrer only; "CSP should be
  added once the asset list is final"). API headers (`app.rs:63-71`) do not apply to the static files.
- **Scenario:** no defence-in-depth against XSS (in-memory tokens can still be used by injected script) and the
  app can be framed. Clickjacking impact is low because a framed copy has no session (tokens are in memory).
- **Fix:** add `Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self';
  img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'` and `X-Frame-Options DENY` to
  the Caddy site block; check the built assets for inline script/style first.
- **Regression test:** deploy smoke check: `curl -sI https://host/ | grep -i content-security-policy` with
  `frame-ancestors 'none'`.

### L8. Artifact release has no four-eyes rule and no integrity re-check
- **Where:** `artifacts.rs:413-425` (`release`: IncidentManage only, `Resource` owner not set, no `verify_integrity`).
- **Scenario:** an admin uploads a file the scanner quarantines (e.g. active PDF) and releases it themselves; or an
  artifact quarantined by an `artifact_integrity_failure` incident is released without the bytes being repaired
  (content/extract still re-verify, so impact is limited to status and the trust signal). Inconsistent with the
  final-review M3 rule that no admin decides containment about themselves.
- **Fix:** pass `owner_user_id` = uploader (`artifacts.created_by` / captures) so `NoSelfReview` applies; when
  `scan_result.quarantined_by_incident` is set, require a passing `verify_integrity` before release.
- **Regression test:** `uploader_cannot_release_own_quarantine`; `integrity_quarantine_release_requires_intact_bytes`.

---

## Info

- **I1.** `POST /reports` (organisation-wide export) is recorded only as a security event (`insights.rs:150`), not in
  the tamper-evident audit chain; add an `export.report` audit record for exports.
- **I2.** `login_finish` (`webauthn.rs:362-418`) does not re-check `ip_blocked`; a challenge obtained up to 5 min
  before a block can still finish. Low impact (start is blocked; blocks are login-only by design).
- **I3.** R1 brute force counts `bad_password`/`bad_totp`/`reauth_bad_password` only (`security-detection/src/lib.rs:296`);
  `bad_webauthn` and `bad_recovery_code` count toward lockout but never toward the incident.
- **I4.** The audit signing key lives in the server environment; checkpoints protect against a DB-only attacker, not
  an app-host compromise (consistent with BUILD_STATUS M1 #6; state it in THREAT_MODEL).

## Checked and dropped (no finding)

- Parallel 401s replaying a rotated refresh token (web single-flight holds).
- Recovery codes as a single factor (they need the password; first_factor rate limits apply; session is non-MFA).
- Cross-tenant use of WebAuthn challenges/credentials (RLS + user-bound AAD + `user_id` filter).
- AI SSRF via base URL (operator-only config, validated; no redirects, no proxy).
- Import-settings injection into the worker (whitelist + worker only tightens limits).
- Emergency unlock re-enabling deliberately disabled admins (fixed by final-review M4, `disabled_by_action`).
- Detection flood / lock queuing (final-review H1).

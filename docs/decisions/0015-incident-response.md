# ADR 0015: Automated, defensive incident response and signed audit checkpoints

Status: accepted (2026-11-05)

## Decision
- `crates/security-detection` (`sec-rules@1`): deterministic rules over the last hour of a tenant's security events:
  brute force per account, password spraying per address, refresh-token theft, authorisation probing, privileged
  operation bursts, artifact integrity failure, audit chain failure, upload abuse, extraction failures, unusual
  report exports. Triggered inline by high-signal events and by a 60 s sweep in `kizuna-server`.
- Incidents are deduplicated per (rule, subject) while open; event ids are kept for forensics; nothing is deleted.
- Response policy: pre-authorised (automatic, reversible or time-limited): revoke sessions, block an address for 60
  min (per tenant, login only), quarantine an artifact, restrict a user's uploads 60 min, suspend extraction 30 min,
  in-app notification. Requires approval by an MFA-verified tenant admin: disabling an account; revoking the sessions
  of an administrator flagged for a privileged-operation burst. Guards: never disable the last active
  administrator; never auto-block an address an active administrator used in the last 24 h (escalated to approval).
  `kizuna-admin emergency-unlock` re-enables admins and lifts blocks (audited).
- Every step is audited and visible in the security dashboard; executed actions can be reverted where meaningful.
- Strictly defensive: no action ever targets anything outside KIZUNA (no scanning, no counter-measures).
- Signed audit checkpoints: Ed25519 (`ed25519-dalek` 2, BSD-3) over (tenant, seq, head hash, time) with a key from
  `KIZUNA_AUDIT_SIGNING_KEY` (outside the DB). Exported checkpoints detect a full re-chaining by a database owner
  (closes M1 review finding #6 when checkpoints are exported and the key is not stored with the database).
- Notifications are in-app only; e-mail/webhook connectors need a deployment decision.

- **Per-tenant thresholds (M7, 2026-11-05).** `GET/PUT /security/detection-settings` (read: security-event
  readers; write: tenant_admin with MFA, versioned in `tenant_settings`, optimistic concurrency, audited
  `settings.detection_updated`). Settings: `mode` (`static` | `adaptive`), `thresholds` (any subset; defaults below),
  `adaptive_max_factor` (1–5, default 3), `baseline_days` (1–30, default 7). Unknown keys are refused.
  Defaults / allowed ranges: brute_force 5 (3–50), brute_force_lock 10 (3–100, ≥ brute_force), spray_failures 8
  (3–200), spray_users 3 (2–50), authz_denials 10 (3–500), privileged_ops 5 (2–200), upload_abuse 5 (2–200),
  extraction_failures 5 (2–200), exports 20 (3–1000). Upper bounds stop a tenant or a stolen admin session from
  switching detection off.
- **Adaptive mode** raises only activity-volume rules (authorization probing, privileged bursts, upload abuse,
  extraction failures, exports) to `busiest past window + 1`, where the baseline is the tenant's own events older
  than 1 h and within `baseline_days` (per user where the rule is per user). Clamped to
  `[configured, configured × adaptive_max_factor]`, so a slow attacker can raise a threshold at most by the factor,
  and an ongoing burst never counts toward its own baseline. Credential, token and integrity rules are never adapted.
  Recomputed at most every 10 minutes per tenant (in-process cache, dropped when settings change).
  `GET` returns the active thresholds.

## Consequences
Defaults are untuned on real traffic; tenants can now tune them within bounds. Address blocks are per tenant and only apply to login.
Platform-level (null-tenant) events are not correlated across tenants yet.

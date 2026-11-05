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

## Consequences
Thresholds are static defaults, untuned on real traffic. Address blocks are per tenant and only apply to login.
Platform-level (null-tenant) events are not correlated across tenants yet.

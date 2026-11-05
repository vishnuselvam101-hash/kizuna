# OWASP ASVS 5.0.0 control-to-test mapping (M1 controls)

Requirement IDs refer to ASVS 5.0.0 chapters (V6 Authentication, V7 Session Management, V8 Authorization,
V16 Security Logging, V4 API, V13 Configuration, V11 Cryptography). Chapter-level mapping; exact requirement numbers
must be re-checked against the published 5.0.0 text before any external compliance statement. No certification is
claimed. Result column = last local run (see BUILD_STATUS.md).

| Area (ASVS 5.0) | Control | Implementation | Test | Result | Limitation / status |
|---|---|---|---|---|---|
| V6 password storage | argon2id m=19456,t=2,p=1 | `crypto.rs::hash_password` | `crypto::tests::password_hash_roundtrip_and_policy` | pass | |
| V6 password policy | 12–128 chars, no composition rules, offline common/breached list + context words | `crypto::check_password_policy` | same + `common_and_context_passwords_rejected`, `auth_hardening::common_and_context_passwords_are_refused`, `m1_auth::password_change_policy_and_revocation` | pass | list = 46k public common passwords ≥12 chars, not a full breach corpus |
| V6 brute force | per-IP + per-account limits, lockout 5/15 min (also bad WebAuthn assertion / recovery code) | `auth::first_factor`, `auth::login_failed`, `ratelimit.rs` | `lockout_after_repeated_failures`, `login_rate_limited_per_ip`, `ratelimit::tests`, `lockout_expires_with_the_clock`, `webauthn_assertion_error_paths` | pass | per-process limiter |
| V6 enumeration | identical error for unknown tenant/user/bad password, dummy hash | `auth::login` | `login_me_and_generic_failures` | pass | `mfa_required` reveals a correct password (rate-limited) |
| V6 MFA | TOTP RFC 6238, replay protection, secret encrypted at rest | `crypto::totp_*`, `auth::totp_*` | `rfc6238_sha1_vectors`, `totp_window_and_replay`, `totp_mfa_flow_and_replay` | pass | |
| V6 phishing-resistant MFA | WebAuthn (UV required), mandatory once enrolled, single-use 5-min challenges, tenant-scoped | `webauthn.rs` | `webauthn_enrolment_makes_key_mandatory_for_admin`, `webauthn_assertion_error_paths` (replay, forged signature, expiry, cross-tenant, cross-user registration), `bad_assertions_lock_out_and_challenges_stay_bounded`, `adding_a_key_to_a_protected_account_needs_a_key_session` | pass | tested with a software authenticator, not real hardware; attestation not enforced |
| V6 MFA recovery | one-time recovery codes, peer reset (not self), operator CLI | `webauthn::recover`, `users::reset_mfa`, `kizuna-admin reset-mfa` | `lost_key_recovery_never_locks_out_admins`, `recovery_code_hash_normalises_formatting_only` | pass | CLI path exercised via shared `webauthn::clear_all`, not end-to-end |
| V7 token entropy & storage | 256-bit opaque tokens, SHA-256 stored | `crypto::issue_token` | `tokens_are_stored_hashed`, `parse_token_never_panics` | pass | |
| V7 session lifetime | access 15 min, absolute 12 h, access capped at session end | `auth::start_session`, `auth::refresh`, `Principal`, `clock.rs` | `session_and_refresh_expiry_follow_the_clock` | pass | |
| V7 rotation & reuse | refresh rotation, reuse → family revoke | `auth::refresh` | `refresh_rotates_and_reuse_revokes_family`, `refresh_token_with_forged_tenant_prefix_fails` | pass | |
| V7 termination | logout, password change, role change, disable, admin revoke | `auth.rs`, `users.rs` | `logout_revokes_server_side`, `password_change_...`, `last_admin_guard_and_role_change_revokes_sessions`, `admin_revokes_sessions_and_resets_mfa` | pass | |
| V8 deny by default | explicit grant table, unknown role denied | `policy-engine` | `policy_engine::tests::*`, `rbac_denies_by_default_and_logs` | pass | |
| V8 tenant isolation | RLS FORCE + policy tenant check | migrations, `db::tenant_tx` | `rls_isolates_at_sql_level_for_runtime_role`, `admin_cannot_touch_other_tenant`, `cross_tenant_always_denied` | pass | |
| V8 privileged ops | MFA-verified session for UserManage; last-admin guard | `policy-engine`, `users.rs` | `privileged_needs_mfa`, `totp_mfa_flow_and_replay`, `last_admin_guard_...` | pass | no re-auth freshness window yet |
| V16 security logging | security_events append-only, no secrets | `security_events.rs` | multiple tests assert event kinds | pass | no alerting (M5) |
| V16 log integrity | hash-chained audit, append-only triggers | `audit-engine`, `audit.rs` | `tampering_is_detected`, `tail_truncation_is_detected_via_head`, `append_only_even_for_owner...`, proptest `any_field_mutation_is_detected`, `concurrent_appends_form_one_chain` | pass | full-chain rewrite by DB superuser undetectable until signed checkpoints |
| V4/V13 HTTP hardening | security headers, body limit, timeouts, no CORS | `app.rs` | `health_and_security_headers`, `body_limit_and_bad_json` | pass | TLS at reverse proxy (deployment) |
| V13 config & secrets | env-only config, redacted Debug, .env.example only | `config.rs` | `config::tests::*`, gitleaks in check.sh | pass | no external secrets manager |
| V13 least privilege DB | runtime role not owner/superuser/BYPASSRLS, refuses to start otherwise | `db::assert_runtime_role` | `runtime_role_check` | pass | |
| V11 crypto at rest | AES-256-GCM with AAD | `crypto::seal/open` | `seal_open_binds_aad` | pass | |
| V11 key rotation | versioned keys, header in AAD, old keys readable until per-tenant re-encrypt | `keyring.rs`, `kizuna-admin rotate-data-key` | `keyring::tests::*`, `data_key_rotation_reencrypts_per_tenant` (isolation, idempotence, legacy blob, in-flight challenge, unreadable blobs) | pass | keys still from env, no KMS/HSM |

## Additions M2–M6 (re-run 2026-11-05, `scripts/check.sh` all green on the integrated main)

| Area (ASVS 5.0) | Control | Implementation | Test | Result | Limitation |
|---|---|---|---|---|---|
| V6 phishing-resistant MFA | WebAuthn for privileged accounts; once enrolled, key mandatory for admin; recovery never locks out admins | `webauthn.rs`, `auth.rs` | `auth_hardening::webauthn_*`, `lost_key_recovery_never_locks_out_admins`, `adding_a_key_to_a_protected_account_needs_a_key_session` | pass | WebAuthn enrolment for admins is optional (founder decision) |
| V6 breached/common passwords | offline common + context password screening | lane-auth | `common_and_context_passwords_are_refused` | pass | offline list only |
| V7 session expiry | injected clock tests of access/session/refresh expiry and lockout expiry | `clock` | `session_and_refresh_expiry_follow_the_clock`, `lockout_expires_with_the_clock` | pass | — (closes M1 gap) |
| V11 key management | versioned data keys, rotation CLI; Ed25519 signing key outside DB | `keyring.rs`, `audit.rs` | `data_key_rotation_reencrypts_per_tenant`, `signed_checkpoints_detect_full_rechain_by_db_owner` | pass | no HSM/KMS |
| V8 four-eyes / privileged review | review = MFA; no self-review of own uploads, claims, findings, decisions | `verification.rs`, `decisions.rs`, policy-engine | `review_m2_m4_regressions::h1_*`, `l6_*` | pass | |
| V8 tenant isolation (new tables) | RLS FORCE on artifacts…incidents, import_settings | migrations 0002–0008 | `artifacts_and_claims_are_tenant_isolated`, `conflicts_are_tenant_isolated`, `decision_rbac_validation_and_isolation`, `incidents_rbac_and_isolation` | pass | |
| V5 file handling | content typing, static scan (escaped names, ObjStm), quarantine, size limits, sandboxed parsing | `evidence-core`, `extraction.rs`, worker | `rejects_disallowed_types_and_oversize`, `regression_review_m5_*`, Python isolation tests | pass | static scan is not antivirus; no seccomp |
| V16 security monitoring & response | detection rules, containment with approval policy, guards | `incidents.rs`, `security-detection` | `m5_defense::*` (8) | pass | static thresholds |
| V16 log integrity | signed checkpoints, report signatures, derivation cross-check | `audit.rs`, `decisions.rs` | `signed_checkpoints_*`, `l6_l7_l8_l9_*` | pass | checkpoints must be exported to be useful |
| V14 data protection in reports | E2E reports redact credentials; gitleaks fingerprint allowlist documented | `scripts/e2e/driver.py`, `.gitleaksignore` | gitleaks in check.sh | pass | |

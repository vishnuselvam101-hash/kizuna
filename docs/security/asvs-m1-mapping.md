# OWASP ASVS 5.0.0 control-to-test mapping (M1 controls)

Requirement IDs refer to ASVS 5.0.0 chapters (V6 Authentication, V7 Session Management, V8 Authorization,
V16 Security Logging, V4 API, V13 Configuration, V11 Cryptography). Chapter-level mapping; exact requirement numbers
must be re-checked against the published 5.0.0 text before any external compliance statement. No certification is
claimed. Result column = last local run (see BUILD_STATUS.md).

| Area (ASVS 5.0) | Control | Implementation | Test | Result | Limitation / status |
|---|---|---|---|---|---|
| V6 password storage | argon2id m=19456,t=2,p=1 | `crypto.rs::hash_password` | `crypto::tests::password_hash_roundtrip_and_policy` | pass | |
| V6 password policy | 12–128 chars, no composition rules | `crypto::check_password_policy` | same + `m1_auth::password_change_policy_and_revocation` | pass | breached-password check missing |
| V6 brute force | per-IP + per-account limits, lockout 5/15 min | `auth::login`, `ratelimit.rs` | `lockout_after_repeated_failures`, `login_rate_limited_per_ip`, `ratelimit::tests` | pass | per-process limiter |
| V6 enumeration | identical error for unknown tenant/user/bad password, dummy hash | `auth::login` | `login_me_and_generic_failures` | pass | `mfa_required` reveals a correct password (rate-limited) |
| V6 MFA | TOTP RFC 6238, replay protection, secret encrypted at rest | `crypto::totp_*`, `auth::totp_*` | `rfc6238_sha1_vectors`, `totp_window_and_replay`, `totp_mfa_flow_and_replay` | pass | WebAuthn not implemented |
| V7 token entropy & storage | 256-bit opaque tokens, SHA-256 stored | `crypto::issue_token` | `tokens_are_stored_hashed`, `parse_token_never_panics` | pass | |
| V7 session lifetime | access 15 min, absolute 12 h | `auth::start_session`, `Principal` | covered indirectly; no time-travel test | partial | add clock-injected expiry test |
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
| V11 crypto at rest | AES-256-GCM with AAD | `crypto::seal/open` | `seal_open_binds_aad` | pass | single static key, no rotation yet |

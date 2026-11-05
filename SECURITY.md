# Security

Report vulnerabilities privately to the maintainers (no public issues). No bug bounty. No certification (ISO 27001,
SOC 2, IEC 62443) is claimed.

## Implemented controls (M1)
argon2id passwords; TOTP MFA (encrypted at rest, replay-protected) required for privileged permissions; 15-min opaque
access tokens and rotating refresh tokens with reuse detection; server-side revocation; deny-by-default RBAC + ABAC
hooks; PostgreSQL RLS FORCE tenant isolation with separate owner/runtime roles; hash-chained append-only audit log;
security_events; per-IP/per-account rate limits and lockout; secure headers, body limits, timeouts, no CORS;
config from env, secrets redacted in Debug, gitleaks in the gate. Mapping to tests: `docs/security/asvs-m1-mapping.md`.

## Known gaps
WebAuthn not implemented; no breached-password screening; in-process rate limiter; single static data key (no
rotation); no signed audit checkpoints yet; TLS must be terminated by a reverse proxy; `mfa_required` response confirms a
correct password.

## Rules
Never log passwords, tokens, keys or document contents. Every confirmed vulnerability gets a regression test.
Defensive only: no hack-back, no scanning of third parties (directive §10).

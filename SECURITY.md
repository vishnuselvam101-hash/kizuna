# Security

Report vulnerabilities privately to the maintainers (no public issues). No bug bounty. No certification (ISO 27001,
SOC 2, IEC 62443) is claimed.

## Implemented controls (M1; M2–M6 additions in the ASVS mapping)
argon2id passwords; TOTP MFA (encrypted at rest, replay-protected) required for privileged permissions; 15-min opaque
access tokens and rotating refresh tokens with reuse detection; server-side revocation; deny-by-default RBAC + ABAC
hooks; PostgreSQL RLS FORCE tenant isolation with separate owner/runtime roles; hash-chained append-only audit log;
security_events; per-IP/per-account rate limits and lockout; secure headers, body limits, timeouts, no CORS;
config from env, secrets redacted in Debug, gitleaks in the gate. Mapping to tests: `docs/security/asvs-mapping.md`.

## Known gaps
Seccomp filter for the extraction worker is a strace-derived allowlist (default EPERM; `ioctl` and `clone` argument-filtered, clone3→ENOSYS; ADR 0016 amendment 2026-11-06) — it must be re-profiled with `scripts/seccomp-profile.sh` when Python, pypdf or tesseract change; ClamAV only when configured (ADR 0017, fail-closed quarantine); in-process rate limiter; WebAuthn
optional for admins (founder decision); TLS must be terminated by a reverse proxy; `mfa_required` confirms a correct
password; signed checkpoints only help when exported off-host. M2–M6 controls: docs/security/asvs-mapping.md.

## Rules
Never log passwords, tokens, keys or document contents. Every confirmed vulnerability gets a regression test.
Defensive only: no hack-back, no scanning of third parties (directive §10).

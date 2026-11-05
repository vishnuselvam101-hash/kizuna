# KIZUNA Verify threat model

Method: STRIDE per trust boundary, updated each milestone. Version: M0/M1 (2026-11-05).

## Assets
Customer documents and extracted claims (confidential, multi-tenant); credentials, sessions, TOTP secrets, data key;
audit and security logs (integrity); decision records (integrity, non-repudiation); availability of review workflow.

## Trust boundaries
1. Browser/API client → API server (internet in production; TLS terminated by a reverse proxy).
2. API server → PostgreSQL (runtime role, RLS).
3. API server → object store (filesystem now).
4. API server → extraction worker (untrusted document content crosses here; worker has no DB credentials).
5. Operators → `kizuna-admin` CLI (owner role; privileged).

## Threats and mitigations
| ID | STRIDE | Threat | Mitigation | Milestone / status |
|---|---|---|---|---|
| T1 | S | Credential stuffing / brute force | argon2id, per-IP + per-account rate limits, lockout, security events | M1 implemented |
| T2 | S | Stolen refresh token replay | rotation + reuse detection revokes family | M1 implemented |
| T3 | S | Phishing of admin credentials | TOTP required for privileged actions; WebAuthn planned | M1 TOTP; WebAuthn designed only |
| T4 | T | Tampering with audit log in DB | append-only grants + triggers + hash chain verify; signed checkpoints later | M1 chain; checkpoints M5 |
| T5 | I | Cross-tenant data access through bug | RLS FORCE + tenant check in policy-engine + tenant-isolation tests | M1 implemented |
| T6 | E | Privilege escalation via role change | UserManage privileged (MFA), last-admin guard, audit of role changes | M1 implemented |
| T7 | R | User denies an action | audit events with actor, session, hash chain | M1 implemented |
| T8 | D | Large request bodies / slowloris | body limits, timeouts; reverse proxy in production | M1 implemented (app level) |
| T9 | I | Secrets in logs | redacting secret types, no header logging, tests | M1 implemented |
| T10 | T/E | Malicious upload (polyglot, zip bomb, PDF exploit) | size limit, magic-byte allowlist, quarantine status, parsing only in sandboxed worker with rlimits/timeouts, no network | M2/M3 |
| T11 | E | Prompt injection in documents steering AI | no external AI; deterministic extraction; document text never treated as instructions; extracted claims are unreviewed data | M3 |
| T12 | I | Path traversal in object store | keys built from UUID + hex only | M2 |
| T13 | T | Artifact silently replaced | content hash at capture, integrity verification job, security event on mismatch | M2 |
| T14 | S/E | Compromised operator CLI | owner credentials only on operator host; CLI actions audited | M1 partial (audit of CLI actions) |
| T15 | D | Lock-out of all admins by automated response | containment never disables the last admin; CLI recovery | M1 guard; M5 policy |
| T16 | I | Hash misread as authenticity proof | docs/UI wording: integrity since capture only | M0 policy |

## Out of scope (now)
Network-level DDoS, host compromise, malicious DB superuser (can rewrite everything; mitigated later by exported signed
checkpoints), supply-chain compromise beyond cargo-audit/lockfiles.

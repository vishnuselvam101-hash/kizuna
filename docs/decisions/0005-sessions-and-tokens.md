# ADR 0005: Opaque server-side access tokens with rotating refresh tokens

Status: accepted (2026-11-05)

## Decision
- Access token: `kza.<tenant-uuid>.<32 random bytes base64url>`, TTL 15 min (configurable 1–60 min). Refresh token:
  `kzr.<tenant-uuid>.<32 random bytes>`; session absolute lifetime 12 h.
- Only SHA-256 of the secret part is stored (`sessions.access_hash`, `refresh_tokens.token_hash`). The tenant prefix
  lets the server open an RLS-scoped transaction before lookup; a forged prefix only selects a tenant where the hash
  does not exist.
- Every refresh rotates both tokens. A refresh token can be used once; presenting a used one is treated as theft:
  the whole session (token family) is revoked, a `auth.refresh_token_reuse` security event (high) is written.
- Every request validates the session row (revocation is immediate). Logout, password change, user disable, admin
  revoke and MFA reset revoke sessions server-side.

## Alternatives
JWT access tokens: stateless but revocation needs a deny-list; algorithm confusion risks. The per-request DB lookup cost
is acceptable at SME scale; revisit with measurements in M6.

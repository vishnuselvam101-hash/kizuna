# ADR 0008: In-process rate limiting plus account lockout

Status: accepted (2026-11-05)

## Decision
- Auth endpoints use an in-process keyed sliding-window limiter (per client IP, and per tenant+email), written in
  ~100 lines with unit tests instead of adding `governor` (keeps the dependency tree small).
- Account lockout in the DB: 5 consecutive failures lock the account for 15 min; success resets. Lockout and limit hits
  produce security events.
- Client IP comes from the socket; `X-Forwarded-For` is only trusted when `KIZUNA_TRUST_PROXY=1`.

## Consequences
Limits are per process. Multi-instance deployment must move counters to PostgreSQL or Redis (ADR needed then).

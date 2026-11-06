# Contributing

- Run `scripts/check.sh` before every commit; it must pass (fmt, clippy -D warnings, tests against real Postgres,
  cargo/pnpm audit, ruff, pytest, tsc, vitest, gitleaks). Never disable a check to go green.
- Small commits, imperative subject, author `vishnuselvam101-hash`. No AI attribution trailers.
- Every significant choice gets an ADR in `docs/decisions/`. New dependency → `docs/research/dependencies.md`.
- New tenant table: `tenant_id NOT NULL`, ENABLE+FORCE RLS, least-privilege grant, isolation test.
- State change → audit event in the same transaction. Security-relevant failure → security event.
- Engines (`crates/*-engine`) stay I/O-free and free of procurement types.
- Integration tests create their own tenants; never depend on test order.
- User-facing server text is a message code (`kizuna_messages::Msg`) with EN + JA in `web/src/i18n-server.json`;
  never a finished English sentence (ADR 0027). New API error code → `err.<code>` in both web dictionaries.

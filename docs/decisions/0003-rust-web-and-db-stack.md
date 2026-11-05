# ADR 0003: axum + tokio + sqlx 0.8 (PostgreSQL), runtime-checked queries

Status: accepted (2026-11-05)

## Decision
- HTTP: axum 0.8 on tokio 1, tower-http 0.6 for body limits, timeouts and headers.
- DB: sqlx 0.8 (postgres, rustls, migrate). Queries use `sqlx::query`/`query_as` with bind parameters only (no string
  interpolation of user data). Compile-time checked macros are not used so the build needs no live DB.
- sqlx 0.9, rand 0.10, sha2 0.11, argon2 0.6 were released recently; we stay on the widely deployed previous major
  lines (no open RustSec advisories at the time of writing, see docs/research/dependencies.md) and track the upgrade
  in ROADMAP.md.
- Time: `time` 0.3. IDs: UUID v7 (`uuid` crate).

## Alternatives
diesel (sync, heavier codegen), tokio-postgres (lower level, no migrations). sqlx is the most common async choice.

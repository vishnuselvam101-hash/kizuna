# ADR 0001: Modular monolith, Rust core, Python extraction workers, TypeScript UI

Status: accepted (2026-11-05)

## Context
Directive §3/§4: separable engines (evidence, provenance, temporal, conflict, dependency, policy, audit, security
events), Rust for the security-critical core, Python for document extraction, TypeScript web app, PostgreSQL, object
storage. No microservices/Kafka/Kubernetes/vector DB/blockchain without a measured need. Small team, one server.

## Decision
- One Cargo workspace. Pure, I/O-free engine crates (`crates/audit-engine`, `crates/policy-engine`, later
  `crates/evidence-core`, `crates/conflict-engine`, ...) hold the rules and are unit/property tested in isolation.
- One binary crate `crates/kizuna-api` (axum) owns I/O: PostgreSQL, object store, HTTP, worker invocation.
- Python extraction runs as an isolated subprocess worker (no DB credentials, no network, resource limits), talking
  JSON over stdin/stdout. Not a network service.
- Web UI is a static TypeScript app served separately (dev) and talking only to the JSON API.
- A crate is created only when it has real code; module names from directive §6 that have no code yet are listed in
  ARCHITECTURE.md as planned, not created as empty placeholders.

## Consequences
Single deployable, simple ops; boundaries enforced by crate dependencies (engines must not depend on the API crate).
Horizontal scale-out later needs shared rate-limit state (see ADR 0008).

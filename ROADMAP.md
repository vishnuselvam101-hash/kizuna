# Roadmap (directive §15)

| Milestone | Scope | State |
|---|---|---|
| M0 | inspection, research registers, ADRs, threat model, data model, test plan | done |
| M1 | structure, migrations, auth, tenants, authz, audit, config, CI, tests | done (see BUILD_STATUS) |
| M2 | artifact ingestion, object store, hashing/integrity, claims, provenance, evidence APIs + viewer | next |
| M3 | PDF/spreadsheet extraction, identity matching, spec comparison, revisions, conflicts, review | |
| M4 | valid/transaction time, decisions, typed dependencies, materiality, selective invalidation, traces | |
| M5 | detection rules, containment, quarantine, revocation, incident dashboard, recovery | |
| M6 | E2E, security review, benchmarks, backup/restore, deployment docs, demo | |

Backlog: WebAuthn; breached-password check; key rotation; signed audit checkpoints; shared rate-limit store;
dependency major upgrades (see dependencies.md).

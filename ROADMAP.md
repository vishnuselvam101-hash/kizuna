# Roadmap (directive §15)

| Milestone | Scope | State |
|---|---|---|
| M0 | inspection, research registers, ADRs, threat model, data model, test plan | done |
| M1 | structure, migrations, auth, tenants, authz, audit, config, CI, tests | done (see BUILD_STATUS) |
| M2 | artifact ingestion, object store, hashing/integrity, claims, provenance, evidence APIs + viewer | done |
| M3 | PDF/spreadsheet extraction, identity matching, spec comparison, revisions, conflicts, review | done |
| M4 | valid/transaction time, decisions, typed dependencies, materiality, selective invalidation, traces | done |
| M5 | detection rules, containment, quarantine, revocation, incident dashboard, recovery | done |
| M6 | E2E, security review, benchmarks, backup/restore, deployment docs, demo | done (pilot demo; not production) |

Backlog: seccomp for the worker; shared rate-limit store; UI switch to the new summary/paging endpoints;
dependency major upgrades (see dependencies.md).

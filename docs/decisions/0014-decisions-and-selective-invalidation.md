# ADR 0014: Decision records, typed dependencies and selective invalidation

Status: accepted (2026-11-05)

## Decision
- A decision is a human record (`decisions`): kind, subject, outcome, rationale, decided_by/at (transaction time),
  optional effective_from (valid time) and a **derivation** snapshot (evidence values, sources, artifact hashes,
  review states, open findings, rule/mapping/engine versions) with SHA-256 over its canonical JSON. Content is
  immutable (trigger); only status changes: valid → potentially_affected → superseded/revoked (final).
- Dependencies are typed (`basis|supporting|condition`) and point to a claim or an upstream decision, with
  applicability (revision, supplier) and a materiality rule: `any_change` (unit-aware), `numeric_abs{threshold}` in the
  snapshot's unit, or `informational`.
- On evidence change (supersession, rejection in review, new claim) the hook runs in the same transaction:
  (1) semantic change from→to; (2) dependencies on that claim **or any earlier version of it**, or on the same
  subject+predicate for new evidence; (3) applicability and materiality against the value the decision used (so
  cumulative drift across several small corrections is caught); (4) material → `potentially_affected`;
  (5) propagate breadth-first only through material decision→decision edges; (6) the previous decision stays;
  (7) an `invalidation_events` row records cause, change, materiality, reason and path — also for immaterial changes;
  (8) a reviewer reconfirms (new decision version against current evidence, old one `superseded`) or revokes
  (dependents are flagged). KIZUNA never re-decides.
- Trace endpoint answers: evidence used, what was known at decision time (transaction-time query), what changed
  afterwards, invalidation history, and whether the stored derivation still hashes to its recorded digest.
- Verification report (JSON/Markdown) bundles trace, findings, reviews, source document hashes and audit-chain status
  with its own SHA-256. It states that hashes prove integrity since capture, not truthfulness.

## Consequences
Propagation loads all decision→decision edges of the tenant (fine at SME scale; measure in M6). Materiality rules are
chosen by the person recording the decision; defaults to `any_change`.

# Evidence and claim data model (v1)

Status: designed in M0; tables created in M2 (artifacts, claims, provenance), M3 (entities, conflicts, reviews),
M4 (decisions, dependencies). Version: `claim_model_version = 1`.

## Principles
- Original artifacts are immutable and content-addressed (SHA-256). Derived data never overwrites the original.
- Claims are append-only. A correction is a new claim that **supersedes** the old one; the old one stays queryable.
- Two time axes: **valid time** (`valid_from`, `valid_to`: when the asserted fact holds in the real world, e.g. a
  revision's effective date) and **transaction time** (`recorded_at`, `superseded_at`: when KIZUNA believed it).
- Every extracted claim points to its source location where possible.
- AI or heuristic extraction is never "verified": `review_status` starts at `unreviewed`.

## Entities (tables)

### artifacts (M2)
`id, tenant_id, sha256 (bytea 32), size_bytes, media_type (validated by magic bytes), original_filename (display only,
never a path), kind (spec_document|drawing|purchase_order|inspection_report|spreadsheet|other), status
(quarantined|accepted|rejected), uploaded_by, uploaded_at, storage_key`. Unique (tenant_id, sha256).

### claims (M2)
| Column | Meaning |
|---|---|
| id, tenant_id | identity, RLS |
| subject_type, subject_key | what the claim is about: `part`, `supplier`, `purchase_order`, `drawing`, `inspection_lot`; key = normalised identifier |
| predicate | controlled vocabulary per domain pack (e.g. `mfg.revision`, `mfg.material`, `mfg.dimension.length`, `mfg.tolerance.plus`, `mfg.approval.status`, `mfg.expiry_date`) |
| value_text / value_num (numeric, exact decimal) / value_unit | asserted value; numbers are decimal strings parsed to NUMERIC, never floats |
| scope (jsonb) | applicability: e.g. `{"part": "...", "revision": "C", "supplier": "..."}` |
| valid_from, valid_to | valid time (nullable = unknown) |
| recorded_at, superseded_at, superseded_by | transaction time; only `superseded_*` may be set later |
| revision | document/part revision the claim was taken from |
| artifact_id + source_location (jsonb) | `{"page":3,"table":1,"row":4,"col":2}` or `{"sheet":"Spec","cell":"C7"}` or `{"record":"po-line-3"}` |
| extraction_method | `manual`, `rule:<name>@<version>`, `worker:<name>@<version>` |
| confidence_bps | 0–10000 integer, method-specific, never presented as probability of truth |
| review_status | `unreviewed`, `accepted`, `rejected`, `needs_evidence` |
| provenance (jsonb) | worker version, input artifact hash, parameters; enough to reproduce the extraction |

### entities (M3)
`entity_id, tenant_id, entity_type, canonical_key`, plus `entity_identifiers(raw, normalised, source claim)` and
`entity_match_decisions(left, right, outcome match|possible_match|no_match|needs_review, reasons, decided_by)`. Ambiguous
pairs are never auto-merged. Identity is separate from specification compatibility.

### conflicts (M3)
`id, tenant_id, kind (revision_mismatch|unit_incompatible|value_mismatch|tolerance_violation|missing_approval|expired_evidence|wrong_applicability|conflicting_supplier_record|conflicting_inspection),
claim_a, claim_b (nullable for single-claim findings), explanation (jsonb: rule, compared values, units), status
(open|resolved|dismissed), detected_at, rule_version`. Both claims always remain.

### reviews (M3)
`review_items(target conflict/claim, decision approve|reject|request_evidence, reviewer, comment, decided_at)`; append-only.

### decisions and dependencies (M4)
`decisions(id, tenant_id, kind, subject, outcome, decided_by, decided_at, status valid|potentially_affected|superseded,
derivation (jsonb snapshot of inputs + rule versions), derivation_sha256)`.
`decision_dependencies(decision_id, depends_on_claim | depends_on_decision, role, applicability scope, materiality rule)`.
`invalidation_events(decision_id, cause, semantic_change, material bool, reason, recorded_at, previous_status)`.

## Temporal queries (M4)
- Evidence for a decision: dependencies of the decision.
- What KIZUNA knew at decision time T: claims with `recorded_at <= T AND (superseded_at IS NULL OR superseded_at > T)`.
- What changed afterwards: claims for the same (subject, predicate, scope) with `recorded_at > T`.
- Potentially affected decisions: selective invalidation (directive §8) over typed dependencies.

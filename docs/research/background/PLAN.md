# KIZUNA vs Chuo-Sogo Procure: comparison and build plan

Date: 2026-11-05. Inputs: the KIZUNA brief (pasted by the founder, `/root/kizuna/BRIEF.md`) and the Procure codebase at
`/root/procurement-platform` (v1.1.0, migrations 0001-0029). Read-only review; no code changed.

## 0. One-paragraph answer

Procure already does a narrow, procurement-only version of most KIZUNA ideas: append-only hash-chained audit
(`audit_events`), immutable decision snapshots (`comparison_runs.quotation_ids` with stale detection), deterministic pure
decision functions (`crates/domain`), human-confirmed entity resolution (`item_aliases` + `domain/aliases.rs::similarity`),
provenance for AI output (`ai_suggestions` with input/prompt SHA-256) and an exception inbox (`agent_exceptions`). What it
lacks is a **general evidence graph**: an explicit decision-to-evidence link table, bitemporal evidence, conflict records,
dependency-driven invalidation, replayable derivations and third-party verification. Recommendation: build KIZUNA first as
an "evidence and decision integrity" layer inside Procure (phases K0-K6 below), then extract it if pilots justify a product.

## 1. Capability comparison

Gap: S = small (wire existing parts), M = new tables + module, L = new subsystem, XL = research.

| KIZUNA capability | What Procure already has (reference) | Gap |
|---|---|---|
| Connectors ERP/MES/IoT CSV | CSV/Excel/PDF/OCR purchase-history import (`0013_purchase_history.sql`, `api/history.rs`); ERP PO/receipt/invoice import profiles with staged rows and file SHA-256 (`0028_integrations.sql`, `server/integrations.rs`, `agents/import.rs`); supplier mail/.eml intake (`0020`, `intake.rs`) | S for procurement data. MES/IoT: L, and out of Procure's scope |
| DATA_RECORD with provenance | `history_imports` + `purchase_history.source_row`, `import_batches`/`import_rows.source_rows`, `attachments.sha256`, `ai_suggestions.input_sha256/prompt_sha256/model` | S: rows already carry source + row; missing a uniform `data_record` view |
| Entity resolution (score, merge/create, provenance) | Items: `item_key` normalisation (`domain/history.rs`), `similarity` proposals (0-1000), human-confirmed one-level aliases (`0015_item_aliases.sql`). Suppliers: platform `suppliers` directory, but `purchase_history.supplier_name` is free text, not resolved | M: add supplier resolution, ER decision log, labelled benchmark. No measured F1 today |
| Bitemporal evidence store | Valid time partly (`purchased_on`, delivery dates); transaction time via `created_at` + append-only tables + audit log. No "as believed at t" query | M |
| Applicability score A(d,c) | Implicit: baseline must predate agreement start and state its unit (`savings_baselines`, `domain/savings.rs`); comparison uses only this factory's data (`domain/shortlist.rs`); FX only from recorded rates | M: explicit integer score (bps, no floats) with reasons |
| Conflict-preserving reasoning | 3-way match keeps both sides as `discrepancies(expected, actual)` (`0007`, `domain/matching.rs`, tolerances `0024`); baseline `mismatch` status in `savings_entries` | M: general `evidence_conflicts` + decision-touched-by-conflict report |
| Selective invalidation (DAG + BFS) | One edge only: recommendation refuses `stale_comparison` when quotation set changed (`api/approvals.rs::comparison_is_stale`); `history_imports.withdrawn_at` cascades rows but no decision is flagged | M: dependency table + BFS + alerts via `agent_exceptions` |
| Reproducible derivation | Pure deterministic domain fns: `comparison.rs`, `decision.rs` (scored memo), `savings.rs`, `matching.rs`, `approval.rs`; snapshot `comparison_runs.results`. Inputs/versions not stored for replay | M: store canonical inputs + fn version + output hash; replay 100% |
| Decision store / API | Decisions are scattered: `approval_records`, `recommendations`, `savings_baselines.confirmed_*`, `discrepancies.resolution`, `agent_exceptions.decided_*`, `product_verifications`, `customer_supplier_approvals` | M: one `decision_records` index over them |
| UI: evidence behind a decision, "changed since" | `RecommendationPanel.tsx` (memo reasons), `Savings.tsx` (line reasons), `Audit.tsx`, `AgentExceptions.tsx` | M: Evidence panel + invalidation inbox |
| Privacy-preserving verification | Per-org SHA-256 audit chain computed in Postgres (`server/audit.rs`), `verify.rs` recomputes chains and file hashes | S-M for commitments + Merkle proofs; ZK = XL (research) |
| Cross-company evidence graph | Deliberately absent: strict RLS tenancy (`TenantCtx`, D3/D4), "other factories' data is never used" | L, and a policy decision, not only code. Only commitments/proofs may cross tenants |
| Multi-tenant, RBAC, audit, i18n, OpenAPI, desktop | Done (RLS FORCE, `domain/rbac.rs`, ja/en/ta/hi, utoipa, Tauri 2) | none: reuse |

## 2. Recommendation: build inside Procure first

Decision: **KIZUNA = an "evidence and decision integrity" module of Procure** (`crates/domain/src/evidence/*`,
`crates/server/src/evidence/*`), scoped to procurement decisions. Not a separate product yet.

Reasons:
1. Real decisions and real evidence already flow through Procure (supplier choice, price acceptance, baseline and savings
   fee, match tolerance acceptance, agent proposals). A standalone KIZUNA would start with synthetic data and no customer.
2. It directly protects revenue: the savings-share fee depends on a baseline and a reconciled price; an auditable,
   replayable fee derivation is a sales argument to a ¥10bn+ factory and reduces billing disputes.
3. 70-80% of the infrastructure (tenancy, RLS, audit chain, job queue, exception inbox, import pipeline, desktop UI,
   OpenAPI) is reused unchanged; one team, one binary, one gate (`scripts/gate.sh`).
4. It keeps Procure's non-negotiables: KIZUNA flags and explains, humans decide; invalidation raises exceptions, never
   auto-reverses an approval.
5. The brief's hypotheses can be measured on Procure data shapes (BOM/part revision mismatch ~ item alias / MPN
   conflicts; duplicates ~ duplicate history rows and supplier names) before investing in MES/IoT/PLM connectors.

Extraction later (a standalone KIZUNA) would reuse: `crates/domain` evidence modules (pure, no I/O), the evidence schema
(0030-0035) minus procurement FKs, `audit.rs` hashing, `verify.rs`, the agents queue/exception pattern, the import
staging pattern (`0028`), RLS/tenancy scaffolding and the Tauri shell. Keep the evidence crate free of procurement types
from day one (adapters map Procure tables into it) so extraction is a crate move, not a rewrite.

## 3. Phased build plan

Conventions for every phase: migrations owned by `procure`, RLS ENABLE+FORCE on `org_id = app_org_id()`, runtime grants
minimal and append-only where possible, audit event in the same transaction, no floats (scores in basis points 0-10000),
locale keys in `apps/desktop/locales/{ja,en}/evidence.json`, `#[sqlx::test]` with tenant-denial tests, OpenAPI regenerated.

### K0 - Synthetic benchmark and baseline measurement (1-2 weeks)
- `crates/evidence-bench` (dev-only binary): seeded generator for a synthetic factory (items with MPN variants and
  revisions, suppliers with name variants half/full-width and kana, purchase history, quotes, orders, invoices) with
  **injected faults and ground truth**: duplicate records, BOM/part revision mismatches, conflicting prices for the same
  item+date, unit mismatches, withdrawn imports. Output: CSVs importable through existing endpoints + `truth.json`.
- Measure current state: `domain/aliases.rs::similarity` as an ER baseline (precision/recall/F1 at MIN_SIMILARITY).
- Acceptance: generator deterministic by seed (same seed -> byte-identical files); baseline F1 recorded in
  `reports/kizuna/K0_baseline.json`. No product change.

### K1 - Evidence and decision records (migrations 0030-0031)
- 0030 `evidence_items`: id, org_id, entity_type, entity_key, attribute, value jsonb, unit, source_type
  (`purchase_history|import_row|quotation_line|invoice_line|ai_suggestion|attachment|manual`), source_id, source_sha256,
  valid_from, valid_to (valid time), recorded_at, superseded_at, superseded_by (transaction time; rows never updated
  except `superseded_*` via column grant). View `evidence_as_of(org, t_valid, t_recorded)`.
- 0031 `decision_records` (kind: `supplier_selection|price_acceptance|baseline_confirmation|savings_line|`
  `tolerance_acceptance|agent_decision|alias_confirmation`, subject_type/id, decided_by, decided_at, status
  `valid|needs_review|superseded`) and `decision_evidence(decision_id, evidence_id, role, applicability_bps)`.
- Rust: `domain/evidence/{model,bitemporal}.rs` (pure), `server/evidence/{capture,repo}.rs`. Capture hooks inside the
  existing transactions: `approvals.rs` (approve/reject), `savings.rs` (baseline confirm, entry write),
  `matching.rs`/discrepancy resolve, `agents/exceptions.rs::decide`, alias create. Backfill job for existing rows.
- API: `GET /api/v1/orgs/{org}/decisions?kind=`, `GET .../decisions/{id}` (with evidence), `GET .../evidence?as_of=`.
- Acceptance: every decision kind above writes a record + links in the same tx (test per kind); as-of query returns the
  belief at t for 100% of generated supersession cases; tenant-denial tests pass.

### K2 - Entity resolution with provenance (migration 0032)
- 0032 `er_candidates` (left/right keys, kind item|supplier, score, features jsonb, status proposed|merged|rejected,
  decided_by) and `supplier_aliases` (history `supplier_name` -> `suppliers.id`, human-confirmed like `item_aliases`).
- Rust: `domain/evidence/er.rs` - feature scoring (normalised MPN, manufacturer, kana/half-width folding, revision
  suffix parsing, token Jaccard), blocking by normalised prefix; integer scores. Merges stay human-confirmed (proposals
  go to `agent_exceptions` via a new `evidence` agent).
- UI: `ResolveEntities.tsx` (candidate pairs, features, confirm/reject).
- Acceptance on K0 data: **ER F1 >= 0.90** (pairwise, items and suppliers separately, at the proposal threshold);
  report precision/recall; target 0.95 stated as stretch, not promised.

### K3 - Applicability and conflicts (migration 0033)
- Applicability A(d,c) in bps from explicit rules with reasons: same item group, same unit, same currency or recorded
  FX, valid time covers decision date, source trust (factory-imported > staff-imported > AI-extracted unconfirmed),
  age decay. Pure fn `domain/evidence/applicability.rs`, versioned (`rule_version`).
- 0033 `evidence_conflicts` (evidence_a, evidence_b, kind `value_mismatch|unit_mismatch|revision_mismatch|duplicate|`
  `temporal_overlap`, detected_at, status open|explained|dismissed, note). Both evidence rows kept; nothing deleted.
- Detector runs on evidence insert (same tx, indexed by entity_key+attribute) plus a nightly sweep agent.
- Decision view lists conflicts touching its evidence; existing `discrepancies` mirrored as conflicts.
- Acceptance on K0 data: **conflict recall >= 0.85** (target 0.90), precision reported; A(d,c) unit tests cover every rule.

### K4 - Selective invalidation (migration 0034)
- 0034 `decision_dependencies` (decision -> evidence and decision -> decision edges, acyclic enforced in Rust) and
  `decision_invalidations` (decision_id, cause_evidence_id, path jsonb, created_at, handled_by/at).
- Triggers for invalidation: evidence superseded, import withdrawn (`history_imports.withdrawn_at`), alias removed,
  quotation replaced, FX rate corrected, conflict opened on used evidence.
- Rust: `domain/evidence/invalidate.rs` BFS over an adjacency snapshot (pure, testable), server marks
  `decision_records.status = needs_review` and raises one `agent_exceptions` row per decision (kind
  `decision_invalidated`). Never reverses an approval or a billed fee automatically; a human decides.
- UI: Invalidation inbox (reuse `AgentExceptions.tsx` filters) and a "changed since decision" badge.
- Acceptance: BFS marks exactly the ground-truth dependent set (0 false negatives, 0 false positives on generated DAGs);
  **invalidation latency p95 < 1 s** from evidence change to exception row, measured on 100k evidence / 10k decisions.

### K5 - Reproducible derivation (migration 0035)
- 0035 `derivation_steps` (decision_id, step_no, kind fact|rule|inference, rule_id, rule_version, inputs jsonb canonical,
  output jsonb, output_sha256) and `decision_records.derivation_root` (hash over steps).
- Wrap the pure fns as named rules: `comparison::compare`, `decision::memo`, `savings::line`, `matching::classify`,
  `approval::required`. Each records canonical inputs (sorted JSON, integer money) and `CARGO_PKG_VERSION`+rule version.
- `POST .../decisions/{id}/replay` re-executes from stored facts; mismatch -> conflict + exception.
- Acceptance: **100% byte-identical replay** for all decisions in the K0 dataset and a backfilled demo DB; property test
  (proptest) that canonicalisation is order-independent; rule version bump keeps old derivations replayable.

### K6 - Verifiable decisions without raw data (migration 0036)
- Commitments: per decision, `commit = SHA-256(salt || derivation_root || evidence hashes)`; salt kept in Procure.
- Merkle tree per org per day over commitments; root anchored as an `audit_events` row (existing hash chain).
- Proof bundle export: decision commitment, Merkle path, day root, audit chain segment; optional selective disclosure of
  chosen evidence values with their salts.
- `procure-verify` standalone CLI (no DB access) verifies a bundle offline.
- UI: "Export proof" on decision detail; admin page for daily roots.
- Acceptance: tampering any byte fails verification (fuzz test); verification time per bundle measured and reported
  (target < 50 ms on this server); no raw evidence in a bundle unless explicitly disclosed.
- **Not MVP:** ZK proofs (e.g. proving "fee = 50% of a saving > 0" without revealing prices) and cross-company graph
  sharing. Research track only, after K6 and a pilot ask for it.

### Sequencing
K0 -> K1 -> (K2 and K3 in parallel) -> K4 -> K5 -> K6. K1 is the only blocking schema. Each phase ends with
`scripts/gate.sh full`, code-reviewer + cybersecurity-expert agents, and a metrics JSON in `reports/kizuna/`.

## 4. Who does what

Founder only:
- Patent/FTO search and filing strategy with a registered patent attorney (弁理士), JP and US; decide before any public
  demo or paper about hypotheses 2-5 (novelty can be lost by disclosure).
- Trademark search for "KIZUNA" (very common word in JP; J-PlatPat, USPTO, EUIPO) and the domain/handles.
- Whether KIZUNA is a Procure feature name or a separate company/product; pricing.
- Funding, grants (e.g. NEDO/SBIR-type programs), investor conversations.
- Pilot customers and data-sharing agreements; any cross-company data policy (it changes Procure's "never use other
  factories' data" rule and the customer contract).
- Approval to deploy, sign, or touch the live/demo servers.

Claude can build: K0-K6 code, migrations, tests, benchmark generator, metrics reports, UI screens and locale keys,
OpenAPI, docs/ADRs, a competitor fact sheet from public sources (with links), and a draft invention-disclosure document
for the attorney (technical description only, not legal advice).

## 5. Risks and verify list

Unverified claims in the brief (treat as false until checked):
| Claim | Status | How to verify |
|---|---|---|
| US12,632,426B2 (Blue Yonder) | UNVERIFIED number and content | USPTO Patent Center / Google Patents: assignee, claims, status, family |
| US20260127456A1 (Itemize) | UNVERIFIED | Same; check publication exists and its claims |
| IBM patent | The brief itself says "assumed": no source | Ignore until a real number is found |
| Benchmark "Result" table (ER F1, recall, latency, proof time) | Placeholder numbers, not measurements | Replace with K0-K6 measured JSON; never quote in pitch material until measured |
| Competitor gaps (Palantir Foundry, Siemens Teamcenter X, SAP PLM/MDM, Collibra, IBM Maximo, Catena-X) | Not checked | Public docs per vendor with links and dates; Foundry and Collibra already do lineage/provenance, Catena-X does cross-company data exchange: the novelty claim must be narrowed |
| "Datalog-with-provenance" novelty | Prior art exists in academia (provenance semirings, why-provenance) | Attorney + literature search before claiming |
| Market size / GTM numbers for Japan | Unsourced | Founder research with sources |

Product and technical risks:
- Scope creep: MES/IoT/PLM connectors and cross-company sharing are each a product. Mitigation: procurement only until a
  pilot asks.
- Capture completeness: a decision path that skips the capture hook silently breaks the graph. Mitigation: one test per
  decision kind + a nightly check "decisions without evidence links".
- Write amplification and latency on the business transaction. Mitigation: evidence capture is small inserts; heavy
  conflict sweeps run in the worker.
- Synthetic-data bias: metrics on our own generator flatter us. Mitigation: hold-out fault types, and re-measure on a
  pilot's anonymised data before any external claim.
- Commitments leak via low-entropy values (prices are guessable). Mitigation: per-decision random salt; salts never
  exported unless disclosed.
- Determinism breaks on dependency upgrades (sorting, JSON). Mitigation: canonical JSON with integer money only, rule
  versions, replay test in the gate.
- Trust: an "invalidated" flag on an approved order or billed fee could alarm customers. Mitigation: wording and a human
  review step; never automatic reversal.

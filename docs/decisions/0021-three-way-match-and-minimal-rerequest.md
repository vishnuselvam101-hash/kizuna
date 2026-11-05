# ADR 0021: Quality three-way match and minimal supplier re-requests

Status: accepted (2026-11-05). Research basis: `docs/research/MARKET_GAPS_2026-11.md` I1 and I4 (KIZUNA's own design;
no competitor behaviour is reproduced).

## Decision
**Three-way match (`mfg-match@1`, domain pack `three_way.rs`).** A PO line (purchase order + line + part, from
`purchase_order` claims) is matched against the design side (claims from `drawing`/`spec_document` artifacts of the
ordered revision, plus unrevisioned ones; supplier-specific specs only for the PO's supplier) and the certificate side
(`inspection_report` claims of the part whose PO/line scope, when stated, is this line). Sides come from the artifact
kind, never from document text; manual claims fill no side. Checks: revision (missing / unknown / stale ordered rev,
certificate inspected another rev), drawing number, drawing approval, material grade (PO↔drawing and certificate↔
required, presentation-insensitive key), EN 10204 type (supplied rank ≥ required; uncomparable text is a finding,
not a pass), toleranced dimensions (unit-aware), one-sided property limits (`mfg.property.X.min|max`), traceability
(lot or heat per certificate). Each finding has a fixed severity (critical / major / minor table in code) and cites
claim ids per side; the API resolves them to document, SHA-256 and exact source location. No cascade: an unresolved
revision skips design checks; no certificate skips certificate checks.

**Acceptance.** `POST /match/line/accept` records a `po_line_acceptance` decision (subject `po_line`, key `PO/line`)
whose typed dependencies are exactly the cited claims (PO = `condition`, drawing and certificate = `basis`,
materiality `any_change`), so M4 selective invalidation reaches it unchanged. `approved` is refused while critical or
major findings exist; `conditional`/`rejected` record the reviewer's judgement. The evaluation is stored append-only in
`match_runs`/`match_findings` with its canonical-JSON SHA-256, referenced from the decision derivation.

**Minimal re-request (`mfg-resupply@1` + `dependency_engine::cover`).** For `potentially_affected` decisions, each
material invalidation event names the changed predicate. The domain table `needs_resupply(changed, candidate)` says
which supplier statements become stale (tolerance/dimension → that measurement; material → material; property limit →
that property; cert type → type; revision/drawing number → re-inspection; a withdrawn/corrected supplier claim → the
same statement; approvals, expiry, quantity → nothing). Items = stale statements the decision depends on (part, PO
line, lot/heat, predicate); candidates = current supplier documents carrying that item. The generic core picks the
fewest documents (exact search up to 16 optional documents, else greedy + pruning, flagged `exact=false`). Baseline =
every supplier document the flagged decisions rely on. Drafts (JA business format + EN) are stored immutably with a
SHA-256; status moves draft → approved | cancelled only; approval needs `review_decide` (MFA session) and a person
other than the drafter. Download is UTF-8 text marked DRAFT until approved; PDF is the browser's print-to-PDF of the
letter (no server PDF/CJK font dependency). KIZUNA never sends e-mail.

## Consequences
- Mapping moves to `mfg-map@2`: heat number (scope), certificate type, `po.`/`qa.drawing_number` (a PO is no longer
  design evidence), property bounds (`min 520`, `≥520`, `520以上`). Existing claims keep their recorded method string.
- Synthetic E2E (`scripts/e2e.sh`) measures match precision/recall per (PO line, kind) and re-request precision/recall
  vs ground truth plus the naive baseline. The generator and rules were built together, so these numbers show the
  pipeline (real extraction, API, DB) works on known layouts; they are not evidence of accuracy on real documents.
- Schema: migration `0018_innov_match.sql` (RLS FORCE, select/insert grants, column-limited update on requests).

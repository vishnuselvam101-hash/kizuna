# ADR 0029: Cross-standard grade guard (tenant-curated, never authoritative)

Status: accepted (2026-11-06). Research basis: `docs/research/MARKET_GAPS_2026-11.md` I9 (KIZUNA's own design).

## Context
Japan/India supply chains mix material standards (JIS, IS, ASTM/EN). A mill sheet that names a grade from another
standard than the drawing is a *substitution*, not a typo, and "equivalent" grades are not identical. Standards text
is copyrighted, and equivalence or chemistry tables typed from memory (by a person or a model) are not verified.

## Decision
**No shipped content.** KIZUNA ships notation rules only (`mfg-grade@1`, `domain-manufacturing/src/grades.rs`): a
leading standards-body token names the system, an optional document number follows, presentation (width, case,
separators, filler words) is ignored, and a one-letter + five-digit designation is in the UNS shape. No grade names,
no equivalences, no limits. Tests and the example CSV (`docs/examples/grade-equivalences.example.csv`) use fictional
grades (XQ…, ZR…, K9…).

**Tenant tables, four-eyes, versioned (migration 0033).** `grade_equivalences` (pair of grades; `identical`,
`acceptable_with_conditions` with conditions, or `not_equivalent`) and `grade_limits` (grade, property key,
min/max, unit). Rows are proposals (typed in, or imported all-or-nothing from the tenant's CSV) that a *second*
person approves; content is immutable, rows are never deleted, edits and retirements are new versions that supersede
the approved one. The database enforces append-only, forward-only status and approver ≠ proposer.

**Never authoritative.** Every entry carries `verification`: `unverified` ("unverified — check against the standard")
by default, `tenant_checked` only when the proposer named the standard document in `reference` and a second person
approved. Imports are always `unverified`. Findings, material bases and API rows always carry the label and a note
that the entry is a tenant statement, not an authoritative standards lookup.

**Match (`mfg-match@2`).** Certificate grade vs required grade: same by notation → pass. Otherwise the tenant's
approved table decides: `identical` → accepted, recorded in `material_basis` (the acceptance depends on that entry
version); `acceptable_with_conditions` → `substitution_requires_decision` (critical) until a `material_substitution`
decision covers exactly that certificate claim, reference claim and entry version; `not_equivalent` → material
mismatch citing the entry. Without an entry: a value naming another system, or no system while the other side names
one, is `substitution_requires_decision`; same-system differences stay `certificate_material_mismatch`. A substitution
can only be decided under an approved entry (`no_equivalence_entry` otherwise): KIZUNA never decides an equivalence.

**Names equal, values fail.** For certificates whose grade is accepted as the required grade, reported
`qa.property.<p>` values are checked against the tenant's approved limits of the required grade
(`grade_property_out_of_limits`, major). The drawing governs: a property the drawing limits is not checked twice,
and a value the certificate does not report is not a finding here.

**Invalidation.** Decisions depend on equivalence versions (`decision_dependencies.depends_on_equivalence`, only
approved versions accepted by a trigger). Approving an edit or a retirement flags those decisions and their
downstream decisions `potentially_affected` (`invalidation_events.cause_equivalence`); a reconfirmation cannot carry a
superseded entry over (`grade_equivalence_changed`).

## Not done / limits
Limit changes do not invalidate earlier acceptances (they only add findings on the next evaluation). No UI screen yet
(API only; error texts EN/JA in `web/src/i18n-grades.ts`). Parsing is heuristic: "K9 220" vs "K12 220" style
designations are read with and without the document-number interpretation. More than the inline limit of affected
decisions is refused rather than queued as a review batch.

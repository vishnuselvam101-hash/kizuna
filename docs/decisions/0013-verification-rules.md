# ADR 0013: Deterministic entity resolution and conflict rules

Status: accepted (2026-11-05)

## Decision
- Entity resolution (`crates/entity-resolution`): safe normalisation (full-width→ASCII, case, whitespace) = automatic
  Match. Separator differences, revision suffixes in identifiers and 1-character differences (ids ≥ 6 chars) =
  PossibleMatch. A key with ≥2 possible matches = RequiresHumanReview. Nothing beyond safe normalisation is merged
  without a human decision (`entity_decisions`, append-only; `entity_aliases`). Identity ≠ specification compatibility.
- Conflict rules (`crates/conflict-engine` machinery + `domain-manufacturing::rules`, version `mfg-rules@1`):
  spec value mismatch / unit incompatibility (exact decimal conversion, `rust_decimal`), revision mismatch (ordered or
  inspected revision unknown or older than latest), missing approval for the ordered revision, expired certificate,
  tolerance violation (measured vs nominal ± tolerance of the same revision), wrong applicability (PO supplier not
  covered by spec scope), conflicting inspection results per lot, conflicting supplier records per PO.
- Findings reference both claims and explain the rule; claims are never changed by detection. Findings that no longer
  fire become `obsolete`; nothing is deleted. Review decisions (approve = documented waiver, reject, request
  evidence) are append-only with mandatory comments for approve/reject; manual claims cannot be reviewed by their
  author (ABAC `no_self_review`).

## Consequences
Rules are explainable and testable but only as good as the extracted values; precision/recall on real documents is
unmeasured (research register R2/R4). Detection loads all current part claims of a tenant into memory (fine for SME
volumes; to be measured in M6).

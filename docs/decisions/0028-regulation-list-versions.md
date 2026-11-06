# ADR 0028: Regulation-list versions as dependency nodes (research I6, `mfg-reglist@1`)

Status: accepted (2026-11-06). Migration 0031.

Context: supplier substance declarations (RoHS/REACH style) answer one version of a regulated-substance list. When
the list changes, the usual answer is a calendar-driven blanket re-survey of every declaration
(docs/research/MARKET_GAPS_2026-11.md §2.3, §2.4, I6). KIZUNA already flags decisions selectively when evidence
changes; a list change was invisible to it.

## Decision
- **No shipped list content.** A tenant registers a list (`POST /reg-lists`, code such as `svhc`) and imports each
  version from a released `regulation_list` CSV upload (`POST /reg-lists/{id}/versions`). Columns are matched by
  EN/JA aliases (substance name, CAS, EC, date added); CAS and EC numbers must carry valid check digits; a version
  is a set (duplicate substances are refused). Substance identity = CAS, else EC, else the normalised name (group
  entries). Tests and docs use invented substances only.
- **Declarations are claims linked to a list version.** A `compliance_declaration` CSV becomes `reg.declaration`
  claims (subject = part, value = `compliant` / `contains_listed`, scope = supplier, list, list version, covered
  materials) plus a `reg_declarations` row naming the version answered. Decisions cite these claims as evidence like
  any other claim.
- **Composition and curation.** A `material_declaration` (FMD-style) CSV becomes `reg.composition` claims (part →
  homogeneous material, optional declared substance). Tenant-curated `material_profiles` (privileged, MFA, audited,
  superseded never rewritten) say which substances a material could contain; an empty profile means "curated as
  containing none of interest".
- **Materiality rule (`mfg-reglist@1`, pure code in `domain-manufacturing::reglist::assess`).** Added substances =
  new version minus the version the declaration answered. Verdict per declaration: `may_contain` when a covered
  material could contain an added substance (FMD or profile); `cannot_rule_out` when a covered material is
  described by neither (never assumed clean) or the part has no composition at all; else `not_affected`. Properties
  tested: "not affected" is sound, and flagging is monotone in the added set.
- **Invalidation reuses the dependency engine.** Flagged declarations seed `dependency_engine::propagate_seeds`
  (cause kind `regulation_list_changed`, message `invalidation.reglist`, semantic change with predicate
  `reg.list.<code>`, from/to labels and the verdict); decisions become `potentially_affected` and the flag travels
  through decision→decision edges exactly as for evidence changes. Each event names the declaration claim. Nothing
  is reversed; people re-review. The minimal re-request planner (I4) treats `reg.declaration` from a
  `compliance_declaration` as supplier evidence to ask for again.
- **Accountability.** Every declaration considered is stored in `reg_assessments` with its verdict (flagged or not),
  so "why was this not flagged" is answerable; the version row keeps a summary. All new tables are append-only,
  RLS-scoped per tenant and denied to supplier-portal sessions. Imports are audited; a document cannot be imported
  twice; `/extract` refuses these document kinds.

## Consequences / limits
- The assessment is only as good as tenant composition data and profiles; with none, every declaration is
  `cannot_rule_out` (equivalent to a blanket re-survey, never fewer flags than warranted).
- Removed substances never flag anything. Threshold/concentration and exemption semantics are not modelled.
- The list-change invalidation runs inline in the import transaction without the inline-limit review batch used for
  evidence changes; very large tenants may need it moved to the background job.
- Import errors report the error code, not the CSV line number (the line is known internally).

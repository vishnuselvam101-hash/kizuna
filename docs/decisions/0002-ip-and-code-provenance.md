# ADR 0002: Code provenance and relation to Chuo-Sogo Procure

Status: accepted (2026-11-05), revised the same day

## Context
`docs/research/background/PLAN.md` recommended building KIZUNA inside Chuo-Sogo Procure (`/root/procurement-platform`).
The founder directive (DIRECTIVE.md) instead defines KIZUNA Verify as a vendor-neutral standalone product with its own
core and domain packs. At the start of this build KIZUNA's owner was undecided, so the initial rule was: read Procure
for patterns, copy nothing.

Revision (2026-11-05, founder update relayed by the engineering coordinator): KIZUNA is owned by Chuo-Sogo Inc., the
same owner as Procure. Reuse/adaptation of Procure code is permitted where it fits, provided the KIZUNA core stays free
of procurement types.

## Decision
1. KIZUNA is a separate repository and product, not a Procure module (supersedes PLAN.md §2).
2. Procure code may be adapted. Any file adapted from Procure must say so in its module doc comment
   (`Adapted from Chuo-Sogo Procure <path>`), so provenance stays auditable.
3. Core crates (`*-engine`, `evidence-core`) must not reference procurement concepts (quotations, savings fees,
   suppliers-as-billing-entities). Manufacturing concepts live in a domain pack (`crates/domain-manufacturing`).
4. As of M1, no Procure code was copied: audit chain, RLS policies, auth, rate limiting were written fresh (Procure
   computes its audit hash in SQL; KIZUNA computes it in Rust with the `sha2` crate, ADR 0006).

## Consequences
Licensing/ownership of KIZUNA follows Chuo-Sogo Inc. Should ownership change again, the provenance comments identify
what must be rewritten.

# ADR 0026: Corrective-action cases (8D / 是正処置) on KIZUNA evidence

Status: accepted (2026-11-05). Context: findings (conflicts, three-way-match findings, integrity signals, revision
drift, SPC out-of-control) need a structured, evidence-bound path to a closed corrective action.

## Decision
- Pure model in `domain_manufacturing::capa`: `Case::apply` is the only transition function (complete step, reopen
  step, request/approve/reject closure, contradicted). Invariants (property-tested on random event sequences):
  completed disciplines form a prefix D1..Dk; `closure_pending` ⇒ D1–D7 done; `closed` ⇒ all done; approver ≠
  requester; errors never change state. `missing(d, content, facts)` lists stable requirement codes per discipline.
- Requirements: D1 ≥ 2 active users incl. a leader; D2 statement + is/is-not ("what" row) + cited evidence; D3 each
  containment action cites evidence; D4 ≥ 1 occurrence cause, every cause cites claims/documents; D5 every cause
  covered by an action (owner, due date); D6 all actions implemented (each with evidence), ≥ 1 citation recorded
  after the last implementation, and at least one of those of internal origin; optional SPC re-check: ≥ 8 points
  recorded after the fix, no Western Electric violation against the reference limits; D7 ≥ 1 measure + horizontal
  deployment (水平展開) or a reason it does not apply; D8 summary, then closure approved by a second person with
  `review_decide` (MFA). The DB also refuses `closed_by = closure_requested_by`.
- Opening (`POST /capa/cases`): from a finding id (evidence links derived from the finding) or from an SPC
  characteristic (I-MR chart over current numeric claims; refused when in control). Reference limits for D6 are
  recomputed without the WE1 points. One active case per finding.
- Supplier: bilingual 8D request drafts (immutable content, four-eyes approval, KIZUNA never sends, like evidence
  requests); uploaded response documents are linked with origin `supplier`. They are citable evidence only: they
  complete no step and alone cannot verify effectiveness.
- Reopening: `decisions::on_claim_change` calls `capa::on_claim_change` in the same transaction. A case with D6 done
  reopens D6–D8 (status `open`, `reopened` + 1 if closed) when a claim cited in D6 (directly or via its document) is
  superseded/withdrawn, or, for SPC cases, a new point lies beyond the reference 3σ limits.
- Overdue: `capa::sweep_overdue` (background every 5 min, `POST /capa/overdue/run`) inserts one in-app notification
  per open overdue action, recipient (owner, case opener) and due date; audited. Dates are UTC.
- Audit: every transition writes `capa_events` (case history) and the audit chain (`capa.*`). Migration 0028, all
  tables FORCE RLS.

## Not done
No supplier login/portal on main: responses are uploaded by staff and linked. No e-mail for overdue notifications
(in-app only; the alert channel is reserved for operator alerts).

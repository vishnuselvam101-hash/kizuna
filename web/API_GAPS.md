# API gaps found while building the web UI

The UI uses only the existing `/api/v1` routes. These gaps were noted instead of changing the Rust server.

| # | Gap | Current UI workaround | Proposed API |
|---|-----|-----------------------|--------------|
| 1 | Import settings (CSV/XLSX/PDF) cannot be stored | Admin → Import settings validates the form and exports JSON; a banner says it is not saved | `GET/PUT /integrations/import-settings` (tenant_admin, MFA, audited, versioned) consumed by the extraction worker |
| 2 | No dashboard summary | Overview loads `/artifacts`, `/claims`, `/conflicts` ×2, `/decisions`, `/audit/verify` | `GET /dashboard/summary` with counts by status |
| 3 | `/claims` has no paging or limit | Overview graph shows at most 300 current claims; Records groups the full list client-side | `limit`/`after` parameters and `GET /subjects?type=` listing distinct records with counts |
| 4 | Claims/evidence responses carry `artifact_id` only | Records joins against `/artifacts` for display names | Include `artifact_display_name` in `ClaimOut` |
| 5 | Decision list omits cited claim IDs | Org graph links decisions to records by `subject_key`; exact links are shown per decision in the trace | `evidence_claim_ids` in `DecisionOut`, or `GET /graph` |
| 6 | Entity decisions accept `subject_type = "part"` only | Duplicate review is shown for components only | Allow `supplier` |
| 7 | Revision comparison exists for parts only (`/compare/part/{key}`) | Comparison screen is component-only | `/compare/{subject_type}/{key}` |
| 8 | Reports are per decision; no organisation-level audit report | Reports screen generates per-decision Markdown/JSON and exports raw audit events as JSON | `POST /reports` (scope: date range / subject / all open findings) returning a hashed, signed report with `Content-Disposition` |
| 9 | Users screen cannot list a user's sessions | "End sessions" revokes all | `GET /users/{id}/sessions` |

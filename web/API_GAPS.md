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

## Status (integrator, 2026-11-05)

All nine gaps now have server endpoints with integration tests (`crates/kizuna-api/tests/m6_api_gaps.rs`):
1 `GET/PUT /integrations/import-settings` (versioned, optimistic concurrency, admin+MFA, audited) — **UI wired** (save + load).
Stored settings are not yet consumed by the extraction worker (worker uses its built-in limits).
2 `GET /dashboard/summary` · 3 `/claims?limit&after` + `GET /subjects?type&after&limit` · 4 `artifact_display_name` in claims ·
5 `evidence_claim_ids` in decisions · 6 supplier entity decisions · 7 `/compare/{subject_type}/{key}` ·
8 `POST /reports` (org report, hashed, Ed25519-signed when a key is configured) · 9 `GET /users/{id}/sessions` — **UI wired**.
Gaps 2–8: the API exists; the UI still uses its earlier client-side workarounds (switching is a follow-up).

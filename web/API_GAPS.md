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

## Status (2026-11-05, M7) — all nine gaps closed

Every gap has a server endpoint with integration tests (`crates/kizuna-api/tests/m6_api_gaps.rs`,
`m7_ai_settings.rs`) and the UI uses it; the client-side workarounds in the table above are removed.

| # | Endpoint | UI |
|---|---|---|
| 1 | `GET/PUT /integrations/import-settings` (versioned, optimistic concurrency, admin+MFA, audited) | Integrations → Import settings; **consumed by the extraction worker since M7** (CSV delimiter/encoding/header row, XLSX sheet/header row, OCR on/off + language, record/page/sheet limits, long-table column mapping); the run records `import_settings_version` |
| 2 | `GET /dashboard/summary` | Overview key figures |
| 3 | `/claims?limit&after`, `GET /subjects?type&after&limit` | Records paging ("Load more"; the identifier filter searches loaded pages only) |
| 4 | `artifact_display_name` in claims | Evidence tables |
| 5 | `evidence_claim_ids` in decisions | Org graph links decisions to their cited claims |
| 6 | supplier entity decisions | Duplicate review on Components and Suppliers |
| 7 | `/compare/{subject_type}/{key}` | Comparison screen with component/supplier selector |
| 8 | `POST /reports` (scope, date range; SHA-256, Ed25519-signed when a key is configured) | Reports → Organisation report (hash, signature state, JSON download) |
| 9 | `GET /users/{id}/sessions` | Users screen |

New in M7, with screens: `GET/PUT /integrations/ai-extraction` (Integrations → AI-assisted extraction; opt-in with
the provider data-processing notice, ADR 0018) and `GET/PUT /security/detection-settings` (Security → detection
thresholds, ADR 0015). `POST /artifacts/{id}/reextract` (reason) re-runs extraction after an import-settings change.

New with the scalable graph (branch `webgl-scale`, see PERF_GRAPH.md): `GET /graph/overview?after&limit` — keyset-paged
evidence graph (default 5000 claims per page, max 20000; claim/record nodes and their document, record and
decision links; the first page also carries every document, decision and open finding plus `claims_total`).
Requires claim and decision read. The Overview graph streams it until `next` is absent, replacing the old
first-300-claims cap.


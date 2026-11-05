# Security review: supplier portal RLS and token separation (2026-11-05)

Independent, read-only review of main d7b67c7 (supplier portal, ADR 0025; CAPA ↔ portal, ADR 0026 amendment).
Result: **High 0 · Medium 0 · Low 3 · Info 4**. All Low findings fixed on main with tests.

Held: tenant and portal extractors refuse each other's token kinds (separate session tables); `kzi.` invites hashed,
72 h, single use; supplier sessions 8 h max, 30 min idle, no refresh; portal transactions READ ONLY without tenant
context; deny-by-default RLS on every table incl. CAPA (migration 0029); cross-tenant and cross-supplier reads refused;
`supplier_id` set only by tenant staff; uploads size-capped while streaming, content-sniffed, scanned fail-closed,
supplier sees no internal ids or findings; CAPA linking only via the supplier's own scoped request.

| # | Sev | Finding | Status |
|---|---|---|---|
| L1 | Low | A reviewer (no MFA) could publish a CAPA letter to any portal supplier, permanently | **fixed**: `publish_to_portal` needs `UserManage` (admin + MFA + login ≤ 15 min) and the portal supplier must be the one the approved letter names (`supplier_mismatch`). Test `supplier_submits_8d_response_through_the_portal_as_evidence_only` |
| L2 | Low | Daily upload cap could be overshot by parallel uploads | **fixed**: cap re-checked and row inserted under a per-supplier advisory lock. Test `portal_daily_cap_holds_under_parallel_uploads_and_reads_are_rate_limited` (6 parallel, cap 2 → 2) |
| L3 | Low | Unlimited portal page views each append an audit row | **fixed**: all portal requests rate limited per supplier user (120/min). Same test |
| I1 | Info | Supplier users are outside incident containment | open: kill switch is `POST /portal-users/{id}/revoke`; containment for supplier principals is future work |
| I2 | Info | Byte-identical supplier upload links to the existing tenant artifact (provenance mix-up; capture row records `supplier_portal`) | documented |
| I3 | Info | `rejected` artifact branch in the status trigger is dead code | documented |
| I4 | Info | ADR 0025 drift (migration number, audit wording) | **fixed** in ADR 0025 |
Withdrawing a published request is not possible (approved evidence requests are final); revoke the supplier's portal
users if a request must be pulled back.

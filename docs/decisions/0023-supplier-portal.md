# ADR 0023: Supplier portal for answering minimal re-requests

Status: accepted (2026-11-05). Context: ADR 0021 (minimal supplier re-requests); suppliers answer by email today.

## Decision
- New principal type **supplier user** (`supplier_users`), scoped to ONE supplier record (`suppliers`, one tenant,
  matched by the re-request name normaliser). It has no role and no tenant permission. Its token is `kzp.` (tenant
  access tokens are `kza.`): the tenant `Principal` extractor refuses `kzp.` with 403 + `portal.denied`, the
  `PortalPrincipal` extractor refuses `kza.` the same way. Sessions: 8 h max, 30 min idle, no refresh token.
- Invitation by a tenant admin (`UserManage`, MFA-verified): single-use `kzi.` link token (hash stored, 72 h),
  carried in the URL fragment (`/portal#invite=…`) so it never reaches logs or Referer. Acceptance = password
  (policy as tenant users) + TOTP enrolment confirmed with a code; only then `active`. Every sign-in needs password
  AND code. Revocation voids the invitation and ends all sessions at once. Lockout/rate limits as tenant logins.
- Visible to the supplier: approved (`status = 'approved'`) requests linked to its supplier — reference, due date
  (`due_on`, set at approval, default +14 days), requested documents projected to name + item labels
  (no artifact ids, hashes, change reasons, decision ids, reviewers), the approved JA/EN letter; its uploads with
  `received | scanning | quarantined | accepted_for_review | rejected`. Nothing else.
- Database scope: `db::portal_tx` (READ ONLY) sets `kizuna.portal_tenant_id`/`kizuna.portal_supplier_id` and never
  `kizuna.tenant_id`, so tenant policies match nothing. Migration 0022 adds a RESTRICTIVE `portal_deny` policy to
  every RLS table (deny whenever the portal scope is set) and opens only `suppliers` (own row),
  `evidence_requests` (own approved) and `portal_uploads` (own), read-only. Writes (upload rows, audit) are done
  by server code in a tenant transaction keyed by the authenticated principal.
- Uploads: `artifacts::ingest` is the one pipeline for internal and portal uploads (static scan, antivirus fail
  closed, content-addressed store, capture provenance, audit); portal uploads are `origin = supplier_portal` on the
  artifact and the capture, kind `inspection_report`. Limits: per-supplier `max_upload_mb` (≤ tenant ≤ server) and
  `max_uploads_per_day`, plus the per-minute upload rate per supplier. Extraction and review stay with tenant
  reviewers (four-eyes unchanged): a supplier upload approves nothing.
- Every portal action is audited (`actor_kind = supplier_user`); every out-of-scope or cross-principal refusal raises `portal.denied` (rate-bounded); failed sign-ins raise `auth.login_failed`.
- Requests link to the supplier record once (invitation links earlier requests by name; drafting links new ones);
  the link and due date are guarded as final in `kz_evidence_requests_guard`.

## Consequences / limits
- A supplier that changes its legal name needs a new supplier record (no merge yet).
- Upload status shown to the supplier is the scan outcome; a later quarantine release is not reflected.
- `tenants` has no RLS (platform table; ids/slugs only) and stays readable by the runtime role in any scope.
- Tests: `crates/kizuna-api/tests/supplier_portal.rs` enumerates every router path × method with a supplier token,
  counts every RLS table under the portal scope, and covers cross-tenant, MFA, revocation and limits.

# ADR 0007: Deny-by-default RBAC with ABAC hooks (policy-engine)

Status: accepted (2026-11-05)

## Decision
- `policy-engine` is pure Rust. `authorize(subject, permission, resource) -> Decision` returns `Allow` only if:
  the role grants the permission (explicit match table, no wildcard), the resource tenant equals the subject tenant,
  privileged permissions are used from an MFA-verified session, and every registered `AttributeRule` allows.
- Roles: `tenant_admin`, `reviewer`, `analyst`, `auditor`, `viewer`. Unknown role strings parse to an error (deny).
- Every handler calls `authorize` before touching data; denials return 403 and write `authz.denied` security events.
- Safety: the last active tenant admin cannot be demoted or disabled; `kizuna-admin` CLI provides emergency recovery.

-- Tenant-scoped label synonyms (mfg-map@5). When the mapper reads a label it does not know (e.g. 製品番号) by a
-- fallback guess, the claim is marked needs_confirmation and carries {label, mapped_as}. A reviewer who accepts
-- that claim teaches the tenant the synonym label → mapped_as, which the next extraction for the SAME tenant uses
-- (basis tenant_synonym). Never shared across tenants (RLS). Reversible: a synonym is revoked, never deleted, and
-- both learning and revoking are audited.

CREATE TABLE label_synonyms (
    id             uuid PRIMARY KEY,
    tenant_id      uuid NOT NULL REFERENCES tenants(id),
    label          text NOT NULL CHECK (char_length(label) BETWEEN 1 AND 40),
    target         text NOT NULL CHECK (char_length(target) BETWEEN 1 AND 40),
    source_claim   uuid NOT NULL,
    created_by     uuid NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    revoked_at     timestamptz,
    revoked_by     uuid,
    revoke_reason  text CHECK (revoke_reason IS NULL OR char_length(revoke_reason) <= 500),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, source_claim) REFERENCES claims(tenant_id, id),
    CHECK ((revoked_at IS NULL) = (revoked_by IS NULL))
);
-- One active mapping per printed label and tenant.
CREATE UNIQUE INDEX label_synonyms_active ON label_synonyms (tenant_id, label) WHERE revoked_at IS NULL;

ALTER TABLE label_synonyms ENABLE ROW LEVEL SECURITY;
ALTER TABLE label_synonyms FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON label_synonyms USING (tenant_id = kz_tenant_id())
    WITH CHECK (tenant_id = kz_tenant_id());

GRANT SELECT, INSERT ON label_synonyms TO kizuna_app;
GRANT UPDATE (revoked_at, revoked_by, revoke_reason) ON label_synonyms TO kizuna_app;

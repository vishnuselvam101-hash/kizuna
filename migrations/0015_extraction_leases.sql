-- One extraction per artifact at a time without pinning a pooled connection for the whole run: a short lease row
-- (expires on its own if the request dies) replaces the session-level advisory lock.
CREATE TABLE extraction_leases (
    tenant_id   uuid NOT NULL REFERENCES tenants(id),
    artifact_id uuid NOT NULL,
    holder      uuid NOT NULL,
    expires_at  timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, artifact_id)
);
ALTER TABLE extraction_leases ENABLE ROW LEVEL SECURITY;
ALTER TABLE extraction_leases FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON extraction_leases USING (tenant_id = kz_tenant_id()) WITH CHECK (tenant_id = kz_tenant_id());
GRANT SELECT, INSERT, UPDATE, DELETE ON extraction_leases TO kizuna_app;

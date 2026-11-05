-- Versioned per-tenant import settings (web/API_GAPS.md #1). Append-only: every change is a new version.
CREATE TABLE import_settings (
    tenant_id  uuid NOT NULL REFERENCES tenants(id),
    version    int NOT NULL CHECK (version > 0),
    settings   jsonb NOT NULL,
    updated_by uuid NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, version)
);
CREATE TRIGGER import_settings_append_only BEFORE UPDATE OR DELETE ON import_settings
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();
ALTER TABLE import_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE import_settings FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON import_settings USING (tenant_id = kz_tenant_id()) WITH CHECK (tenant_id = kz_tenant_id());
GRANT SELECT, INSERT ON import_settings TO kizuna_app;

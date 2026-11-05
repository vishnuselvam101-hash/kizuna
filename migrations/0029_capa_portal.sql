-- 8D responses through the supplier portal: an approved CAPA supplier request can be published to one portal supplier
-- as an (approved) evidence request; documents that supplier uploads against it are linked to the case as supplier
-- responses (evidence only, never an approval or a completed step).
CREATE TABLE capa_portal_links (
    tenant_id           uuid NOT NULL REFERENCES tenants(id),
    capa_request_id     uuid NOT NULL,
    evidence_request_id uuid NOT NULL,
    supplier_id         uuid NOT NULL,
    created_by          uuid NOT NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, capa_request_id),
    UNIQUE (tenant_id, evidence_request_id),
    FOREIGN KEY (tenant_id, capa_request_id) REFERENCES capa_supplier_requests (tenant_id, id),
    FOREIGN KEY (tenant_id, evidence_request_id) REFERENCES evidence_requests (tenant_id, id),
    FOREIGN KEY (tenant_id, supplier_id) REFERENCES suppliers (tenant_id, id)
);
CREATE TRIGGER capa_portal_links_append_only BEFORE UPDATE OR DELETE ON capa_portal_links
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();
ALTER TABLE capa_portal_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE capa_portal_links FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON capa_portal_links USING (tenant_id = kz_tenant_id()) WITH CHECK (tenant_id = kz_tenant_id());
GRANT SELECT, INSERT ON capa_portal_links TO kizuna_app;

-- Deny-by-default for the portal scope also covers every RLS table created after the portal migration (CAPA, links,
-- and any later table): a table without a portal policy is invisible and unwritable to a supplier session.
DO $$
DECLARE t text;
BEGIN
    FOR t IN SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE n.nspname = 'public' AND c.relkind = 'r' AND c.relrowsecurity
               AND NOT EXISTS (SELECT 1 FROM pg_policy p WHERE p.polrelid = c.oid
                               AND p.polname IN ('portal_deny', 'portal_scope')) LOOP
        EXECUTE format('CREATE POLICY portal_deny ON %I AS RESTRICTIVE USING (kz_portal_supplier_id() IS NULL) '
                       'WITH CHECK (kz_portal_supplier_id() IS NULL)', t);
    END LOOP;
END $$;

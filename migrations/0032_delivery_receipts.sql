-- Innovation I7: sealed delivery receipts. One row per supplier portal delivery that became a stored artifact, written
-- in the same transaction as the capture and its `delivery_receipt.issued` audit event. The receipt itself is built
-- on download from that audit event (plus a signed checkpoint once one covers it); this row only links the upload to
-- the event, so the supplier can find its receipt under the portal scope. Append-only.
CREATE TABLE delivery_receipts (
    id               uuid PRIMARY KEY,
    tenant_id        uuid NOT NULL REFERENCES tenants(id),
    supplier_id      uuid NOT NULL,
    portal_upload_id uuid NOT NULL REFERENCES portal_uploads (id),
    artifact_id      uuid NOT NULL,
    event_seq        bigint NOT NULL CHECK (event_seq > 0),
    created_at       timestamptz NOT NULL DEFAULT now(),
    UNIQUE (portal_upload_id),
    UNIQUE (tenant_id, event_seq),
    FOREIGN KEY (tenant_id, supplier_id) REFERENCES suppliers (tenant_id, id),
    FOREIGN KEY (tenant_id, artifact_id) REFERENCES artifacts (tenant_id, id)
);
CREATE TRIGGER delivery_receipts_append_only BEFORE UPDATE OR DELETE ON delivery_receipts
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();
ALTER TABLE delivery_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE delivery_receipts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON delivery_receipts USING (tenant_id = kz_tenant_id()) WITH CHECK (tenant_id = kz_tenant_id());
-- Portal scope: a supplier session reads its own supplier's receipts and writes nothing.
CREATE POLICY portal_scope ON delivery_receipts AS RESTRICTIVE
    USING (kz_portal_supplier_id() IS NULL OR (tenant_id = kz_portal_tenant_id() AND supplier_id = kz_portal_supplier_id()))
    WITH CHECK (kz_portal_supplier_id() IS NULL);
CREATE POLICY portal_read ON delivery_receipts FOR SELECT
    USING (tenant_id = kz_portal_tenant_id() AND supplier_id = kz_portal_supplier_id());
GRANT SELECT, INSERT ON delivery_receipts TO kizuna_app;

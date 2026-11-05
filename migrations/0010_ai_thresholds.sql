-- M7: AI-assisted extraction (opt-in per tenant), per-tenant detection thresholds.

-- AI suggestions are a separate review state: never 'accepted' until a human reviews them.
ALTER TABLE claims DROP CONSTRAINT claims_review_status_check;
ALTER TABLE claims ADD CONSTRAINT claims_review_status_check
    CHECK (review_status IN ('unreviewed', 'accepted', 'rejected', 'needs_evidence', 'ai_suggested'));
-- A claim can only become ai_suggested when it is created, never by a later update.
CREATE FUNCTION kz_ai_status_insert_only() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.review_status = 'ai_suggested' AND OLD.review_status <> 'ai_suggested' THEN
        RAISE EXCEPTION 'ai_suggested is set at creation only';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER claims_ai_status_insert_only BEFORE UPDATE OF review_status ON claims
    FOR EACH ROW EXECUTE FUNCTION kz_ai_status_insert_only();

-- Versioned per-tenant settings by kind ('ai_extraction', 'detection'). Append-only, like import_settings.
CREATE TABLE tenant_settings (
    tenant_id  uuid NOT NULL REFERENCES tenants(id),
    kind       text NOT NULL CHECK (kind IN ('ai_extraction', 'detection')),
    version    int NOT NULL CHECK (version > 0),
    settings   jsonb NOT NULL,
    updated_by uuid NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, kind, version)
);
CREATE TRIGGER tenant_settings_append_only BEFORE UPDATE OR DELETE ON tenant_settings
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();
ALTER TABLE tenant_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE tenant_settings FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON tenant_settings USING (tenant_id = kz_tenant_id()) WITH CHECK (tenant_id = kz_tenant_id());
GRANT SELECT, INSERT ON tenant_settings TO kizuna_app;

-- One row per AI call, written before the call (budget accounting). No document text is stored; outcomes are
-- in the audit log (extraction.ai_call).
CREATE TABLE ai_calls (
    id            uuid PRIMARY KEY,
    tenant_id     uuid NOT NULL REFERENCES tenants(id),
    artifact_id   uuid NOT NULL,
    model         text NOT NULL,
    prompt_sha256 bytea NOT NULL,
    prompt_chars  int NOT NULL,
    status        text NOT NULL CHECK (status = 'sent'),
    occurred_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX ai_calls_tenant_time ON ai_calls (tenant_id, occurred_at);
CREATE TRIGGER ai_calls_append_only BEFORE UPDATE OR DELETE ON ai_calls
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();
ALTER TABLE ai_calls ENABLE ROW LEVEL SECURITY;
ALTER TABLE ai_calls FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON ai_calls USING (tenant_id = kz_tenant_id()) WITH CHECK (tenant_id = kz_tenant_id());
GRANT SELECT, INSERT ON ai_calls TO kizuna_app;

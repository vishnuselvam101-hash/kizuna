-- M5 defensive security: incidents, response actions, containment controls, signed audit checkpoints.

CREATE TABLE incidents (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL REFERENCES tenants(id),
    rule        text NOT NULL,
    severity    text NOT NULL CHECK (severity IN ('low', 'medium', 'high', 'critical')),
    subject     text NOT NULL,
    summary     text NOT NULL,
    status      text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'contained', 'resolved', 'false_positive')),
    event_ids   jsonb NOT NULL DEFAULT '[]',
    first_seen  timestamptz NOT NULL DEFAULT now(),
    last_seen   timestamptz NOT NULL DEFAULT now(),
    resolution  text,
    UNIQUE (tenant_id, id)
);
CREATE UNIQUE INDEX incidents_one_open ON incidents (tenant_id, rule, subject) WHERE status IN ('open', 'contained');

CREATE TABLE response_actions (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL,
    incident_id uuid NOT NULL,
    kind        text NOT NULL CHECK (kind IN ('revoke_user_sessions', 'lock_account', 'block_ip', 'quarantine_artifact',
                                              'restrict_uploads', 'suspend_extraction', 'notify_admins')),
    target      text NOT NULL,
    policy      text NOT NULL CHECK (policy IN ('pre_authorised', 'requires_approval')),
    status      text NOT NULL CHECK (status IN ('proposed', 'executed', 'rejected', 'reverted', 'refused')),
    detail      text,
    decided_by  uuid,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (tenant_id, incident_id) REFERENCES incidents(tenant_id, id)
);

CREATE TABLE ip_blocks (
    tenant_id   uuid NOT NULL REFERENCES tenants(id),
    ip          text NOT NULL,
    until       timestamptz NOT NULL,
    action_id   uuid,
    PRIMARY KEY (tenant_id, ip)
);

CREATE TABLE tenant_controls (
    tenant_id                  uuid PRIMARY KEY REFERENCES tenants(id),
    extraction_suspended_until timestamptz
);

ALTER TABLE users ADD COLUMN upload_blocked_until timestamptz;

CREATE TABLE audit_checkpoints (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenants(id),
    seq        bigint NOT NULL,
    head_hash  bytea NOT NULL CHECK (length(head_hash) = 32),
    signed_at  timestamptz NOT NULL,
    key_id     text NOT NULL,
    signature  bytea NOT NULL CHECK (length(signature) = 64),
    created_by uuid
);
CREATE TRIGGER audit_checkpoints_append_only BEFORE UPDATE OR DELETE ON audit_checkpoints
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['incidents', 'response_actions', 'ip_blocks', 'tenant_controls', 'audit_checkpoints'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY tenant_isolation ON %I USING (tenant_id = kz_tenant_id()) '
                       'WITH CHECK (tenant_id = kz_tenant_id())', t);
    END LOOP;
END $$;

GRANT SELECT, INSERT ON incidents, response_actions, audit_checkpoints TO kizuna_app;
GRANT UPDATE (status, event_ids, last_seen, resolution, severity, summary) ON incidents TO kizuna_app;
GRANT UPDATE (status, detail, decided_by, updated_at, policy) ON response_actions TO kizuna_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ip_blocks TO kizuna_app;
GRANT SELECT, INSERT, UPDATE ON tenant_controls TO kizuna_app;

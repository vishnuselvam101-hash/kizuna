-- M1 secure foundation: tenants, users, sessions, refresh tokens, audit chain, security events.
-- Owner: kizuna (migration role). Runtime: kizuna_app (least privilege, subject to RLS).

CREATE FUNCTION kz_tenant_id() RETURNS uuid LANGUAGE sql STABLE AS
$$ SELECT nullif(current_setting('kizuna.tenant_id', true), '')::uuid $$;

-- Append-only guard, used by audit/security tables.
CREATE FUNCTION kz_forbid_change() RETURNS trigger LANGUAGE plpgsql AS
$$ BEGIN RAISE EXCEPTION 'table % is append-only', TG_TABLE_NAME USING ERRCODE = 'insufficient_privilege'; END $$;

CREATE TABLE tenants (
    id         uuid PRIMARY KEY,
    slug       text NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9][a-z0-9-]{1,62}$'),
    name       text NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    status     text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended')),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE users (
    id                 uuid PRIMARY KEY,
    tenant_id          uuid NOT NULL REFERENCES tenants(id),
    email              text NOT NULL CHECK (email = lower(email) AND length(email) BETWEEN 3 AND 254),
    display_name       text NOT NULL CHECK (length(display_name) BETWEEN 1 AND 200),
    password_hash      text NOT NULL,
    role               text NOT NULL CHECK (role IN ('tenant_admin', 'reviewer', 'analyst', 'auditor', 'viewer')),
    status             text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    failed_logins      int NOT NULL DEFAULT 0,
    locked_until       timestamptz,
    totp_secret_enc    bytea,
    totp_pending_enc   bytea,
    totp_enabled       boolean NOT NULL DEFAULT false,
    totp_last_step     bigint,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, email),
    UNIQUE (tenant_id, id)
);

CREATE TABLE sessions (
    id                uuid PRIMARY KEY,
    tenant_id         uuid NOT NULL,
    user_id           uuid NOT NULL,
    access_hash       bytea NOT NULL UNIQUE,
    access_expires_at timestamptz NOT NULL,
    expires_at        timestamptz NOT NULL,
    mfa_verified      boolean NOT NULL,
    client_ip         text,
    created_at        timestamptz NOT NULL DEFAULT now(),
    revoked_at        timestamptz,
    revoked_reason    text,
    FOREIGN KEY (tenant_id, user_id) REFERENCES users(tenant_id, id),
    UNIQUE (tenant_id, id)
);
CREATE INDEX sessions_user ON sessions (tenant_id, user_id) WHERE revoked_at IS NULL;

CREATE TABLE refresh_tokens (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL,
    session_id  uuid NOT NULL,
    token_hash  bytea NOT NULL UNIQUE,
    expires_at  timestamptz NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    used_at     timestamptz,
    FOREIGN KEY (tenant_id, session_id) REFERENCES sessions(tenant_id, id)
);

CREATE TABLE audit_heads (
    tenant_id uuid PRIMARY KEY REFERENCES tenants(id),
    last_seq  bigint NOT NULL,
    last_hash bytea NOT NULL
);

CREATE TABLE audit_events (
    tenant_id     uuid NOT NULL REFERENCES tenants(id),
    seq           bigint NOT NULL CHECK (seq > 0),
    occurred_at   timestamptz NOT NULL,
    actor_user_id uuid,
    actor_kind    text NOT NULL,
    action        text NOT NULL,
    target_type   text NOT NULL,
    target_id     text,
    details       jsonb NOT NULL,
    prev_hash     bytea NOT NULL CHECK (length(prev_hash) = 32),
    hash          bytea NOT NULL CHECK (length(hash) = 32),
    PRIMARY KEY (tenant_id, seq)
);
CREATE TRIGGER audit_events_append_only BEFORE UPDATE OR DELETE ON audit_events
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();
CREATE TRIGGER audit_events_no_truncate BEFORE TRUNCATE ON audit_events
    FOR EACH STATEMENT EXECUTE FUNCTION kz_forbid_change();

CREATE TABLE security_events (
    id          uuid PRIMARY KEY,
    tenant_id   uuid REFERENCES tenants(id),
    occurred_at timestamptz NOT NULL DEFAULT now(),
    kind        text NOT NULL,
    severity    text NOT NULL CHECK (severity IN ('info', 'low', 'medium', 'high', 'critical')),
    user_id     uuid,
    client_ip   text,
    details     jsonb NOT NULL DEFAULT '{}'
);
CREATE INDEX security_events_tenant_time ON security_events (tenant_id, occurred_at DESC);
CREATE TRIGGER security_events_append_only BEFORE UPDATE OR DELETE ON security_events
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();
CREATE TRIGGER security_events_no_truncate BEFORE TRUNCATE ON security_events
    FOR EACH STATEMENT EXECUTE FUNCTION kz_forbid_change();

-- Row-level security: FORCE so the owner is subject to it too.
DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['users', 'sessions', 'refresh_tokens', 'audit_heads', 'audit_events'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY tenant_isolation ON %I USING (tenant_id = kz_tenant_id()) '
                       'WITH CHECK (tenant_id = kz_tenant_id())', t);
    END LOOP;
END $$;
ALTER TABLE security_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE security_events FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_read ON security_events FOR SELECT USING (tenant_id = kz_tenant_id());
-- Events without a tenant (e.g. login to an unknown tenant) may be written by the runtime; only the owner reads them.
CREATE POLICY platform_read ON security_events FOR SELECT TO kizuna USING (true);
CREATE POLICY tenant_write ON security_events FOR INSERT
    WITH CHECK (tenant_id IS NULL OR tenant_id = kz_tenant_id());

-- Runtime grants (least privilege).
GRANT USAGE ON SCHEMA public TO kizuna_app;
GRANT EXECUTE ON FUNCTION kz_tenant_id() TO kizuna_app;
GRANT SELECT ON tenants TO kizuna_app;
GRANT SELECT, INSERT, UPDATE ON users TO kizuna_app;
GRANT SELECT, INSERT, UPDATE ON sessions TO kizuna_app;
GRANT SELECT, INSERT, UPDATE ON refresh_tokens TO kizuna_app;
GRANT SELECT, INSERT, UPDATE ON audit_heads TO kizuna_app;
GRANT SELECT, INSERT ON audit_events TO kizuna_app;
GRANT SELECT, INSERT ON security_events TO kizuna_app;

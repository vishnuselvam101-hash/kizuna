-- Auth hardening: WebAuthn credentials and challenges, MFA recovery codes (ADR 0006).
-- Secrets (passkey material, ceremony state) are sealed with the versioned data key (keyring.rs); never plaintext.

ALTER TABLE users ADD COLUMN webauthn_enabled boolean NOT NULL DEFAULT false;

CREATE TABLE webauthn_credentials (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL,
    user_id      uuid NOT NULL,
    cred_id      bytea NOT NULL CHECK (length(cred_id) BETWEEN 16 AND 1023),
    passkey_enc  bytea NOT NULL,
    label        text NOT NULL CHECK (length(label) BETWEEN 1 AND 100),
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz,
    FOREIGN KEY (tenant_id, user_id) REFERENCES users(tenant_id, id),
    UNIQUE (tenant_id, cred_id)
);
CREATE INDEX webauthn_credentials_user ON webauthn_credentials (tenant_id, user_id);

-- Single-use ceremony state; consumed by DELETE ... RETURNING.
CREATE TABLE webauthn_challenges (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL,
    user_id    uuid NOT NULL,
    purpose    text NOT NULL CHECK (purpose IN ('register', 'login')),
    state_enc  bytea NOT NULL,
    expires_at timestamptz NOT NULL,
    FOREIGN KEY (tenant_id, user_id) REFERENCES users(tenant_id, id)
);

-- One-time recovery codes (128-bit random; SHA-256 stored, sufficient for high-entropy secrets).
CREATE TABLE mfa_recovery_codes (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL,
    user_id    uuid NOT NULL,
    code_hash  bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    used_at    timestamptz,
    FOREIGN KEY (tenant_id, user_id) REFERENCES users(tenant_id, id),
    UNIQUE (tenant_id, user_id, code_hash)
);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['webauthn_credentials', 'webauthn_challenges', 'mfa_recovery_codes'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY tenant_isolation ON %I USING (tenant_id = kz_tenant_id()) '
                       'WITH CHECK (tenant_id = kz_tenant_id())', t);
    END LOOP;
END $$;

GRANT SELECT, INSERT, UPDATE, DELETE ON webauthn_credentials, webauthn_challenges, mfa_recovery_codes TO kizuna_app;

-- Supplier portal (ADR 0023): a supplier answers an approved minimal re-request by uploading the requested documents.
-- A supplier user is a separate, least-privilege principal scoped to ONE supplier record in ONE tenant. Its database
-- scope is two transaction-local settings (`kizuna.portal_tenant_id`, `kizuna.portal_supplier_id`) and never
-- `kizuna.tenant_id`, so every existing tenant policy yields nothing; a RESTRICTIVE policy on every RLS table then
-- denies by default whenever the portal scope is set, and only the rows below are opened, read-only.

CREATE FUNCTION kz_portal_tenant_id() RETURNS uuid LANGUAGE sql STABLE AS
$$ SELECT nullif(current_setting('kizuna.portal_tenant_id', true), '')::uuid $$;
CREATE FUNCTION kz_portal_supplier_id() RETURNS uuid LANGUAGE sql STABLE AS
$$ SELECT nullif(current_setting('kizuna.portal_supplier_id', true), '')::uuid $$;
GRANT EXECUTE ON FUNCTION kz_portal_tenant_id(), kz_portal_supplier_id() TO kizuna_app;

-- One supplier of one tenant, identified by its normalised name (the same normaliser that matches request letters).
CREATE TABLE suppliers (
    id                  uuid PRIMARY KEY,
    tenant_id           uuid NOT NULL REFERENCES tenants(id),
    name                text NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    name_norm           text NOT NULL CHECK (length(name_norm) BETWEEN 1 AND 200),
    -- Per-supplier upload bounds (the server and tenant ceilings still apply).
    max_upload_mb       int NOT NULL DEFAULT 25 CHECK (max_upload_mb BETWEEN 1 AND 100),
    max_uploads_per_day int NOT NULL DEFAULT 50 CHECK (max_uploads_per_day BETWEEN 1 AND 1000),
    created_by          uuid NOT NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, name_norm),
    UNIQUE (tenant_id, id)
);

CREATE TABLE supplier_users (
    id                uuid PRIMARY KEY,
    tenant_id         uuid NOT NULL REFERENCES tenants(id),
    supplier_id       uuid NOT NULL,
    email             text NOT NULL CHECK (length(email) BETWEEN 3 AND 254),
    display_name      text NOT NULL CHECK (length(display_name) BETWEEN 1 AND 200),
    status            text NOT NULL DEFAULT 'invited' CHECK (status IN ('invited', 'active', 'revoked')),
    -- Single-use invitation: only the SHA-256 of the link secret is stored; cleared when used or revoked.
    invite_hash       bytea UNIQUE CHECK (length(invite_hash) = 32),
    invite_expires_at timestamptz NOT NULL,
    invited_by        uuid NOT NULL,
    invited_at        timestamptz NOT NULL DEFAULT now(),
    password_hash     text,
    totp_pending_enc  bytea,
    totp_secret_enc   bytea,
    totp_last_step    bigint,
    failed_logins     int NOT NULL DEFAULT 0,
    locked_until      timestamptz,
    activated_at      timestamptz,
    revoked_at        timestamptz,
    revoked_by        uuid,
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, supplier_id) REFERENCES suppliers (tenant_id, id),
    -- MFA is not optional: an active supplier user always has a confirmed authenticator and no open invitation.
    CHECK (status <> 'active' OR (password_hash IS NOT NULL AND totp_secret_enc IS NOT NULL AND invite_hash IS NULL)),
    CHECK ((status = 'revoked') = (revoked_at IS NOT NULL)),
    CHECK (status <> 'revoked' OR invite_hash IS NULL)
);
-- One live account per email and tenant; a revoked account frees the address for a new invitation.
CREATE UNIQUE INDEX supplier_users_email ON supplier_users (tenant_id, email) WHERE status <> 'revoked';
CREATE INDEX supplier_users_supplier ON supplier_users (tenant_id, supplier_id);

CREATE TABLE supplier_sessions (
    id               uuid PRIMARY KEY,
    tenant_id        uuid NOT NULL REFERENCES tenants(id),
    supplier_user_id uuid NOT NULL,
    access_hash      bytea NOT NULL UNIQUE CHECK (length(access_hash) = 32),
    created_at       timestamptz NOT NULL DEFAULT now(),
    expires_at       timestamptz NOT NULL,
    last_activity_at timestamptz NOT NULL,
    revoked_at       timestamptz,
    revoke_reason    text,
    client_ip        text,
    FOREIGN KEY (tenant_id, supplier_user_id) REFERENCES supplier_users (tenant_id, id)
);
CREATE INDEX supplier_sessions_user ON supplier_sessions (tenant_id, supplier_user_id) WHERE revoked_at IS NULL;

-- Requests are addressed to a supplier record only by an explicit, audited admin action (never by name matching);
-- the link is set once and the due date is fixed when the request is approved.
ALTER TABLE evidence_requests ADD COLUMN supplier_id uuid, ADD COLUMN due_on date,
    ADD FOREIGN KEY (tenant_id, supplier_id) REFERENCES suppliers (tenant_id, id);
CREATE INDEX evidence_requests_supplier ON evidence_requests (tenant_id, supplier_id, status);

CREATE OR REPLACE FUNCTION kz_evidence_requests_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'evidence requests are append-only' USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF (to_jsonb(NEW) - ARRAY['status', 'reviewed_by', 'reviewed_at', 'review_comment', 'supplier_id', 'due_on'])
        IS DISTINCT FROM (to_jsonb(OLD) - ARRAY['status', 'reviewed_by', 'reviewed_at', 'review_comment', 'supplier_id', 'due_on']) THEN
        RAISE EXCEPTION 'evidence request content is immutable' USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF OLD.supplier_id IS NOT NULL AND NEW.supplier_id IS DISTINCT FROM OLD.supplier_id THEN
        RAISE EXCEPTION 'evidence request supplier link is final' USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF OLD.status <> 'draft' AND (NEW.status, NEW.reviewed_by, NEW.reviewed_at, NEW.review_comment, NEW.due_on)
        IS DISTINCT FROM (OLD.status, OLD.reviewed_by, OLD.reviewed_at, OLD.review_comment, OLD.due_on) THEN
        RAISE EXCEPTION 'evidence request status % is final', OLD.status USING ERRCODE = 'insufficient_privilege';
    END IF;
    RETURN NEW;
END $$;
GRANT UPDATE (supplier_id, due_on) ON evidence_requests TO kizuna_app;

-- Provenance of every captured document: uploaded inside the tenant, or by a supplier through the portal.
ALTER TABLE artifacts ADD COLUMN origin text NOT NULL DEFAULT 'internal' CHECK (origin IN ('internal', 'supplier_portal'));
ALTER TABLE artifact_captures ADD COLUMN origin text NOT NULL DEFAULT 'internal'
    CHECK (origin IN ('internal', 'supplier_portal'));

-- What a supplier sent against a request, and how far it got. A supplier upload approves nothing: the artifact goes
-- through the normal scan/quarantine/extraction pipeline and tenant reviewers decide on everything extracted from it.
CREATE TABLE portal_uploads (
    id                uuid PRIMARY KEY,
    tenant_id         uuid NOT NULL REFERENCES tenants(id),
    supplier_id       uuid NOT NULL,
    supplier_user_id  uuid NOT NULL,
    request_id        uuid NOT NULL,
    artifact_id       uuid,
    original_filename text NOT NULL CHECK (length(original_filename) BETWEEN 1 AND 255),
    size_bytes        bigint NOT NULL CHECK (size_bytes >= 0),
    -- What the supplier is shown; follows the artifact's real state (trigger below), never internal findings.
    status            text NOT NULL DEFAULT 'received'
                      CHECK (status IN ('received', 'scanning', 'quarantined', 'received_for_review', 'not_accepted')),
    -- Neutral reason for `not_accepted` only (no scanner findings or reviewer text reach the supplier).
    reason_code       text CHECK (reason_code IN ('file_type_not_accepted', 'not_accepted_after_review', 'upload_failed')),
    CHECK ((status = 'not_accepted') = (reason_code IS NOT NULL)),
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (tenant_id, supplier_id) REFERENCES suppliers (tenant_id, id),
    FOREIGN KEY (tenant_id, supplier_user_id) REFERENCES supplier_users (tenant_id, id),
    FOREIGN KEY (tenant_id, request_id) REFERENCES evidence_requests (tenant_id, id),
    FOREIGN KEY (tenant_id, artifact_id) REFERENCES artifacts (tenant_id, id)
);
CREATE INDEX portal_uploads_request ON portal_uploads (tenant_id, request_id, created_at);
CREATE INDEX portal_uploads_supplier_day ON portal_uploads (tenant_id, supplier_id, created_at);

CREATE FUNCTION kz_portal_uploads_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'portal uploads are append-only' USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF (to_jsonb(NEW) - ARRAY['status', 'artifact_id', 'updated_at', 'reason_code']) IS DISTINCT FROM
       (to_jsonb(OLD) - ARRAY['status', 'artifact_id', 'updated_at', 'reason_code']) THEN
        RAISE EXCEPTION 'portal upload content is immutable' USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF OLD.artifact_id IS NOT NULL AND NEW.artifact_id IS DISTINCT FROM OLD.artifact_id THEN
        RAISE EXCEPTION 'portal upload artifact link is final' USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF OLD.status = 'not_accepted' THEN
        RAISE EXCEPTION 'portal upload status % is final', OLD.status USING ERRCODE = 'insufficient_privilege';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER portal_uploads_guard BEFORE UPDATE OR DELETE ON portal_uploads
    FOR EACH ROW EXECUTE FUNCTION kz_portal_uploads_guard();

-- The supplier-visible status follows the artifact: released from quarantine → received_for_review, quarantined
-- again (e.g. incident containment) → quarantined, rejected → not_accepted (neutral reason). Runs in the caller's
-- tenant transaction, so RLS applies as for any tenant write.
CREATE FUNCTION kz_portal_uploads_follow() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE portal_uploads SET
        status = CASE NEW.status WHEN 'accepted' THEN 'received_for_review' WHEN 'quarantined' THEN 'quarantined'
                                 ELSE 'not_accepted' END,
        reason_code = CASE WHEN NEW.status = 'rejected' THEN 'not_accepted_after_review' END,
        updated_at = now()
    WHERE tenant_id = NEW.tenant_id AND artifact_id = NEW.id AND status <> 'not_accepted';
    RETURN NULL;
END $$;
CREATE TRIGGER artifacts_portal_follow AFTER UPDATE OF status ON artifacts
    FOR EACH ROW WHEN (NEW.status IS DISTINCT FROM OLD.status AND NEW.status IN ('accepted', 'quarantined', 'rejected'))
    EXECUTE FUNCTION kz_portal_uploads_follow();

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['suppliers', 'supplier_users', 'supplier_sessions', 'portal_uploads'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY tenant_isolation ON %I USING (tenant_id = kz_tenant_id()) '
                       'WITH CHECK (tenant_id = kz_tenant_id())', t);
    END LOOP;
END $$;

-- Deny by default: with the portal scope set, every row of every RLS table is invisible and unwritable...
DO $$
DECLARE t text;
BEGIN
    FOR t IN SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE n.nspname = 'public' AND c.relkind = 'r' AND c.relrowsecurity
               AND c.relname NOT IN ('suppliers', 'evidence_requests', 'portal_uploads') LOOP
        EXECUTE format('CREATE POLICY portal_deny ON %I AS RESTRICTIVE USING (kz_portal_supplier_id() IS NULL) '
                       'WITH CHECK (kz_portal_supplier_id() IS NULL)', t);
    END LOOP;
END $$;

-- ...except these supplier-scoped reads (one supplier, one tenant; requests only once a person approved them).
CREATE POLICY portal_scope ON suppliers AS RESTRICTIVE
    USING (kz_portal_supplier_id() IS NULL OR (tenant_id = kz_portal_tenant_id() AND id = kz_portal_supplier_id()))
    WITH CHECK (kz_portal_supplier_id() IS NULL);
CREATE POLICY portal_read ON suppliers FOR SELECT
    USING (tenant_id = kz_portal_tenant_id() AND id = kz_portal_supplier_id());

CREATE POLICY portal_scope ON evidence_requests AS RESTRICTIVE
    USING (kz_portal_supplier_id() IS NULL OR (tenant_id = kz_portal_tenant_id()
           AND supplier_id = kz_portal_supplier_id() AND status = 'approved'))
    WITH CHECK (kz_portal_supplier_id() IS NULL);
CREATE POLICY portal_read ON evidence_requests FOR SELECT
    USING (tenant_id = kz_portal_tenant_id() AND supplier_id = kz_portal_supplier_id() AND status = 'approved');

CREATE POLICY portal_scope ON portal_uploads AS RESTRICTIVE
    USING (kz_portal_supplier_id() IS NULL OR (tenant_id = kz_portal_tenant_id() AND supplier_id = kz_portal_supplier_id()))
    WITH CHECK (kz_portal_supplier_id() IS NULL);
CREATE POLICY portal_read ON portal_uploads FOR SELECT
    USING (tenant_id = kz_portal_tenant_id() AND supplier_id = kz_portal_supplier_id());

GRANT SELECT, INSERT ON suppliers, portal_uploads TO kizuna_app;
GRANT UPDATE (status, artifact_id, updated_at, reason_code) ON portal_uploads TO kizuna_app;
GRANT SELECT, INSERT ON supplier_users, supplier_sessions TO kizuna_app;
GRANT UPDATE (status, invite_hash, password_hash, totp_pending_enc, totp_secret_enc, totp_last_step, failed_logins,
              locked_until, activated_at, revoked_at, revoked_by) ON supplier_users TO kizuna_app;
GRANT UPDATE (last_activity_at, revoked_at, revoke_reason) ON supplier_sessions TO kizuna_app;

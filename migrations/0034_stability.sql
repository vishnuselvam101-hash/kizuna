-- Stability (STABILITY_2026-11-06): idempotency keys, a retry-safe background job queue, alert delivery marks.

-- Idempotency-Key replay store. One row per (tenant, principal, key) for 24 h. The response body is sealed with the
-- data key (AES-GCM; AAD binds tenant, principal and key), so a replayed body never sits in the database in clear.
CREATE TABLE idempotency_keys (
    tenant_id        uuid NOT NULL REFERENCES tenants(id),
    principal_kind   text NOT NULL CHECK (principal_kind IN ('user', 'supplier')),
    principal_id     uuid NOT NULL,
    key              text NOT NULL CHECK (length(key) BETWEEN 1 AND 200),
    request_hash     bytea NOT NULL CHECK (length(request_hash) = 32),
    state            text NOT NULL CHECK (state IN ('in_progress', 'completed')),
    status_code      int CHECK (status_code BETWEEN 100 AND 599),
    response_headers jsonb,
    response_body    bytea,
    created_at       timestamptz NOT NULL DEFAULT now(),
    -- An in-progress row whose holder died (crash, cancelled request) can be taken over after this instant.
    locked_until     timestamptz NOT NULL,
    expires_at       timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, principal_kind, principal_id, key)
);
CREATE INDEX idempotency_keys_expiry ON idempotency_keys (tenant_id, expires_at);
ALTER TABLE idempotency_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE idempotency_keys FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON idempotency_keys USING (tenant_id = kz_tenant_id())
    WITH CHECK (tenant_id = kz_tenant_id());
GRANT SELECT, INSERT, UPDATE, DELETE ON idempotency_keys TO kizuna_app;

-- Background jobs: at-least-once, leased, poison jobs quarantined after max_attempts.
-- attempts counts claims (a claim whose worker died counts too, so a job that kills its worker is quarantined).
-- Payloads carry identifiers only.
CREATE TABLE jobs (
    id               bigserial PRIMARY KEY,
    tenant_id        uuid NOT NULL REFERENCES tenants(id),
    kind             text NOT NULL CHECK (kind ~ '^[a-z][a-z_.]{0,63}$'),
    dedupe_key       text NOT NULL DEFAULT '' CHECK (length(dedupe_key) <= 200),
    payload          jsonb NOT NULL DEFAULT '{}',
    state            text NOT NULL DEFAULT 'queued' CHECK (state IN ('queued', 'running', 'done', 'quarantined')),
    attempts         int NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    max_attempts     int NOT NULL DEFAULT 5 CHECK (max_attempts BETWEEN 1 AND 100),
    run_after        timestamptz NOT NULL DEFAULT now(),
    lease_owner      text,
    lease_expires_at timestamptz,
    last_error       text CHECK (length(last_error) <= 500),
    created_by       uuid,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    finished_at      timestamptz
);
-- At most one queued job per (tenant, kind, dedupe key): enqueue is idempotent. A running job does not block a new
-- queued one, so work that arrives while a handler is finishing is never dropped.
CREATE UNIQUE INDEX jobs_queued_dedupe ON jobs (tenant_id, kind, dedupe_key) WHERE state = 'queued';
CREATE INDEX jobs_ready ON jobs (tenant_id, run_after, id) WHERE state = 'queued';
CREATE INDEX jobs_leased ON jobs (tenant_id, lease_expires_at) WHERE state = 'running';
CREATE INDEX jobs_finished ON jobs (tenant_id, finished_at) WHERE state = 'done';
-- Not FORCEd: the owner-defined functions below see every tenant's queue (identifiers and counts only); the runtime
-- role stays confined to its tenant.
ALTER TABLE jobs ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON jobs USING (tenant_id = kz_tenant_id()) WITH CHECK (tenant_id = kz_tenant_id());
GRANT SELECT, INSERT, UPDATE, DELETE ON jobs TO kizuna_app;
GRANT USAGE ON SEQUENCE jobs_id_seq TO kizuna_app;

-- Tenants that have a claimable job (queued and due, or running with an expired lease).
CREATE FUNCTION kz_jobs_ready_tenants() RETURNS SETOF uuid LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public AS $$
    SELECT DISTINCT tenant_id FROM jobs
     WHERE (state = 'queued' AND run_after <= now()) OR (state = 'running' AND lease_expires_at < now())
$$;
-- Queue depth by state, all tenants (metrics).
CREATE FUNCTION kz_jobs_depth() RETURNS TABLE (state text, n bigint) LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path = pg_catalog, public AS $$
    SELECT state, count(*) FROM jobs WHERE state <> 'done' GROUP BY state
$$;
REVOKE ALL ON FUNCTION kz_jobs_ready_tenants() FROM PUBLIC;
REVOKE ALL ON FUNCTION kz_jobs_depth() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION kz_jobs_ready_tenants() TO kizuna_app;
GRANT EXECUTE ON FUNCTION kz_jobs_depth() TO kizuna_app;

-- Alert delivery marks: the alert job sends each executed notify_admins action once and records it here (a crash
-- between send and mark re-sends: at-least-once). Existing actions count as delivered (no replay after upgrade).
ALTER TABLE response_actions ADD COLUMN alerted_at timestamptz;
DO $$
DECLARE t uuid;
BEGIN
    FOR t IN SELECT id FROM tenants LOOP
        PERFORM set_config('kizuna.tenant_id', t::text, true);
        UPDATE response_actions SET alerted_at = now() WHERE kind = 'notify_admins' AND alerted_at IS NULL;
    END LOOP;
    PERFORM set_config('kizuna.tenant_id', '', true);
END $$;
GRANT UPDATE (alerted_at) ON response_actions TO kizuna_app;

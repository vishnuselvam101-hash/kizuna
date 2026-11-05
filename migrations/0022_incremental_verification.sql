-- Incremental verification runs (PERF_VERIFY_2026-11-05).
-- Every claim insert / supersession / review-status change and every alias change appends a row to
-- verification_changes in the same transaction. An incremental run consumes (deletes) the rows it sees, re-evaluates
-- only the affected subjects and records the highest consumed id as its watermark. Consuming by DELETE ... RETURNING
-- (not "id > watermark") keeps it exact under concurrency: a change committed after the run's snapshot is never
-- skipped, because a bigserial id may commit out of order.

CREATE TABLE verification_changes (
    id           bigserial PRIMARY KEY,
    tenant_id    uuid NOT NULL REFERENCES tenants(id),
    subject_type text NOT NULL,
    -- NULL: every subject of this type must be re-evaluated (an alias / entity decision changed canonical keys).
    subject_key  text,
    claim_id     uuid,
    changed_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX verification_changes_tenant ON verification_changes (tenant_id, subject_type, id);

CREATE TABLE verification_state (
    tenant_id        uuid PRIMARY KEY REFERENCES tenants(id),
    watermark        bigint NOT NULL DEFAULT 0 CHECK (watermark >= 0),
    -- Tenant calendar day of the last run: expiry findings depend on it, so a new day forces a full run.
    last_day         date,
    last_run_at      timestamptz,
    last_full_at     timestamptz,
    runs_since_full  int NOT NULL DEFAULT 0 CHECK (runs_since_full >= 0)
);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['verification_changes', 'verification_state'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY tenant_isolation ON %I USING (tenant_id = kz_tenant_id()) '
                       'WITH CHECK (tenant_id = kz_tenant_id())', t);
    END LOOP;
END $$;
GRANT SELECT, INSERT, DELETE ON verification_changes TO kizuna_app;
GRANT USAGE ON SEQUENCE verification_changes_id_seq TO kizuna_app;
GRANT SELECT, INSERT, UPDATE ON verification_state TO kizuna_app;

CREATE FUNCTION kz_claims_note_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO verification_changes (tenant_id, subject_type, subject_key, claim_id)
    VALUES (NEW.tenant_id, NEW.subject_type, NEW.subject_key, NEW.id);
    RETURN NULL;
END $$;
CREATE TRIGGER claims_note_change AFTER INSERT OR UPDATE OF superseded_at, review_status ON claims
    FOR EACH ROW EXECUTE FUNCTION kz_claims_note_change();

CREATE FUNCTION kz_aliases_note_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO verification_changes (tenant_id, subject_type, subject_key)
    VALUES (coalesce(NEW.tenant_id, OLD.tenant_id), coalesce(NEW.subject_type, OLD.subject_type), NULL);
    RETURN NULL;
END $$;
CREATE TRIGGER entity_aliases_note_change AFTER INSERT OR UPDATE OR DELETE ON entity_aliases
    FOR EACH ROW EXECUTE FUNCTION kz_aliases_note_change();

-- Stale-finding lookup by claim (the dedup index leads with kind, so claim-only probes could not use it).
CREATE INDEX conflicts_open_claim_a ON conflicts (tenant_id, claim_a) WHERE status IN ('open', 'needs_evidence');
CREATE INDEX conflicts_open_claim_b ON conflicts (tenant_id, claim_b) WHERE status IN ('open', 'needs_evidence')
    AND claim_b IS NOT NULL;

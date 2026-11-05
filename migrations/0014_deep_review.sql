-- Deep review 2026-11-05.
-- KZ-A1: inactivity timeout; KZ-A3: authentication freshness for privileged operations.
ALTER TABLE sessions ADD COLUMN last_activity_at timestamptz NOT NULL DEFAULT now();
ALTER TABLE sessions ADD COLUMN auth_at timestamptz NOT NULL DEFAULT now();

-- KZ-I1: the runtime role no longer updates audit_heads; the head advances in an owner-defined trigger that only
-- accepts the next link of the chain, so a compromised app role cannot rewind it.
CREATE FUNCTION kz_audit_advance_head() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, public AS $$
BEGIN
    UPDATE audit_heads SET last_seq = NEW.seq, last_hash = NEW.hash
     WHERE tenant_id = NEW.tenant_id AND last_seq = NEW.seq - 1 AND last_hash = NEW.prev_hash;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'audit event % does not extend the chain head', NEW.seq;
    END IF;
    RETURN NEW;
END $$;
REVOKE ALL ON FUNCTION kz_audit_advance_head() FROM PUBLIC;
CREATE TRIGGER audit_events_advance_head AFTER INSERT ON audit_events
    FOR EACH ROW EXECUTE FUNCTION kz_audit_advance_head();
REVOKE UPDATE ON audit_heads FROM kizuna_app;

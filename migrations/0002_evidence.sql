-- M2 evidence foundation: artifacts (content-addressed originals), capture provenance, integrity checks, claims.

CREATE TABLE artifacts (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL REFERENCES tenants(id),
    sha256       bytea NOT NULL CHECK (length(sha256) = 32),
    size_bytes   bigint NOT NULL CHECK (size_bytes > 0),
    media_type   text NOT NULL,
    kind         text NOT NULL CHECK (kind IN ('spec_document', 'drawing', 'purchase_order', 'inspection_report',
                                               'spreadsheet', 'other')),
    status       text NOT NULL CHECK (status IN ('pending_scan', 'accepted', 'quarantined', 'rejected')),
    scan_result  jsonb NOT NULL DEFAULT '{}',
    storage_key  text NOT NULL,
    display_name text NOT NULL CHECK (length(display_name) BETWEEN 1 AND 255),
    created_by   uuid NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, sha256),
    UNIQUE (tenant_id, id)
);

-- Every upload event, including re-uploads of identical bytes (append-only provenance of capture).
CREATE TABLE artifact_captures (
    id                uuid PRIMARY KEY,
    tenant_id         uuid NOT NULL,
    artifact_id       uuid NOT NULL,
    uploaded_by       uuid NOT NULL,
    uploaded_at       timestamptz NOT NULL DEFAULT now(),
    original_filename text NOT NULL,
    declared_type     text,
    client_ip         text,
    FOREIGN KEY (tenant_id, artifact_id) REFERENCES artifacts(tenant_id, id)
);

CREATE TABLE integrity_checks (
    id              uuid PRIMARY KEY,
    tenant_id       uuid NOT NULL,
    artifact_id     uuid NOT NULL,
    checked_at      timestamptz NOT NULL DEFAULT now(),
    ok              boolean NOT NULL,
    observed_sha256 bytea,
    detail          text,
    FOREIGN KEY (tenant_id, artifact_id) REFERENCES artifacts(tenant_id, id)
);

CREATE TABLE claims (
    id                 uuid PRIMARY KEY,
    tenant_id          uuid NOT NULL REFERENCES tenants(id),
    model_version      smallint NOT NULL DEFAULT 1,
    subject_type       text NOT NULL CHECK (subject_type ~ '^[a-z_]{2,40}$'),
    subject_key        text NOT NULL CHECK (length(subject_key) BETWEEN 1 AND 200),
    predicate          text NOT NULL CHECK (predicate ~ '^[a-z][a-z0-9_.]{1,80}$'),
    value_text         text CHECK (length(value_text) <= 2000),
    value_num          numeric,
    value_unit         text CHECK (length(value_unit) <= 32),
    scope              jsonb NOT NULL DEFAULT '{}',
    valid_from         timestamptz,
    valid_to           timestamptz,
    revision           text CHECK (length(revision) <= 40),
    artifact_id        uuid,
    source_location    jsonb NOT NULL DEFAULT '{}',
    extraction_method  text NOT NULL CHECK (length(extraction_method) <= 80),
    confidence_bps     int NOT NULL CHECK (confidence_bps BETWEEN 0 AND 10000),
    review_status      text NOT NULL DEFAULT 'unreviewed'
                       CHECK (review_status IN ('unreviewed', 'accepted', 'rejected', 'needs_evidence')),
    provenance         jsonb NOT NULL DEFAULT '{}',
    recorded_by        uuid,
    recorded_at        timestamptz NOT NULL DEFAULT now(),
    superseded_at      timestamptz,
    superseded_by      uuid,
    CHECK (value_text IS NOT NULL OR value_num IS NOT NULL),
    CHECK (valid_to IS NULL OR valid_from IS NULL OR valid_to > valid_from),
    CHECK ((superseded_at IS NULL) = (superseded_by IS NULL)),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, artifact_id) REFERENCES artifacts(tenant_id, id)
);
CREATE INDEX claims_subject ON claims (tenant_id, subject_type, subject_key, predicate);
CREATE INDEX claims_artifact ON claims (tenant_id, artifact_id);

-- Claims are never rewritten: only supersession (once) and review status may change.
CREATE FUNCTION kz_claims_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'claims are append-only' USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF (NEW.id, NEW.tenant_id, NEW.subject_type, NEW.subject_key, NEW.predicate, NEW.value_text, NEW.value_num,
        NEW.value_unit, NEW.scope, NEW.valid_from, NEW.valid_to, NEW.revision, NEW.artifact_id, NEW.source_location,
        NEW.extraction_method, NEW.confidence_bps, NEW.provenance, NEW.recorded_by, NEW.recorded_at, NEW.model_version)
       IS DISTINCT FROM
       (OLD.id, OLD.tenant_id, OLD.subject_type, OLD.subject_key, OLD.predicate, OLD.value_text, OLD.value_num,
        OLD.value_unit, OLD.scope, OLD.valid_from, OLD.valid_to, OLD.revision, OLD.artifact_id, OLD.source_location,
        OLD.extraction_method, OLD.confidence_bps, OLD.provenance, OLD.recorded_by, OLD.recorded_at, OLD.model_version)
    THEN
        RAISE EXCEPTION 'claim content is immutable' USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF OLD.superseded_at IS NOT NULL AND (NEW.superseded_at, NEW.superseded_by)
       IS DISTINCT FROM (OLD.superseded_at, OLD.superseded_by) THEN
        RAISE EXCEPTION 'supersession is final' USING ERRCODE = 'insufficient_privilege';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER claims_guard BEFORE UPDATE OR DELETE ON claims FOR EACH ROW EXECUTE FUNCTION kz_claims_guard();

CREATE TRIGGER artifact_captures_append_only BEFORE UPDATE OR DELETE ON artifact_captures
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();
CREATE TRIGGER integrity_checks_append_only BEFORE UPDATE OR DELETE ON integrity_checks
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['artifacts', 'artifact_captures', 'integrity_checks', 'claims'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY tenant_isolation ON %I USING (tenant_id = kz_tenant_id()) '
                       'WITH CHECK (tenant_id = kz_tenant_id())', t);
    END LOOP;
END $$;

GRANT SELECT, INSERT ON artifacts, artifact_captures, integrity_checks, claims TO kizuna_app;
GRANT UPDATE (status, scan_result) ON artifacts TO kizuna_app;
GRANT UPDATE (superseded_at, superseded_by, review_status) ON claims TO kizuna_app;

-- Certificate integrity signals (I3) and revision drift radar (I2). Findings are review items only: nothing here
-- rejects evidence. Signals and drift findings are kept forever (status changes only; no delete), reviews and
-- settings are append-only. Tenant isolation by FORCE RLS like every other tenant table.

-- Versioned per-tenant thresholds (bounded in integrity-signals::Settings::validate).
CREATE TABLE integrity_signal_settings (
    tenant_id  uuid NOT NULL REFERENCES tenants(id),
    version    int NOT NULL CHECK (version > 0),
    settings   jsonb NOT NULL,
    updated_by uuid NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, version)
);

CREATE TABLE integrity_signals (
    id               uuid PRIMARY KEY,
    tenant_id        uuid NOT NULL REFERENCES tenants(id),
    kind             text NOT NULL CHECK (kind IN ('identical_values', 'table_copy', 'narrow_band', 'at_limit',
                         'excess_precision', 'chemistry_sum', 'date_order', 'lot_sequence')),
    supplier_key     text NOT NULL CHECK (length(supplier_key) BETWEEN 1 AND 200),
    part_key         text CHECK (length(part_key) <= 200),
    characteristic   text CHECK (length(characteristic) <= 120),
    -- How pronounced the pattern is; never a probability of wrongdoing.
    strength         text NOT NULL CHECK (strength IN ('weak', 'moderate', 'strong')),
    explanation      jsonb NOT NULL,
    statistic        jsonb NOT NULL,
    evidence         jsonb NOT NULL,
    artifact_ids     uuid[] NOT NULL,
    fingerprint      text NOT NULL CHECK (fingerprint ~ '^[0-9a-f]{64}$'),
    engine_version   text NOT NULL,
    settings_version int NOT NULL CHECK (settings_version >= 0),
    -- explained = a reviewer recorded an ordinary cause; follow_up = a reviewer decided to ask the supplier.
    status           text NOT NULL DEFAULT 'open'
                     CHECK (status IN ('open', 'needs_evidence', 'explained', 'follow_up', 'obsolete')),
    detected_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, fingerprint)
);
CREATE INDEX integrity_signals_list ON integrity_signals (tenant_id, status, detected_at DESC);

CREATE TABLE revision_drift_findings (
    id               uuid PRIMARY KEY,
    tenant_id        uuid NOT NULL REFERENCES tenants(id),
    kind             text NOT NULL CHECK (kind IN ('silent_change', 'revision_without_change', 'revision_regression')),
    drawing_key      text NOT NULL CHECK (length(drawing_key) BETWEEN 1 AND 200),
    part_key         text NOT NULL CHECK (length(part_key) BETWEEN 1 AND 200),
    earlier_artifact uuid NOT NULL,
    later_artifact   uuid NOT NULL,
    earlier_revision text CHECK (length(earlier_revision) <= 40),
    later_revision   text CHECK (length(later_revision) <= 40),
    changes          jsonb NOT NULL,
    strength         text NOT NULL CHECK (strength IN ('weak', 'moderate', 'strong')),
    explanation      jsonb NOT NULL,
    fingerprint      text NOT NULL CHECK (fingerprint ~ '^[0-9a-f]{64}$'),
    engine_version   text NOT NULL,
    settings_version int NOT NULL CHECK (settings_version >= 0),
    -- accepted_change = the difference is legitimate / documented; confirmed = drift confirmed, dependent
    -- decisions were assessed by selective invalidation.
    status           text NOT NULL DEFAULT 'open'
                     CHECK (status IN ('open', 'needs_evidence', 'accepted_change', 'confirmed', 'obsolete')),
    detected_at      timestamptz NOT NULL DEFAULT now(),
    CHECK (earlier_artifact <> later_artifact),
    FOREIGN KEY (tenant_id, earlier_artifact) REFERENCES artifacts(tenant_id, id),
    FOREIGN KEY (tenant_id, later_artifact) REFERENCES artifacts(tenant_id, id),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, fingerprint)
);
CREATE INDEX revision_drift_list ON revision_drift_findings (tenant_id, status, detected_at DESC);

CREATE TABLE integrity_reviews (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL REFERENCES tenants(id),
    target_type text NOT NULL CHECK (target_type IN ('integrity_signal', 'revision_drift')),
    target_id   uuid NOT NULL,
    decision    text NOT NULL CHECK (decision IN ('explained', 'follow_up', 'request_evidence', 'accept_change',
                    'confirm_drift')),
    comment     text CHECK (length(comment) <= 2000),
    reviewer    uuid NOT NULL,
    decided_at  timestamptz NOT NULL DEFAULT now(),
    CHECK ((target_type = 'integrity_signal' AND decision IN ('explained', 'follow_up', 'request_evidence'))
        OR (target_type = 'revision_drift' AND decision IN ('accept_change', 'confirm_drift', 'request_evidence')))
);
CREATE INDEX integrity_reviews_target ON integrity_reviews (tenant_id, target_type, target_id);

CREATE TRIGGER integrity_signal_settings_append_only BEFORE UPDATE OR DELETE ON integrity_signal_settings
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();
CREATE TRIGGER integrity_reviews_append_only BEFORE UPDATE OR DELETE ON integrity_reviews
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();

-- Findings: only the status may change after insert (and, when a finding re-fires after being obsolete, its
-- recomputed explanation/statistic/evidence/strength). Identity and provenance are immutable; no deletes.
CREATE FUNCTION kz_integrity_finding_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'integrity findings are never deleted' USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF NEW.id <> OLD.id OR NEW.tenant_id <> OLD.tenant_id OR NEW.kind <> OLD.kind
       OR NEW.fingerprint <> OLD.fingerprint OR NEW.detected_at <> OLD.detected_at THEN
        RAISE EXCEPTION 'integrity finding identity is immutable' USING ERRCODE = 'insufficient_privilege';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER integrity_signals_guard BEFORE UPDATE OR DELETE ON integrity_signals
    FOR EACH ROW EXECUTE FUNCTION kz_integrity_finding_guard();
CREATE TRIGGER revision_drift_guard BEFORE UPDATE OR DELETE ON revision_drift_findings
    FOR EACH ROW EXECUTE FUNCTION kz_integrity_finding_guard();

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['integrity_signal_settings', 'integrity_signals', 'revision_drift_findings',
                             'integrity_reviews'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY tenant_isolation ON %I USING (tenant_id = kz_tenant_id()) '
                       'WITH CHECK (tenant_id = kz_tenant_id())', t);
    END LOOP;
END $$;

GRANT SELECT, INSERT ON integrity_signal_settings, integrity_signals, revision_drift_findings, integrity_reviews
    TO kizuna_app;
GRANT UPDATE (status, explanation, statistic, evidence, strength, artifact_ids, engine_version, settings_version)
    ON integrity_signals TO kizuna_app;
GRANT UPDATE (status, changes, explanation, strength, engine_version, settings_version)
    ON revision_drift_findings TO kizuna_app;

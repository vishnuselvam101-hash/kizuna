-- M3 manufacturing verification: extraction runs, entity resolution decisions, conflicts, reviews.

CREATE TABLE extraction_runs (
    id              uuid PRIMARY KEY,
    tenant_id       uuid NOT NULL,
    artifact_id     uuid NOT NULL,
    extractor       text NOT NULL,
    mapping_version text NOT NULL,
    status          text NOT NULL CHECK (status IN ('succeeded', 'failed')),
    error           text,
    records         int NOT NULL DEFAULT 0,
    claims_created  int NOT NULL DEFAULT 0,
    output_sha256   bytea,
    warnings        jsonb NOT NULL DEFAULT '[]',
    started_by      uuid NOT NULL,
    started_at      timestamptz NOT NULL,
    finished_at     timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (tenant_id, artifact_id) REFERENCES artifacts(tenant_id, id)
);
CREATE UNIQUE INDEX extraction_runs_once ON extraction_runs (tenant_id, artifact_id, mapping_version) WHERE status = 'succeeded';

-- Human entity-resolution decisions (append-only) and the resulting alias map.
CREATE TABLE entity_decisions (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL REFERENCES tenants(id),
    subject_type text NOT NULL,
    left_key     text NOT NULL,
    right_key    text NOT NULL,
    outcome      text NOT NULL CHECK (outcome IN ('same', 'different')),
    reasons      jsonb NOT NULL DEFAULT '[]',
    decided_by   uuid NOT NULL,
    decided_at   timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE entity_aliases (
    tenant_id     uuid NOT NULL REFERENCES tenants(id),
    subject_type  text NOT NULL,
    alias_key     text NOT NULL,
    canonical_key text NOT NULL,
    decision_id   uuid NOT NULL REFERENCES entity_decisions(id),
    PRIMARY KEY (tenant_id, subject_type, alias_key),
    CHECK (alias_key <> canonical_key)
);

CREATE TABLE conflicts (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL REFERENCES tenants(id),
    kind         text NOT NULL CHECK (kind IN ('revision_mismatch', 'unit_incompatible', 'value_mismatch',
                    'tolerance_violation', 'missing_approval', 'expired_evidence', 'wrong_applicability',
                    'conflicting_supplier_record', 'conflicting_inspection')),
    subject_type text NOT NULL,
    subject_key  text NOT NULL,
    claim_a      uuid NOT NULL,
    claim_b      uuid,
    explanation  jsonb NOT NULL,
    rule_version text NOT NULL,
    status       text NOT NULL DEFAULT 'open'
                 CHECK (status IN ('open', 'needs_evidence', 'resolved_approved', 'resolved_rejected', 'obsolete')),
    detected_at  timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (tenant_id, claim_a) REFERENCES claims(tenant_id, id),
    FOREIGN KEY (tenant_id, claim_b) REFERENCES claims(tenant_id, id),
    UNIQUE (tenant_id, id)
);
CREATE UNIQUE INDEX conflicts_dedup ON conflicts (tenant_id, kind, claim_a, coalesce(claim_b, claim_a));

CREATE TABLE review_items (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL REFERENCES tenants(id),
    target_type text NOT NULL CHECK (target_type IN ('conflict', 'claim')),
    target_id   uuid NOT NULL,
    decision    text NOT NULL CHECK (decision IN ('approve', 'reject', 'request_evidence')),
    comment     text CHECK (length(comment) <= 2000),
    reviewer    uuid NOT NULL,
    decided_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX review_items_target ON review_items (tenant_id, target_type, target_id);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['extraction_runs', 'entity_decisions', 'review_items'] LOOP
        EXECUTE format('CREATE TRIGGER %I BEFORE UPDATE OR DELETE ON %I FOR EACH ROW EXECUTE FUNCTION kz_forbid_change()',
                       t || '_append_only', t);
    END LOOP;
    FOREACH t IN ARRAY ARRAY['extraction_runs', 'entity_decisions', 'entity_aliases', 'conflicts', 'review_items'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY tenant_isolation ON %I USING (tenant_id = kz_tenant_id()) '
                       'WITH CHECK (tenant_id = kz_tenant_id())', t);
    END LOOP;
END $$;

GRANT SELECT, INSERT ON extraction_runs, entity_decisions, entity_aliases, conflicts, review_items TO kizuna_app;
GRANT UPDATE (status) ON conflicts TO kizuna_app;

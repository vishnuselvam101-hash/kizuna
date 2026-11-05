-- M4 temporal & decision engine: decisions, typed evidence dependencies, invalidation events.

CREATE TABLE decisions (
    id                uuid PRIMARY KEY,
    tenant_id         uuid NOT NULL REFERENCES tenants(id),
    kind              text NOT NULL CHECK (kind ~ '^[a-z][a-z0-9_]{2,60}$'),
    title             text NOT NULL CHECK (length(title) BETWEEN 1 AND 200),
    subject_type      text NOT NULL,
    subject_key       text NOT NULL,
    outcome           text NOT NULL CHECK (outcome IN ('approved', 'rejected', 'conditional')),
    rationale         text NOT NULL CHECK (length(rationale) BETWEEN 1 AND 4000),
    effective_from    timestamptz,
    decided_by        uuid NOT NULL,
    decided_at        timestamptz NOT NULL DEFAULT now(),
    status            text NOT NULL DEFAULT 'valid'
                      CHECK (status IN ('valid', 'potentially_affected', 'superseded', 'revoked')),
    derivation        jsonb NOT NULL,
    derivation_sha256 bytea NOT NULL CHECK (length(derivation_sha256) = 32),
    supersedes        uuid,
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, supersedes) REFERENCES decisions(tenant_id, id)
);

CREATE TABLE decision_dependencies (
    id                  uuid PRIMARY KEY,
    tenant_id           uuid NOT NULL,
    decision_id         uuid NOT NULL,
    depends_on_claim    uuid,
    depends_on_decision uuid,
    role                text NOT NULL CHECK (role IN ('basis', 'supporting', 'condition')),
    subject_key         text NOT NULL,
    predicate           text,
    applicability       jsonb NOT NULL DEFAULT '{}',
    materiality         jsonb NOT NULL,
    snapshot            jsonb NOT NULL,
    CHECK ((depends_on_claim IS NULL) <> (depends_on_decision IS NULL)),
    FOREIGN KEY (tenant_id, decision_id) REFERENCES decisions(tenant_id, id),
    FOREIGN KEY (tenant_id, depends_on_claim) REFERENCES claims(tenant_id, id),
    FOREIGN KEY (tenant_id, depends_on_decision) REFERENCES decisions(tenant_id, id)
);
CREATE INDEX decision_deps_claim ON decision_dependencies (tenant_id, depends_on_claim);
CREATE INDEX decision_deps_subject ON decision_dependencies (tenant_id, subject_key, predicate);
CREATE INDEX decision_deps_decision ON decision_dependencies (tenant_id, depends_on_decision);

CREATE TABLE invalidation_events (
    id              uuid PRIMARY KEY,
    tenant_id       uuid NOT NULL,
    decision_id     uuid NOT NULL,
    cause_claim     uuid,
    cause_decision  uuid,
    cause_kind      text NOT NULL,
    semantic_change jsonb NOT NULL,
    material        boolean NOT NULL,
    reason          text NOT NULL,
    path            jsonb NOT NULL,
    previous_status text NOT NULL,
    new_status      text NOT NULL,
    recorded_at     timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (tenant_id, decision_id) REFERENCES decisions(tenant_id, id)
);
CREATE INDEX invalidation_events_decision ON invalidation_events (tenant_id, decision_id);

-- Decision content is immutable; only status may change (and never back from superseded/revoked).
CREATE FUNCTION kz_decisions_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'decisions are append-only' USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF (to_jsonb(NEW) - 'status') IS DISTINCT FROM (to_jsonb(OLD) - 'status') THEN
        RAISE EXCEPTION 'decision content is immutable' USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF OLD.status IN ('superseded', 'revoked') AND NEW.status <> OLD.status THEN
        RAISE EXCEPTION 'decision status % is final', OLD.status USING ERRCODE = 'insufficient_privilege';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER decisions_guard BEFORE UPDATE OR DELETE ON decisions FOR EACH ROW EXECUTE FUNCTION kz_decisions_guard();
CREATE TRIGGER decision_dependencies_append_only BEFORE UPDATE OR DELETE ON decision_dependencies
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();
CREATE TRIGGER invalidation_events_append_only BEFORE UPDATE OR DELETE ON invalidation_events
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['decisions', 'decision_dependencies', 'invalidation_events'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY tenant_isolation ON %I USING (tenant_id = kz_tenant_id()) '
                       'WITH CHECK (tenant_id = kz_tenant_id())', t);
    END LOOP;
END $$;

ALTER TABLE review_items DROP CONSTRAINT review_items_target_type_check;
ALTER TABLE review_items ADD CONSTRAINT review_items_target_type_check
    CHECK (target_type IN ('conflict', 'claim', 'decision'));
ALTER TABLE review_items DROP CONSTRAINT review_items_decision_check;
ALTER TABLE review_items ADD CONSTRAINT review_items_decision_check
    CHECK (decision IN ('approve', 'reject', 'request_evidence', 'reconfirm', 'revoke'));

GRANT SELECT, INSERT ON decisions, decision_dependencies, invalidation_events TO kizuna_app;
GRANT UPDATE (status) ON decisions TO kizuna_app;

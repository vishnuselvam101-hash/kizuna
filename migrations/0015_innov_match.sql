-- Quality three-way match (PO line ↔ drawing ↔ certificate) and minimal supplier re-requests.
-- Integrator: renumber if another 0015 lands first (no dependency on 0014).

-- One evaluation of one PO line, kept as recorded (append-only). `decision_id` is set when the run was the basis of
-- an acceptance decision; that decision's typed dependencies are the claims the run cited.
CREATE TABLE match_runs (
    id              uuid PRIMARY KEY,
    tenant_id       uuid NOT NULL REFERENCES tenants(id),
    purchase_order  text NOT NULL CHECK (length(purchase_order) BETWEEN 1 AND 200),
    po_line         text NOT NULL CHECK (length(po_line) BETWEEN 1 AND 40),
    part_key        text NOT NULL CHECK (length(part_key) BETWEEN 1 AND 200),
    rule_version    text NOT NULL,
    result          jsonb NOT NULL,
    result_sha256   bytea NOT NULL CHECK (length(result_sha256) = 32),
    blocking        int NOT NULL CHECK (blocking >= 0),
    decision_id     uuid,
    created_by      uuid NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, decision_id) REFERENCES decisions(tenant_id, id)
);
CREATE INDEX match_runs_line ON match_runs (tenant_id, purchase_order, po_line, created_at);

CREATE TABLE match_findings (
    id                 uuid PRIMARY KEY,
    tenant_id          uuid NOT NULL,
    run_id             uuid NOT NULL,
    kind               text NOT NULL CHECK (kind ~ '^[a-z][a-z_]{2,60}$'),
    severity           text NOT NULL CHECK (severity IN ('critical', 'major', 'minor')),
    check_name         text NOT NULL CHECK (length(check_name) <= 120),
    po_claims          uuid[] NOT NULL,
    drawing_claims     uuid[] NOT NULL,
    certificate_claims uuid[] NOT NULL,
    certificate_doc    uuid,
    explanation        jsonb NOT NULL,
    FOREIGN KEY (tenant_id, run_id) REFERENCES match_runs(tenant_id, id),
    FOREIGN KEY (tenant_id, certificate_doc) REFERENCES artifacts(tenant_id, id)
);
CREATE INDEX match_findings_run ON match_findings (tenant_id, run_id);

-- Supplier re-request drafts. Content is immutable; only the review status moves draft → approved | cancelled.
-- Nothing is sent by KIZUNA: an approved draft is downloaded and sent by a person.
CREATE TABLE evidence_requests (
    id                 uuid PRIMARY KEY,
    tenant_id          uuid NOT NULL REFERENCES tenants(id),
    reference          text NOT NULL CHECK (length(reference) BETWEEN 1 AND 40),
    supplier           text NOT NULL CHECK (length(supplier) BETWEEN 1 AND 200),
    engine             text NOT NULL,
    decision_ids       uuid[] NOT NULL,
    documents          jsonb NOT NULL,
    requested_count    int NOT NULL CHECK (requested_count >= 1),
    baseline_count     int NOT NULL CHECK (baseline_count >= requested_count),
    exact              boolean NOT NULL,
    body_ja            text NOT NULL CHECK (length(body_ja) <= 100000),
    body_en            text NOT NULL CHECK (length(body_en) <= 100000),
    content_sha256     bytea NOT NULL CHECK (length(content_sha256) = 32),
    status             text NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'approved', 'cancelled')),
    created_by         uuid NOT NULL,
    created_at         timestamptz NOT NULL DEFAULT now(),
    reviewed_by        uuid,
    reviewed_at        timestamptz,
    review_comment     text CHECK (length(review_comment) <= 2000),
    CHECK ((status = 'draft') = (reviewed_by IS NULL)),
    CHECK ((reviewed_by IS NULL) = (reviewed_at IS NULL)),
    UNIQUE (tenant_id, id)
);
CREATE INDEX evidence_requests_status ON evidence_requests (tenant_id, status, created_at);

CREATE FUNCTION kz_evidence_requests_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'evidence requests are append-only' USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF (to_jsonb(NEW) - ARRAY['status', 'reviewed_by', 'reviewed_at', 'review_comment'])
        IS DISTINCT FROM (to_jsonb(OLD) - ARRAY['status', 'reviewed_by', 'reviewed_at', 'review_comment']) THEN
        RAISE EXCEPTION 'evidence request content is immutable' USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF OLD.status <> 'draft' THEN
        RAISE EXCEPTION 'evidence request status % is final', OLD.status USING ERRCODE = 'insufficient_privilege';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER evidence_requests_guard BEFORE UPDATE OR DELETE ON evidence_requests
    FOR EACH ROW EXECUTE FUNCTION kz_evidence_requests_guard();
CREATE TRIGGER match_runs_append_only BEFORE UPDATE OR DELETE ON match_runs
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();
CREATE TRIGGER match_findings_append_only BEFORE UPDATE OR DELETE ON match_findings
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['match_runs', 'match_findings', 'evidence_requests'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY tenant_isolation ON %I USING (tenant_id = kz_tenant_id()) '
                       'WITH CHECK (tenant_id = kz_tenant_id())', t);
    END LOOP;
END $$;

GRANT SELECT, INSERT ON match_runs, match_findings, evidence_requests TO kizuna_app;
GRANT UPDATE (status, reviewed_by, reviewed_at, review_comment) ON evidence_requests TO kizuna_app;

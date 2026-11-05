-- Corrective-action cases (8D / 是正処置), ADR 0024. A case opens from a finding and carries its evidence links;
-- disciplines D1–D8 are structured steps whose completion needs cited evidence; closure needs a second person.
-- Every transition is in the audit chain and in capa_events (append-only).

CREATE TABLE capa_cases (
    id                   uuid PRIMARY KEY,
    tenant_id            uuid NOT NULL REFERENCES tenants(id),
    reference            text NOT NULL CHECK (length(reference) BETWEEN 1 AND 40),
    title                text NOT NULL CHECK (length(title) BETWEEN 1 AND 200),
    source_kind          text NOT NULL CHECK (source_kind IN ('conflict', 'match_finding', 'integrity_signal',
                             'drift_finding', 'spc')),
    -- The finding id, or for SPC "subject_type|subject_key|predicate".
    source_ref           text NOT NULL CHECK (length(source_ref) BETWEEN 1 AND 500),
    -- Snapshot of the finding at opening (explanation, SPC limits) and the evidence it carried.
    source               jsonb NOT NULL,
    source_links         jsonb NOT NULL CHECK (jsonb_typeof(source_links) = 'array'),
    subject_type         text,
    subject_key          text,
    supplier_key         text CHECK (length(supplier_key) <= 200),
    status               text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'closure_pending', 'closed')),
    reopened             int NOT NULL DEFAULT 0 CHECK (reopened >= 0),
    closure_requested_by uuid,
    closure_requested_at timestamptz,
    closed_by            uuid,
    closed_at            timestamptz,
    created_by           uuid NOT NULL,
    created_at           timestamptz NOT NULL DEFAULT now(),
    updated_at           timestamptz NOT NULL DEFAULT now(),
    CHECK ((status = 'open') = (closure_requested_by IS NULL)),
    CHECK ((status = 'closed') = (closed_by IS NOT NULL)),
    CHECK (closed_by IS NULL OR closed_by <> closure_requested_by),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, reference)
);
-- One active case per finding.
CREATE UNIQUE INDEX capa_cases_active_source ON capa_cases (tenant_id, source_kind, source_ref) WHERE status <> 'closed';
CREATE INDEX capa_cases_list ON capa_cases (tenant_id, status, updated_at DESC);

CREATE TABLE capa_steps (
    tenant_id    uuid NOT NULL REFERENCES tenants(id),
    case_id      uuid NOT NULL,
    discipline   smallint NOT NULL CHECK (discipline BETWEEN 1 AND 8),
    status       text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'done')),
    content      jsonb NOT NULL DEFAULT '{}',
    completed_by uuid,
    completed_at timestamptz,
    updated_by   uuid,
    updated_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (case_id, discipline),
    CHECK ((status = 'done') = (completed_at IS NOT NULL)),
    FOREIGN KEY (tenant_id, case_id) REFERENCES capa_cases(tenant_id, id)
);

CREATE TABLE capa_actions (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL REFERENCES tenants(id),
    case_id      uuid NOT NULL,
    description  text NOT NULL CHECK (length(description) BETWEEN 1 AND 2000),
    owner_id     uuid NOT NULL,
    due_date     date NOT NULL,
    -- Indexes into the D4 causes this action addresses.
    causes       int[] NOT NULL CHECK (cardinality(causes) >= 1),
    status       text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'done')),
    evidence     jsonb NOT NULL DEFAULT '[]',
    done_by      uuid,
    done_at      timestamptz,
    done_comment text CHECK (length(done_comment) <= 2000),
    created_by   uuid NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    CHECK ((status = 'done') = (done_at IS NOT NULL)),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, case_id) REFERENCES capa_cases(tenant_id, id),
    FOREIGN KEY (tenant_id, owner_id) REFERENCES users(tenant_id, id)
);
CREATE INDEX capa_actions_case ON capa_actions (tenant_id, case_id);
CREATE INDEX capa_actions_due ON capa_actions (due_date) WHERE status = 'open';

-- A request to the supplier for their own 8D response. Drafted, approved by a second person, sent by a person:
-- KIZUNA never sends. Content immutable; only the review fields move.
CREATE TABLE capa_supplier_requests (
    id             uuid PRIMARY KEY,
    tenant_id      uuid NOT NULL REFERENCES tenants(id),
    case_id        uuid NOT NULL,
    reference      text NOT NULL CHECK (length(reference) BETWEEN 1 AND 60),
    supplier       text NOT NULL CHECK (length(supplier) BETWEEN 1 AND 200),
    disciplines    smallint[] NOT NULL CHECK (cardinality(disciplines) >= 1),
    due_date       date NOT NULL,
    body_ja        text NOT NULL CHECK (length(body_ja) <= 100000),
    body_en        text NOT NULL CHECK (length(body_en) <= 100000),
    content_sha256 bytea NOT NULL CHECK (length(content_sha256) = 32),
    status         text NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'approved', 'cancelled')),
    created_by     uuid NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    reviewed_by    uuid,
    reviewed_at    timestamptz,
    review_comment text CHECK (length(review_comment) <= 2000),
    CHECK ((status = 'draft') = (reviewed_by IS NULL)),
    CHECK (reviewed_by IS NULL OR reviewed_by <> created_by),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, case_id) REFERENCES capa_cases(tenant_id, id)
);

-- Documents the supplier sent back for an approved request: evidence of origin "supplier", never an approval.
CREATE TABLE capa_supplier_responses (
    tenant_id   uuid NOT NULL REFERENCES tenants(id),
    case_id     uuid NOT NULL,
    request_id  uuid NOT NULL,
    artifact_id uuid NOT NULL,
    added_by    uuid NOT NULL,
    added_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (case_id, artifact_id),
    FOREIGN KEY (tenant_id, case_id) REFERENCES capa_cases(tenant_id, id),
    FOREIGN KEY (tenant_id, request_id) REFERENCES capa_supplier_requests(tenant_id, id),
    FOREIGN KEY (tenant_id, artifact_id) REFERENCES artifacts(tenant_id, id)
);

CREATE TABLE capa_events (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenants(id),
    case_id    uuid NOT NULL,
    at         timestamptz NOT NULL DEFAULT now(),
    actor_id   uuid,
    actor_kind text NOT NULL CHECK (actor_kind IN ('user', 'system')),
    kind       text NOT NULL CHECK (kind ~ '^[a-z][a-z_.]{2,60}$'),
    discipline smallint CHECK (discipline BETWEEN 1 AND 8),
    detail     jsonb NOT NULL DEFAULT '{}',
    FOREIGN KEY (tenant_id, case_id) REFERENCES capa_cases(tenant_id, id)
);
CREATE INDEX capa_events_case ON capa_events (tenant_id, case_id, at);

-- In-app notifications (overdue corrective actions). One per action, recipient and due date.
CREATE TABLE capa_notifications (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenants(id),
    user_id    uuid NOT NULL,
    case_id    uuid NOT NULL,
    action_id  uuid NOT NULL,
    kind       text NOT NULL CHECK (kind IN ('action_overdue')),
    due_date   date NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    read_at    timestamptz,
    UNIQUE (tenant_id, action_id, user_id, kind, due_date),
    FOREIGN KEY (tenant_id, case_id) REFERENCES capa_cases(tenant_id, id),
    FOREIGN KEY (tenant_id, action_id) REFERENCES capa_actions(tenant_id, id)
);
CREATE INDEX capa_notifications_user ON capa_notifications (tenant_id, user_id, created_at DESC);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['capa_cases', 'capa_steps', 'capa_actions', 'capa_supplier_requests',
                             'capa_supplier_responses', 'capa_events', 'capa_notifications'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY tenant_isolation ON %I USING (tenant_id = kz_tenant_id()) '
                       'WITH CHECK (tenant_id = kz_tenant_id())', t);
    END LOOP;
END $$;

GRANT SELECT, INSERT ON capa_cases, capa_steps, capa_actions, capa_supplier_requests, capa_supplier_responses,
    capa_events, capa_notifications TO kizuna_app;
GRANT UPDATE (status, reopened, closure_requested_by, closure_requested_at, closed_by, closed_at, updated_at)
    ON capa_cases TO kizuna_app;
GRANT UPDATE (status, content, completed_by, completed_at, updated_by, updated_at) ON capa_steps TO kizuna_app;
GRANT UPDATE (status, evidence, done_by, done_at, done_comment) ON capa_actions TO kizuna_app;
GRANT UPDATE (status, reviewed_by, reviewed_at, review_comment) ON capa_supplier_requests TO kizuna_app;
GRANT UPDATE (read_at) ON capa_notifications TO kizuna_app;

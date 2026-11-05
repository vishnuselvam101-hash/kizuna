-- Review batches: an evidence change or decision revocation whose impact exceeds the inline bound is committed
-- anyway, and its invalidation is handed to a background job that applies it in bounded, resumable chunks.
-- Until the batch is done its subject is "impact analysis in progress": no new decision may rely on it.

CREATE TABLE review_batches (
    id              uuid PRIMARY KEY,
    tenant_id       uuid NOT NULL REFERENCES tenants(id),
    -- The cause, replayable by the job: {"type":"claim","change":{...}} or {"type":"revoked","decision":"<uuid>"}.
    cause           jsonb NOT NULL,
    cause_claim     uuid,
    cause_decision  uuid,
    subject_type    text NOT NULL,
    subject_keys    text[] NOT NULL CHECK (cardinality(subject_keys) >= 1),
    status          text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'running', 'done')),
    total           int CHECK (total >= 0),
    processed       int NOT NULL DEFAULT 0 CHECK (processed >= 0),
    flagged         int NOT NULL DEFAULT 0 CHECK (flagged >= 0),
    created_by      uuid NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    finished_at     timestamptz,
    UNIQUE (tenant_id, id),
    CHECK (total IS NULL OR processed <= total)
);
CREATE INDEX review_batches_open ON review_batches (tenant_id, subject_type) WHERE status <> 'done';

-- The impacts computed once when the job starts the batch; `applied` flips in the same transaction that applies
-- the impact, so a restart resumes exactly where the last committed chunk ended.
CREATE TABLE review_batch_items (
    tenant_id   uuid NOT NULL REFERENCES tenants(id),
    batch_id    uuid NOT NULL,
    seq         int NOT NULL CHECK (seq >= 0),
    decision_id uuid NOT NULL,
    impact      jsonb NOT NULL,
    applied     boolean NOT NULL DEFAULT false,
    PRIMARY KEY (batch_id, seq),
    FOREIGN KEY (tenant_id, batch_id) REFERENCES review_batches(tenant_id, id)
);
CREATE INDEX review_batch_items_pending ON review_batch_items (batch_id, seq) WHERE NOT applied;

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['review_batches', 'review_batch_items'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY tenant_isolation ON %I USING (tenant_id = kz_tenant_id()) '
                       'WITH CHECK (tenant_id = kz_tenant_id())', t);
    END LOOP;
END $$;

GRANT SELECT, INSERT ON review_batches, review_batch_items TO kizuna_app;
GRANT UPDATE (status, total, processed, flagged, updated_at, finished_at) ON review_batches TO kizuna_app;
GRANT UPDATE (applied) ON review_batch_items TO kizuna_app;

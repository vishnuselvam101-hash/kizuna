-- Cross-standard material grade guard (mfg-grade@1, mfg-match@2). KIZUNA ships NO standards content: these tables
-- start empty for every tenant and hold only what the tenant curates (typed in, or imported from its own CSV).
-- Nothing in them is authoritative: every row is shown as "unverified — check against the standard" unless the
-- tenant recorded (four-eyes) that it checked the row against the standard document.
--
-- Each row is one VERSION of an equivalence entry (entry_id is stable across versions). A change is proposed and
-- becomes effective only when a second person approves it (four-eyes, enforced here and in the API). Rows are never
-- deleted; only the status columns move, forward only. Approving an edit or a retirement supersedes the previous
-- approved version and triggers selective invalidation of the decisions that depend on that entry
-- (decision_dependencies.depends_on_equivalence).
CREATE TABLE grade_equivalences (
    id               uuid PRIMARY KEY,
    tenant_id        uuid NOT NULL REFERENCES tenants(id),
    entry_id         uuid NOT NULL,
    version          int NOT NULL CHECK (version >= 1),
    change           text NOT NULL CHECK (change IN ('create', 'edit', 'retire')),
    a_standard       text CHECK (a_standard IS NULL OR char_length(a_standard) BETWEEN 1 AND 40),
    a_grade          text NOT NULL CHECK (char_length(a_grade) BETWEEN 1 AND 80),
    b_standard       text CHECK (b_standard IS NULL OR char_length(b_standard) BETWEEN 1 AND 40),
    b_grade          text NOT NULL CHECK (char_length(b_grade) BETWEEN 1 AND 80),
    -- Order-independent normalised identity of the pair (notation rules, mfg-grade@1).
    pair_key         text NOT NULL CHECK (char_length(pair_key) BETWEEN 3 AND 300),
    equivalence      text NOT NULL CHECK (equivalence IN ('identical', 'acceptable_with_conditions', 'not_equivalent')),
    conditions       text CHECK (conditions IS NULL OR char_length(conditions) BETWEEN 1 AND 2000),
    reference        text CHECK (reference IS NULL OR char_length(reference) <= 500),
    -- Every entry is "unverified — check against the standard" until the proposer records that it was checked
    -- against the standard document (named in `reference`) and a second person approves that statement.
    source_checked   boolean NOT NULL DEFAULT false,
    status           text NOT NULL DEFAULT 'proposed'
                     CHECK (status IN ('proposed', 'approved', 'rejected', 'superseded', 'retired')),
    proposed_by      uuid NOT NULL,
    proposed_at      timestamptz NOT NULL DEFAULT now(),
    proposal_note    text CHECK (proposal_note IS NULL OR char_length(proposal_note) <= 2000),
    import_batch     uuid,
    supersedes       uuid,
    decided_by       uuid,
    decided_at       timestamptz,
    decision_comment text CHECK (decision_comment IS NULL OR char_length(decision_comment) <= 2000),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, entry_id, version),
    FOREIGN KEY (tenant_id, supersedes) REFERENCES grade_equivalences(tenant_id, id),
    CHECK (equivalence <> 'acceptable_with_conditions' OR conditions IS NOT NULL),
    CHECK (NOT source_checked OR reference IS NOT NULL),
    CHECK ((status = 'retired') = (change = 'retire' AND status NOT IN ('proposed', 'rejected'))),
    CHECK ((change = 'create') = (supersedes IS NULL)),
    CHECK ((decided_by IS NULL) = (decided_at IS NULL)),
    CHECK (status = 'proposed' OR decided_by IS NOT NULL),
    -- Four-eyes: nobody approves or rejects their own proposal.
    CHECK (decided_by IS NULL OR decided_by <> proposed_by)
);
-- One effective version per entry, one open proposal per entry, one effective entry per grade pair.
CREATE UNIQUE INDEX grade_equivalences_effective ON grade_equivalences (tenant_id, entry_id) WHERE status = 'approved';
CREATE UNIQUE INDEX grade_equivalences_open ON grade_equivalences (tenant_id, entry_id) WHERE status = 'proposed';
CREATE UNIQUE INDEX grade_equivalences_pair ON grade_equivalences (tenant_id, pair_key) WHERE status = 'approved';
CREATE INDEX grade_equivalences_entry ON grade_equivalences (tenant_id, entry_id, version);

-- Shared guard for the tenant-curated grade tables: rows are never deleted, content is immutable, and the status
-- moves forward only: proposed → approved | rejected | retired (an approved retirement), approved → superseded.
CREATE FUNCTION kz_grade_curation_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    st constant text[] := ARRAY['status', 'decided_by', 'decided_at', 'decision_comment'];
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION '% is append-only', TG_TABLE_NAME USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF (to_jsonb(NEW) - st) IS DISTINCT FROM (to_jsonb(OLD) - st) THEN
        RAISE EXCEPTION '% content is immutable', TG_TABLE_NAME USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF NOT ((OLD.status = 'proposed' AND NEW.status IN ('approved', 'rejected', 'retired'))
            OR (OLD.status = 'approved' AND NEW.status = 'superseded')
            OR (OLD.status = NEW.status AND OLD.decided_by IS NOT DISTINCT FROM NEW.decided_by
                AND OLD.decided_at IS NOT DISTINCT FROM NEW.decided_at
                AND OLD.decision_comment IS NOT DISTINCT FROM NEW.decision_comment)) THEN
        RAISE EXCEPTION '% status % -> % not allowed', TG_TABLE_NAME, OLD.status, NEW.status
            USING ERRCODE = 'check_violation';
    END IF;
    -- Superseding keeps the original approval record.
    IF OLD.status = 'approved' AND (NEW.decided_by, NEW.decided_at, NEW.decision_comment)
        IS DISTINCT FROM (OLD.decided_by, OLD.decided_at, OLD.decision_comment) THEN
        RAISE EXCEPTION 'approval record is immutable' USING ERRCODE = 'insufficient_privilege';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER grade_equivalences_guard BEFORE UPDATE OR DELETE ON grade_equivalences
    FOR EACH ROW EXECUTE FUNCTION kz_grade_curation_guard();

ALTER TABLE grade_equivalences ENABLE ROW LEVEL SECURITY;
ALTER TABLE grade_equivalences FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON grade_equivalences USING (tenant_id = kz_tenant_id())
    WITH CHECK (tenant_id = kz_tenant_id());
GRANT SELECT, INSERT ON grade_equivalences TO kizuna_app;
GRANT UPDATE (status, decided_by, decided_at, decision_comment) ON grade_equivalences TO kizuna_app;

-- Typed dependency of a decision on one approved equivalence VERSION (exactly one target per dependency).
ALTER TABLE decision_dependencies ADD COLUMN depends_on_equivalence uuid;
ALTER TABLE decision_dependencies ADD CONSTRAINT decision_dependencies_equivalence_fk
    FOREIGN KEY (tenant_id, depends_on_equivalence) REFERENCES grade_equivalences(tenant_id, id);
ALTER TABLE decision_dependencies DROP CONSTRAINT decision_dependencies_check;
ALTER TABLE decision_dependencies ADD CONSTRAINT decision_dependencies_one_target CHECK (
    num_nonnulls(depends_on_claim, depends_on_decision, depends_on_equivalence) = 1);
CREATE INDEX decision_deps_equivalence ON decision_dependencies (tenant_id, depends_on_equivalence)
    WHERE depends_on_equivalence IS NOT NULL;

-- Only an approved version can back a decision (any writer).
CREATE FUNCTION kz_dependency_equivalence_approved() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.depends_on_equivalence IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM grade_equivalences g
         WHERE g.tenant_id = NEW.tenant_id AND g.id = NEW.depends_on_equivalence AND g.status = 'approved'
    ) THEN
        RAISE EXCEPTION 'only an approved grade equivalence can back a decision' USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER decision_dependencies_equivalence_approved BEFORE INSERT ON decision_dependencies
    FOR EACH ROW EXECUTE FUNCTION kz_dependency_equivalence_approved();

ALTER TABLE invalidation_events ADD COLUMN cause_equivalence uuid;

-- Tenant-recorded property limits of a grade (chemistry or mechanical), versioned and four-eyes like the
-- equivalences. Used to flag certificate values that fail the required grade although the names look equivalent.
CREATE TABLE grade_limits (
    id               uuid PRIMARY KEY,
    tenant_id        uuid NOT NULL REFERENCES tenants(id),
    entry_id         uuid NOT NULL,
    version          int NOT NULL CHECK (version >= 1),
    change           text NOT NULL CHECK (change IN ('create', 'edit', 'retire')),
    standard         text CHECK (standard IS NULL OR char_length(standard) BETWEEN 1 AND 40),
    grade            text NOT NULL CHECK (char_length(grade) BETWEEN 1 AND 80),
    property         text NOT NULL CHECK (property ~ '^[a-z][a-z0-9_]{0,59}$'),
    limit_key        text NOT NULL CHECK (char_length(limit_key) BETWEEN 3 AND 300),
    min_value        numeric,
    max_value        numeric,
    unit             text CHECK (unit IS NULL OR char_length(unit) BETWEEN 1 AND 20),
    reference        text CHECK (reference IS NULL OR char_length(reference) <= 500),
    source_checked   boolean NOT NULL DEFAULT false,
    status           text NOT NULL DEFAULT 'proposed'
                     CHECK (status IN ('proposed', 'approved', 'rejected', 'superseded', 'retired')),
    proposed_by      uuid NOT NULL,
    proposed_at      timestamptz NOT NULL DEFAULT now(),
    proposal_note    text CHECK (proposal_note IS NULL OR char_length(proposal_note) <= 2000),
    supersedes       uuid,
    decided_by       uuid,
    decided_at       timestamptz,
    decision_comment text CHECK (decision_comment IS NULL OR char_length(decision_comment) <= 2000),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, entry_id, version),
    FOREIGN KEY (tenant_id, supersedes) REFERENCES grade_limits(tenant_id, id),
    CHECK (min_value IS NOT NULL OR max_value IS NOT NULL),
    CHECK (min_value IS NULL OR max_value IS NULL OR min_value <= max_value),
    CHECK (NOT source_checked OR reference IS NOT NULL),
    CHECK ((change = 'create') = (supersedes IS NULL)),
    CHECK ((status = 'retired') = (change = 'retire' AND status NOT IN ('proposed', 'rejected'))),
    CHECK ((decided_by IS NULL) = (decided_at IS NULL)),
    CHECK (status = 'proposed' OR decided_by IS NOT NULL),
    CHECK (decided_by IS NULL OR decided_by <> proposed_by)
);
CREATE UNIQUE INDEX grade_limits_effective ON grade_limits (tenant_id, entry_id) WHERE status = 'approved';
CREATE UNIQUE INDEX grade_limits_open ON grade_limits (tenant_id, entry_id) WHERE status = 'proposed';
CREATE UNIQUE INDEX grade_limits_key ON grade_limits (tenant_id, limit_key) WHERE status = 'approved';
CREATE TRIGGER grade_limits_guard BEFORE UPDATE OR DELETE ON grade_limits
    FOR EACH ROW EXECUTE FUNCTION kz_grade_curation_guard();
ALTER TABLE grade_limits ENABLE ROW LEVEL SECURITY;
ALTER TABLE grade_limits FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON grade_limits USING (tenant_id = kz_tenant_id())
    WITH CHECK (tenant_id = kz_tenant_id());
GRANT SELECT, INSERT ON grade_limits TO kizuna_app;
GRANT UPDATE (status, decided_by, decided_at, decision_comment) ON grade_limits TO kizuna_app;

-- Supplier-portal sessions never see or write the tenant's grade tables (deny-by-default, as migration 0029).
CREATE POLICY portal_deny ON grade_equivalences AS RESTRICTIVE USING (kz_portal_supplier_id() IS NULL)
    WITH CHECK (kz_portal_supplier_id() IS NULL);
CREATE POLICY portal_deny ON grade_limits AS RESTRICTIVE USING (kz_portal_supplier_id() IS NULL)
    WITH CHECK (kz_portal_supplier_id() IS NULL);

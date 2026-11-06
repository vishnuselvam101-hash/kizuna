-- Regulation-list versions as dependency nodes (research I6, ADR 0028, `mfg-reglist@1`).
-- A tenant imports versioned substance lists (KIZUNA ships no list content); supplier compliance declarations are
-- claims linked to the list version they answered; material composition (FMD) arrives as claims; tenant-curated
-- material profiles say which substances a material could contain. A new list version is assessed once, and the
-- assessment (every declaration considered, with its verdict) is kept as an append-only record.

-- Upload kinds for the three CSV sources.
ALTER TABLE artifacts DROP CONSTRAINT artifacts_kind_check;
ALTER TABLE artifacts ADD CONSTRAINT artifacts_kind_check CHECK (kind IN ('spec_document', 'drawing', 'purchase_order',
    'inspection_report', 'spreadsheet', 'other', 'regulation_list', 'compliance_declaration', 'material_declaration'));

CREATE TABLE reg_lists (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenants(id),
    code       text NOT NULL CHECK (code ~ '^[a-z][a-z0-9_]{1,39}$'),
    name       text NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    created_by uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, code),
    UNIQUE (tenant_id, id)
);

CREATE TABLE reg_list_versions (
    id               uuid PRIMARY KEY,
    tenant_id        uuid NOT NULL,
    list_id          uuid NOT NULL,
    label            text NOT NULL CHECK (label ~ '^[A-Za-z0-9._-]{1,40}$'),
    seq              int NOT NULL CHECK (seq >= 1),
    artifact_id      uuid NOT NULL,
    previous_id      uuid,
    entry_count      int NOT NULL CHECK (entry_count >= 0),
    engine           text NOT NULL,
    -- Counts of the assessment run at import (declarations considered, flagged, decisions flagged).
    summary          jsonb NOT NULL,
    imported_by      uuid NOT NULL,
    imported_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, list_id, label),
    UNIQUE (tenant_id, list_id, seq),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, list_id) REFERENCES reg_lists (tenant_id, id),
    FOREIGN KEY (tenant_id, artifact_id) REFERENCES artifacts (tenant_id, id),
    FOREIGN KEY (tenant_id, previous_id) REFERENCES reg_list_versions (tenant_id, id)
);

CREATE TABLE reg_list_entries (
    tenant_id     uuid NOT NULL,
    version_id    uuid NOT NULL,
    substance_key text NOT NULL CHECK (length(substance_key) BETWEEN 3 AND 520),
    name          text NOT NULL CHECK (length(name) BETWEEN 1 AND 500),
    cas           text CHECK (cas ~ '^[0-9]{2,7}-[0-9]{2}-[0-9]$'),
    ec            text CHECK (ec ~ '^[0-9]{3}-[0-9]{3}-[0-9]$'),
    date_added    text CHECK (length(date_added) <= 40),
    PRIMARY KEY (tenant_id, version_id, substance_key),
    FOREIGN KEY (tenant_id, version_id) REFERENCES reg_list_versions (tenant_id, id)
);

-- Tenant-curated: which listed substances a material could contain. Current row = not superseded; an empty array is
-- a curated "contains none of interest"; no row = unknown (never assumed clean).
CREATE TABLE material_profiles (
    id            uuid PRIMARY KEY,
    tenant_id     uuid NOT NULL REFERENCES tenants(id),
    material_key  text NOT NULL CHECK (length(material_key) BETWEEN 1 AND 200),
    substances    text[] NOT NULL CHECK (cardinality(substances) <= 500),
    note          text CHECK (length(note) <= 1000),
    created_by    uuid NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    superseded_at timestamptz,
    superseded_by uuid,
    CHECK ((superseded_at IS NULL) = (superseded_by IS NULL)),
    UNIQUE (tenant_id, id)
);
CREATE UNIQUE INDEX material_profiles_current ON material_profiles (tenant_id, material_key) WHERE superseded_at IS NULL;

-- A declaration claim (`reg.declaration`) and the list version it answered, the supplier and the covered materials.
CREATE TABLE reg_declarations (
    claim_id    uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL,
    version_id  uuid NOT NULL,
    list_id     uuid NOT NULL,
    supplier    text NOT NULL CHECK (length(supplier) BETWEEN 1 AND 200),
    part        text NOT NULL CHECK (length(part) BETWEEN 1 AND 200),
    status      text NOT NULL CHECK (status IN ('compliant', 'contains_listed')),
    materials   text[] NOT NULL CHECK (cardinality(materials) <= 200),
    recorded_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (tenant_id, claim_id) REFERENCES claims (tenant_id, id),
    FOREIGN KEY (tenant_id, version_id) REFERENCES reg_list_versions (tenant_id, id),
    FOREIGN KEY (tenant_id, list_id) REFERENCES reg_lists (tenant_id, id)
);
CREATE INDEX reg_declarations_list ON reg_declarations (tenant_id, list_id, part, supplier);

-- One row per declaration considered when a version was imported (flagged or not, so "why not flagged" is answerable).
CREATE TABLE reg_assessments (
    id                uuid PRIMARY KEY,
    tenant_id         uuid NOT NULL,
    version_id        uuid NOT NULL,
    declaration_claim uuid NOT NULL,
    answered_version  uuid NOT NULL,
    part              text NOT NULL,
    supplier          text NOT NULL,
    verdict           text NOT NULL CHECK (verdict IN ('may_contain', 'cannot_rule_out', 'not_affected')),
    detail            jsonb NOT NULL,
    decisions_flagged int NOT NULL DEFAULT 0,
    created_at        timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (tenant_id, version_id) REFERENCES reg_list_versions (tenant_id, id),
    FOREIGN KEY (tenant_id, answered_version) REFERENCES reg_list_versions (tenant_id, id),
    FOREIGN KEY (tenant_id, declaration_claim) REFERENCES claims (tenant_id, id)
);
CREATE INDEX reg_assessments_version ON reg_assessments (tenant_id, version_id, verdict);

CREATE TRIGGER reg_lists_append_only BEFORE UPDATE OR DELETE ON reg_lists
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();
CREATE TRIGGER reg_list_versions_append_only BEFORE UPDATE OR DELETE ON reg_list_versions
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();
CREATE TRIGGER reg_list_entries_append_only BEFORE UPDATE OR DELETE ON reg_list_entries
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();
CREATE TRIGGER reg_declarations_append_only BEFORE UPDATE OR DELETE ON reg_declarations
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();
CREATE TRIGGER reg_assessments_append_only BEFORE UPDATE OR DELETE ON reg_assessments
    FOR EACH ROW EXECUTE FUNCTION kz_forbid_change();

-- Profiles are superseded once, never rewritten or deleted.
CREATE FUNCTION kz_material_profiles_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'material profiles are append-only' USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF OLD.superseded_at IS NOT NULL
       OR (to_jsonb(NEW) - 'superseded_at' - 'superseded_by') IS DISTINCT FROM (to_jsonb(OLD) - 'superseded_at' - 'superseded_by') THEN
        RAISE EXCEPTION 'material profile content is immutable' USING ERRCODE = 'insufficient_privilege';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER material_profiles_guard BEFORE UPDATE OR DELETE ON material_profiles
    FOR EACH ROW EXECUTE FUNCTION kz_material_profiles_guard();

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['reg_lists', 'reg_list_versions', 'reg_list_entries', 'material_profiles',
                             'reg_declarations', 'reg_assessments'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY tenant_isolation ON %I USING (tenant_id = kz_tenant_id()) '
                       'WITH CHECK (tenant_id = kz_tenant_id())', t);
        -- Invisible and unwritable to a supplier portal session (deny-by-default, migration 0029).
        EXECUTE format('CREATE POLICY portal_deny ON %I AS RESTRICTIVE USING (kz_portal_supplier_id() IS NULL) '
                       'WITH CHECK (kz_portal_supplier_id() IS NULL)', t);
    END LOOP;
END $$;

GRANT SELECT, INSERT ON reg_lists, reg_list_versions, reg_list_entries, material_profiles, reg_declarations,
    reg_assessments TO kizuna_app;
GRANT UPDATE (superseded_at, superseded_by) ON material_profiles TO kizuna_app;

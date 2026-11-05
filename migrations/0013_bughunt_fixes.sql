-- Bughunt fixes.
-- Tenant-local calendar: expiry checks and report date ranges use the tenant's UTC offset (default JST, +09:00).
ALTER TABLE tenants ADD COLUMN utc_offset_minutes int NOT NULL DEFAULT 540
    CHECK (utc_offset_minutes BETWEEN -720 AND 840 AND utc_offset_minutes % 15 = 0);

-- Decision links carry the subject type, so a part and a supplier with the same key never cross-trigger.
ALTER TABLE decision_dependencies ADD COLUMN subject_type text;
-- Backfill across tenants: the migrating owner temporarily lifts FORCE RLS (and the append-only trigger).
ALTER TABLE decision_dependencies NO FORCE ROW LEVEL SECURITY;
ALTER TABLE claims NO FORCE ROW LEVEL SECURITY;
ALTER TABLE decision_dependencies DISABLE TRIGGER decision_dependencies_append_only;
UPDATE decision_dependencies d SET subject_type = c.subject_type FROM claims c WHERE c.id = d.depends_on_claim;
ALTER TABLE decision_dependencies ENABLE TRIGGER decision_dependencies_append_only;
ALTER TABLE decision_dependencies FORCE ROW LEVEL SECURITY;
ALTER TABLE claims FORCE ROW LEVEL SECURITY;
ALTER TABLE decision_dependencies ADD CONSTRAINT decision_dependencies_claim_type
    CHECK (depends_on_claim IS NULL OR subject_type IS NOT NULL);

-- Explicit re-extraction (e.g. after an import-settings change): a later successful run of the same artifact and
-- mapping version must name the run it supersedes. The first successful run stays unique.
ALTER TABLE extraction_runs ADD COLUMN supersedes_run uuid REFERENCES extraction_runs(id);
ALTER TABLE extraction_runs ADD COLUMN import_settings_version int NOT NULL DEFAULT 0;
DROP INDEX extraction_runs_once;
CREATE UNIQUE INDEX extraction_runs_once ON extraction_runs (tenant_id, artifact_id, mapping_version)
    WHERE status = 'succeeded' AND supersedes_run IS NULL;
-- A run can be superseded at most once (linear history).
CREATE UNIQUE INDEX extraction_runs_superseded_once ON extraction_runs (supersedes_run) WHERE supersedes_run IS NOT NULL;

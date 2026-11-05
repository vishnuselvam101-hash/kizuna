-- Checkpoints are chained: each signs the SHA-256 of the previous checkpoint's signature (KZCP2). Deleting or
-- replacing an earlier checkpoint breaks the link. Legacy (KZCP1) rows have NULL.
ALTER TABLE audit_checkpoints ADD COLUMN prev_sig_sha256 bytea;

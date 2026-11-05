-- KZCP3 checkpoints also sign the RFC 6962 Merkle root over the chain hashes of events 1..seq, so evidence packs can
-- prove single events included without disclosing the others. Older (KZCP1/KZCP2) rows have NULL.
ALTER TABLE audit_checkpoints ADD COLUMN merkle_root bytea CHECK (merkle_root IS NULL OR length(merkle_root) = 32);

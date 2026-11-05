-- Japanese evidence fidelity (mfg-map@3). Extraction keeps provenance in claims.source_location: `original`
-- (printed 全角 / 和暦 text next to the normalised value), `ocr_conf_bps`, `region` / `bbox_px` (印影 regions),
-- and `review` = 'needs_confirmation' for OCR values below the confidence floor. FAX transmission headers and
-- approval marks are ordinary claims (capture.fax.*, mfg.approval.mark.*), so no new tables are needed.
--
-- Rule: a claim marked needs_confirmation backs no decision until a person accepted it. The API refuses it
-- (claim_needs_confirmation); this trigger makes the rule hold for any writer.
CREATE OR REPLACE FUNCTION kz_dependency_claim_confirmed() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.depends_on_claim IS NOT NULL AND EXISTS (
        SELECT 1 FROM claims c
         WHERE c.tenant_id = NEW.tenant_id AND c.id = NEW.depends_on_claim
           AND c.source_location->>'review' = 'needs_confirmation' AND c.review_status <> 'accepted'
    ) THEN
        RAISE EXCEPTION 'claim needs confirmation before it can back a decision' USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER decision_dependencies_claim_confirmed BEFORE INSERT ON decision_dependencies
    FOR EACH ROW EXECUTE FUNCTION kz_dependency_claim_confirmed();

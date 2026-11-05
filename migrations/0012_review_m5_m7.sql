-- Review M5-M7 fixes.
-- L5: remember which containment action restricted uploads, so a revert only lifts its own restriction.
ALTER TABLE users ADD COLUMN upload_blocked_by_action uuid;

-- M1: AI provenance is sticky; a suggestion leaves `ai_suggested` only by acceptance or rejection.
CREATE OR REPLACE FUNCTION kz_ai_status_insert_only() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.review_status = 'ai_suggested' AND OLD.review_status <> 'ai_suggested' THEN
        RAISE EXCEPTION 'ai_suggested is set at creation only';
    END IF;
    IF OLD.review_status = 'ai_suggested' AND NEW.review_status NOT IN ('ai_suggested', 'accepted', 'rejected') THEN
        RAISE EXCEPTION 'an AI suggestion is only accepted or rejected';
    END IF;
    RETURN NEW;
END $$;

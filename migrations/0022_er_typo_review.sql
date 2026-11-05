-- Entity-resolution settings per tenant (kind 'entity_resolution', versioned and audited like the other tenant
-- settings): `typo_review` = off | strict | broad for the one-character part-number rule (default strict).
ALTER TABLE tenant_settings DROP CONSTRAINT tenant_settings_kind_check;
ALTER TABLE tenant_settings ADD CONSTRAINT tenant_settings_kind_check
    CHECK (kind IN ('ai_extraction', 'detection', 'general', 'entity_resolution'));

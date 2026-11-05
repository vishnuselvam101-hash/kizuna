-- Tenant timezone (IANA name) as versioned, audited tenant settings (kind 'general'); replaces the fixed offset.
ALTER TABLE tenant_settings DROP CONSTRAINT tenant_settings_kind_check;
ALTER TABLE tenant_settings ADD CONSTRAINT tenant_settings_kind_check CHECK (kind IN ('ai_extraction', 'detection', 'general'));
ALTER TABLE tenants DROP COLUMN utc_offset_minutes;

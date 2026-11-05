-- Fixes from docs/security/REVIEW_M2_M4.md.
-- L2: alias chains are flattened when entities are merged.
GRANT UPDATE (canonical_key) ON entity_aliases TO kizuna_app;
GRANT UPDATE (explanation) ON conflicts TO kizuna_app;  -- H2: reopened findings refresh their explanation

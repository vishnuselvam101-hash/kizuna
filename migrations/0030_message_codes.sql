-- Server-generated text as stable message codes + typed parameters ({"code": ..., "params": {...}}), rendered by the
-- web app (and server-rendered letters/reports) in the reader's language. The existing English text columns stay as
-- the plain fallback for API clients; rows written before this migration have no message and show that text.
ALTER TABLE incidents ADD COLUMN summary_msg jsonb;
GRANT UPDATE (summary_msg) ON incidents TO kizuna_app;
ALTER TABLE invalidation_events ADD COLUMN reason_msg jsonb;
ALTER TABLE response_actions ADD COLUMN detail_msg jsonb;
GRANT UPDATE (detail_msg) ON response_actions TO kizuna_app;
